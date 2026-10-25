//! Unattended grant failures must not release expired or unrelated authority.
use super::machine_grant_tests::{access_response, client_fixture, isolated};
use rmcp_client::transport::auth::CredentialStore;
use std::sync::Arc;
use wiremock::{Mock, ResponseTemplate, matchers::method};

#[test]
fn missing_machine_secret_and_anonymous_callers_fail_without_a_provider_request() {
    isolated(
        "upstream::machine_grant_failure_tests::missing_machine_secret_and_anonymous_callers_fail_without_a_provider_request",
        async {
            let directory = tempfile::tempdir().unwrap();
            let (_directory, server, _, config, cache, _, managers) =
                client_fixture(serde_json::json!("client_secret")).await;
            let mut missing = config.clone();
            if let labby_runtime::gateway_config::UpstreamOauthRegistration::Preregistered {
                client_secret_env,
                ..
            } = &mut missing.oauth.as_mut().unwrap().registration
            {
                *client_secret_env =
                    Some("LABBY_MACHINE_GRANT_INTENTIONALLY_MISSING_SECRET".into());
            }
            let original = managers.get("machine").unwrap().clone();
            assert!(
                original.begin_authorization("caller").await.is_err(),
                "machine grants cannot fall back to browser sign-in"
            );
            assert!(
                cache.get_or_build(&config, "").await.is_err(),
                "machine mode cannot admit anonymous callers"
            );
            let sqlite = crate::sqlite::SqliteStore::open(directory.path().join("auth.db"))
                .await
                .unwrap();
            managers.insert(
                "machine".into(),
                super::manager::UpstreamOauthManager::new(
                    sqlite,
                    super::encryption::load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
                        .unwrap(),
                    missing.clone(),
                    "http://127.0.0.1/callback".into(),
                ),
            );
            assert!(cache.get_or_build(&missing, "caller").await.is_err());
            assert!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .all(|request| request.method.as_str() != "POST")
            );
        },
    );
}

#[test]
fn missing_machine_token_expiry_fails_closed_without_persisting_a_token() {
    isolated(
        "upstream::machine_grant_failure_tests::missing_machine_token_expiry_fails_closed_without_persisting_a_token",
        async {
            let (_directory, server, _, config, cache, store, _) =
                client_fixture(serde_json::json!("client_secret")).await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"access_token":"unbounded-token","token_type":"Bearer"}),
                ))
                .mount(&server)
                .await;
            assert!(cache.get_or_build(&config, "caller").await.is_err());
            assert!(store.load().await.unwrap().is_none());
        },
    );
}

#[test]
fn a_late_grant_rejection_cannot_delete_newer_authorization() {
    isolated(
        "upstream::machine_grant_failure_tests::a_late_grant_rejection_cannot_delete_newer_authorization",
        async {
            let (_directory, server, _, config, cache, store, _) =
                client_fixture(serde_json::json!("client_secret")).await;
            access_response(&server).await;
            let client = cache.get_or_build(&config, "caller").await.unwrap();
            let replacement = store.load().await.unwrap().unwrap();
            let mut expired = replacement.clone();
            expired.token_received_at = Some(1);
            store.save(expired).await.unwrap();
            server.reset().await;
            let entered = Arc::new(tokio::sync::Notify::new());
            let observed = Arc::clone(&entered);
            Mock::given(method("POST"))
                .respond_with(move |_: &wiremock::Request| {
                    observed.notify_one();
                    ResponseTemplate::new(400)
                        .set_delay(std::time::Duration::from_secs(1))
                        .set_body_json(serde_json::json!({"error":"invalid_grant"}))
                })
                .mount(&server)
                .await;
            let request = tokio::spawn(async move { client.get_access_token().await });
            tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                .await
                .unwrap();
            store.save(replacement).await.unwrap();
            assert!(request.await.unwrap().is_err());
            assert!(store.load().await.unwrap().is_some());
            assert!(
                !cache.has_pending_invalidations(),
                "failed stale CAS must not invalidate newer authority"
            );
        },
    );
}

async fn initial_grant_race(clear: bool) {
    let (_directory, server, sqlite, config, cache, store, _) =
        client_fixture(serde_json::json!("client_secret")).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let observed = Arc::clone(&entered);
    Mock::given(method("POST")).respond_with(move |_:&wiremock::Request| {
        observed.notify_one();ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(1))
            .set_body_json(serde_json::json!({"access_token":"delayed-access","token_type":"Bearer","expires_in":3600}))
    }).mount(&server).await;
    let caller_cache = cache.clone();
    let request_config = config.clone();
    let request =
        tokio::spawn(async move { caller_cache.get_or_build(&request_config, "caller").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    if clear {
        let _writer = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            cache.invalidation_barrier().write_owned(),
        )
        .await
        .expect("network grant must not hold the lifecycle reader");
        cache.advance_subject_epoch("machine", "caller");
        sqlite
            .clear_upstream_oauth_identity("machine", "caller")
            .await
            .unwrap();
    } else {
        let replacement=serde_json::from_value(serde_json::json!({"client_id":"machine-client","token_response":{"access_token":"newer-access","token_type":"Bearer","expires_in":3600},"granted_scopes":[],"token_received_at":1})).unwrap();
        store.save(replacement).await.unwrap();
    }
    assert!(
        request.await.unwrap().is_err(),
        "a delayed initial grant cannot publish obsolete authority"
    );
    let current = store.load().await.unwrap();
    if clear {
        assert!(
            current.is_none(),
            "initial grant must not recreate authority after clear"
        );
        server.reset().await;
        access_response(&server).await;
        let fresh = cache.get_or_build(&config, "caller").await.expect(
            "an old grant must not poison newly cleared authority with its failure cooldown",
        );
        assert_eq!(fresh.get_access_token().await.unwrap(), "machine-access");
    } else {
        use oauth2::TokenResponse as _;
        assert_eq!(
            current
                .unwrap()
                .token_response
                .unwrap()
                .access_token()
                .secret(),
            "newer-access"
        );
    }
}

#[test]
fn initial_machine_grant_cannot_overwrite_newer_authorization() {
    isolated(
        "upstream::machine_grant_failure_tests::initial_machine_grant_cannot_overwrite_newer_authorization",
        initial_grant_race(false),
    );
}

#[test]
fn initial_machine_grant_cannot_recreate_cleared_authority() {
    isolated(
        "upstream::machine_grant_failure_tests::initial_machine_grant_cannot_recreate_cleared_authority",
        initial_grant_race(true),
    );
}

#[test]
fn repeated_machine_rejection_uses_the_existing_failure_cooldown() {
    isolated(
        "upstream::machine_grant_failure_tests::repeated_machine_rejection_uses_the_existing_failure_cooldown",
        async {
            let (_directory, server, _, config, cache, _, _) =
                client_fixture(serde_json::json!("client_secret")).await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(400)
                        .set_body_json(serde_json::json!({"error":"invalid_grant"})),
                )
                .mount(&server)
                .await;
            assert!(cache.get_or_build(&config, "caller").await.is_err());
            assert!(cache.get_or_build(&config, "caller").await.is_err());
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request.method.as_str() == "POST")
                    .count(),
                1,
                "a known rejected grant must not be retried on every status or client lookup"
            );
        },
    );
}

#[test]
fn unattended_grant_errors_never_invoke_the_browser_reauthorization_hook() {
    isolated(
        "upstream::machine_grant_failure_tests::unattended_grant_errors_never_invoke_the_browser_reauthorization_hook",
        async {
            let (_directory, _server, sqlite, mut config, cache, _, managers) =
                client_fixture(serde_json::json!("client_secret")).await;
            if let labby_runtime::gateway_config::UpstreamOauthRegistration::Preregistered {
                client_secret_env,
                ..
            } = &mut config.oauth.as_mut().unwrap().registration
            {
                *client_secret_env =
                    Some("LABBY_MACHINE_GRANT_INTENTIONALLY_MISSING_SECRET".into());
            }
            managers.insert(
                "machine".into(),
                super::manager::UpstreamOauthManager::new(
                    sqlite,
                    super::encryption::load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
                        .unwrap(),
                    config.clone(),
                    "http://127.0.0.1/callback".into(),
                ),
            );
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed = Arc::clone(&calls);
            let cache = cache.with_reauth_handler(Arc::new(move |_, _| {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async {
                    Err(super::types::OauthError::NeedsReauth(
                        "browser hook must not handle unattended grants".into(),
                    ))
                })
            }));
            assert!(cache.get_or_build(&config, "caller").await.is_err());
            assert!(
                cache
                    .get_or_build_capped(&config, "another-caller", reqwest::Client::new())
                    .await
                    .is_err()
            );
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "machine errors must remain machine errors, without opening an interactive flow"
            );
        },
    );
}

#[test]
fn machine_renewal_does_not_wait_behind_a_writer_while_its_rpc_holds_a_reader() {
    isolated(
        "upstream::machine_grant_failure_tests::machine_renewal_does_not_wait_behind_a_writer_while_its_rpc_holds_a_reader",
        async {
            let (_directory, server, _, config, cache, credentials, _) =
                client_fixture(serde_json::json!("client_secret")).await;
            access_response(&server).await;
            let client = cache.get_or_build(&config, "caller").await.unwrap();
            let mut expired = credentials.load().await.unwrap().unwrap();
            expired.token_received_at = Some(1);
            credentials.save(expired).await.unwrap();
            let barrier = cache.invalidation_barrier();
            let rpc_reader = barrier.read().await;
            let writer_barrier = Arc::clone(&barrier);
            let writer = tokio::spawn(async move { writer_barrier.write_owned().await });
            // Tokio's fair lock blocks new readers once the writer is queued.
            for _ in 0..100 {
                if barrier.try_read().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(
                barrier.try_read().is_err(),
                "writer must be queued before renewal"
            );
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                client.get_access_token(),
            )
            .await;
            assert!(
                result.is_ok(),
                "renewal must fail closed without waiting on the RPC's own reader"
            );
            assert!(
                result.unwrap().is_err(),
                "pending invalidation must prevent publication"
            );
            drop(rpc_reader);
            drop(writer.await.unwrap());
        },
    );
}

#[test]
fn cancelled_machine_grant_callers_retain_one_worker_and_failure_cooldown() {
    isolated(
        "upstream::machine_grant_failure_tests::cancelled_machine_grant_callers_retain_one_worker_and_failure_cooldown",
        async {
            let (_directory, server, _, config, cache, store, managers) =
                client_fixture(serde_json::json!("client_secret")).await;
            let entered = Arc::new(tokio::sync::Notify::new());
            let observed = Arc::clone(&entered);
            Mock::given(method("POST"))
                .respond_with(move |_: &wiremock::Request| {
                    observed.notify_one();
                    ResponseTemplate::new(400)
                        .set_delay(std::time::Duration::from_millis(750))
                        .set_body_json(serde_json::json!({"error":"invalid_grant"}))
                })
                .mount(&server)
                .await;
            let start = || {
                let cache = cache.clone();
                let config = config.clone();
                tokio::spawn(async move { cache.get_or_build(&config, "caller").await })
            };
            let first = start();
            tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                .await
                .unwrap();
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
            for _ in 0..3 {
                let caller = start();
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while managers
                    .get("machine")
                    .unwrap()
                    .unattended_grant_running("caller")
                {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("retained worker must finish within its provider response budget");
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                cache.get_or_build(&config, "caller"),
            )
            .await
            .expect("the retained worker must have published the cooldown");
            assert!(result.is_err());
            let posts = server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|request| request.method.as_str() == "POST")
                .count();
            assert_eq!(
                posts, 1,
                "cancelled callers must join one retained worker, including failure publication"
            );
            assert!(store.load().await.unwrap().is_none());
        },
    );
}

#[test]
fn machine_grant_deadline_includes_credential_guard_admission() {
    isolated(
        "upstream::machine_grant_failure_tests::machine_grant_deadline_includes_credential_guard_admission",
        async {
            let (_directory, server, _, config, cache, store, managers) =
                client_fixture(serde_json::json!("client_secret")).await;
            let guard = store.acquire_refresh_guard().await.unwrap();
            let request = {
                let cache = cache.clone();
                let config = config.clone();
                tokio::spawn(async move { cache.get_or_build(&config, "caller").await })
            };
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(91)).await;
            for _ in 0..100 {
                tokio::task::yield_now().await;
            }
            assert!(
                request.is_finished(),
                "grant deadline must include waiting for the identity guard"
            );
            assert_eq!(
                request
                    .await
                    .unwrap()
                    .expect_err("deadline must fail")
                    .kind(),
                "timeout"
            );
            assert!(
                !managers
                    .get("machine")
                    .unwrap()
                    .unattended_grant_running("caller")
            );
            drop(guard);
            tokio::time::resume();
            assert!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .all(|r| r.method.as_str() != "POST")
            );
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    cache.get_or_build(&config, "caller")
                )
                .await
                .unwrap()
                .is_err()
            );
            assert!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .all(|r| r.method.as_str() != "POST")
            );
        },
    );
}

#[test]
fn cancelled_machine_status_waiters_release_coordination_ownership() {
    isolated(
        "upstream::machine_grant_failure_tests::cancelled_machine_status_waiters_release_coordination_ownership",
        async {
            let (_directory, server, _, _, cache, _, managers) =
                client_fixture(serde_json::json!("client_secret")).await;
            let manager = managers
                .get("machine")
                .unwrap()
                .clone()
                .with_grant_publication_policy(
                    cache.invalidation_barrier(),
                    cache.lifecycle_epoch_for("machine", "caller"),
                );
            let entered = Arc::new(tokio::sync::Notify::new());
            let observed = Arc::clone(&entered);
            Mock::given(method("POST"))
                .respond_with(move |_: &wiremock::Request| {
                    observed.notify_one();
                    ResponseTemplate::new(400)
                        .set_delay(std::time::Duration::from_millis(750))
                        .set_body_json(serde_json::json!({"error":"invalid_grant"}))
                })
                .mount(&server)
                .await;
            let mut first = Box::pin(manager.refresh_auth_client_if_due("caller"));
            assert!(
                std::future::poll_fn(|cx| std::task::Poll::Ready(
                    first.as_mut().poll(cx).is_pending()
                ))
                .await
            );
            tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                .await
                .unwrap();
            drop(first);
            let baseline = manager.unattended_coordination_owners();
            for _ in 0..3 {
                let mut waiter = Box::pin(manager.refresh_auth_client_if_due("caller"));
                assert!(
                    std::future::poll_fn(|cx| std::task::Poll::Ready(
                        waiter.as_mut().poll(cx).is_pending()
                    ))
                    .await
                );
                for _ in 0..100 {
                    tokio::task::yield_now().await;
                }
                drop(waiter);
                assert_eq!(
                    manager.unattended_coordination_owners(),
                    baseline,
                    "cancelled status callers must not leave detached coordination waiters"
                );
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while manager.unattended_grant_running("caller") {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert!(manager.refresh_auth_client_if_due("caller").await.is_err());
            assert_eq!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .filter(|request| request.method.as_str() == "POST")
                    .count(),
                1
            );
        },
    );
}
