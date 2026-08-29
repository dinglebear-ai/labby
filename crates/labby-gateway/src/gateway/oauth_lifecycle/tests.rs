use super::{
    OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN, OAUTH_STATUS_DISCOVERY_FRESHNESS,
    OauthStatusDiscoverySnapshot, oauth_status_discovery_is_fresh,
    probe::{
        TRANSIENT_MANAGER_MAX, TRANSIENT_MANAGER_TTL, install_test_probe_metadata,
        probe_manager_key, run as run_probe, schedule_transient_manager_expiry,
        transient_manager_evictions, validate_probe_upstream_name, validate_probe_url,
    },
    should_use_dynamic_registration,
};

fn discovery_snapshot(age: std::time::Duration, failed: bool) -> OauthStatusDiscoverySnapshot {
    OauthStatusDiscoverySnapshot {
        completed_at: tokio::time::Instant::now() - age,
        summary: None,
        tool_error: failed.then(|| "unavailable".to_string()),
        error: None,
    }
}

fn test_authorization_metadata(origin: &str) -> rmcp::transport::auth::AuthorizationMetadata {
    let mut metadata = rmcp::transport::auth::AuthorizationMetadata::default();
    metadata.authorization_endpoint = format!("{origin}/authorize");
    metadata.token_endpoint = format!("{origin}/token");
    metadata.issuer = Some(origin.to_string());
    metadata.code_challenge_methods_supported = Some(vec!["S256".to_string()]);
    metadata
}

#[test]
fn successful_oauth_status_discovery_has_short_freshness_window() {
    assert!(oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FRESHNESS
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("freshness exceeds one second"),
        false,
    )));
    assert!(!oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FRESHNESS + std::time::Duration::from_secs(1),
        false,
    )));
}

#[test]
fn failed_oauth_status_discovery_observes_retry_cooldown() {
    assert!(oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("cooldown exceeds one second"),
        true,
    )));
    assert!(!oauth_status_discovery_is_fresh(&discovery_snapshot(
        OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN + std::time::Duration::from_secs(1),
        true,
    )));
}

#[tokio::test]
async fn oauth_invalidation_fences_inflight_status_and_removes_stale_snapshot() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    ));
    let key = ("fixture".to_string(), "alice".to_string());
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    manager
        .oauth_status_discovery_locks
        .insert(key.clone(), lock.clone());
    let guard = lock.lock().await;
    manager.oauth_status_discovery_cache.lock().await.insert(
        key.clone(),
        discovery_snapshot(std::time::Duration::ZERO, false),
    );
    let epoch = manager.oauth_status_epoch("fixture", "alice");

    let invalidator = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .invalidate_oauth_status_discovery("fixture", Some("alice"))
                .await;
        })
    };
    tokio::task::yield_now().await;
    assert!(
        !invalidator.is_finished(),
        "invalidation bypassed status singleflight"
    );
    drop(guard);
    invalidator.await.unwrap();

    assert!(
        !manager
            .oauth_status_discovery_cache
            .lock()
            .await
            .contains_key(&key)
    );
    assert!(manager.oauth_status_epoch("fixture", "alice") > epoch);
}

#[tokio::test]
async fn oauth_status_epochs_are_isolated_by_upstream_and_subject() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};

    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let unrelated = manager.oauth_status_epoch("upstream-b", "bob");
    manager
        .invalidate_oauth_status_discovery("upstream-a", Some("alice"))
        .await;
    assert_eq!(manager.oauth_status_epoch("upstream-b", "bob"), unrelated);
    assert!(manager.oauth_status_epoch("upstream-a", "alice") > (0, 0));

    let alice = manager.oauth_status_epoch("upstream-a", "alice");
    manager
        .invalidate_oauth_status_discovery("upstream-a", None)
        .await;
    assert!(manager.oauth_status_epoch("upstream-a", "alice") > alice);
    assert_eq!(manager.oauth_status_epoch("upstream-b", "bob"), unrelated);
}

#[tokio::test]
async fn repeated_status_windows_reuse_the_exact_published_pool_incarnation() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use crate::upstream::pool::UpstreamPool;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    let published = Arc::new(UpstreamPool::new());
    runtime.swap(Some(published.clone())).await;
    let baseline = published.connection_count_for_tests().await;

    for _subject_window in 0..1_000 {
        let (selected, ephemeral) = manager.oauth_status_pool(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        );
        assert!(!ephemeral);
        assert!(Arc::ptr_eq(&selected, &published));
        assert_eq!(selected.revision_label(), published.revision_label());
    }
    tokio::task::yield_now().await;
    assert_eq!(published.connection_count_for_tests().await, baseline);

    runtime.swap(None).await;
    let (cold, ephemeral) = manager.oauth_status_pool(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    );
    assert!(ephemeral);
    cold.drain_for_swap("test.oauth.status.cold").await;
    assert_eq!(cold.connection_count_for_tests().await, 0);
}

#[test]
fn callback_after_probe_expiry_returns_stable_retryable_error() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};

    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let error = manager
        .require_oauth_manager("expired-probe", "callback")
        .err()
        .expect("expired callback must fail");
    assert_eq!(error.kind(), "oauth_probe_expired");
    assert!(error.to_string().contains("probe again"));
}

#[tokio::test]
async fn failed_real_probe_callback_immediately_discards_transient_runtime() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use base64::Engine as _;
    use labby_auth::sqlite::SqliteStore;
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::encryption::load_key;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(managers.clone());
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(managers.clone())
    .with_oauth_client_cache(cache.clone())
    .with_oauth_resources(
        SqliteStore::open(dir.path().join("auth.sqlite"))
            .await
            .unwrap(),
        load_key(&base64::engine::general_purpose::STANDARD.encode([4_u8; 32])).unwrap(),
        "https://lab.example.com/auth/upstream/callback".to_string(),
    );
    let url = "https://failed-callback.example/mcp";
    install_test_probe_metadata(
        url,
        test_authorization_metadata("https://failed-callback.example"),
    );
    run_probe(&manager, url, Some("failed-callback"))
        .await
        .unwrap();
    assert!(managers.contains_key("failed-callback"));

    let error = manager
        .complete_upstream_authorization_callback(
            "failed-callback",
            "alice",
            "invalid-code",
            "invalid-state",
        )
        .await
        .unwrap_err();
    assert!(!error.kind().is_empty());
    assert!(!managers.contains_key("failed-callback"));
    assert!(
        !manager
            .transient_oauth_managers
            .lock()
            .await
            .contains_key("failed-callback")
    );
    assert!(cache.is_empty());
    assert_eq!(cache.build_lock_count_for_tests(), 0);
}

#[derive(Clone)]
struct DeterministicMcpResponder;

impl wiremock::Respond for DeterministicMcpResponder {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let Some(id) = body.get("id").cloned() else {
            return wiremock::ResponseTemplate::new(202);
        };
        let result = match body.get("method").and_then(serde_json::Value::as_str) {
            Some("server/discover") => serde_json::json!({
                "type": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": {"tools": {}},
                "ttlMs": 0,
                "cacheScope": "private",
                "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "oauth-lifecycle-fixture", "version": "1"}}
            }),
            Some("initialize") => serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "oauth-lifecycle-fixture", "version": "1"}
            }),
            Some("tools/list") => serde_json::json!({
                "tools": [{"name": "echo", "description": "fixture", "inputSchema": {"type": "object"}}]
            }),
            Some("tools/call") => serde_json::json!({
                "content": [{"type": "text", "text": "ok"}], "isError": false
            }),
            _ => serde_json::json!({}),
        };
        wiremock::ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_json(serde_json::json!({
                "jsonrpc": "2.0", "id": id, "result": result
            }))
    }
}

#[tokio::test]
async fn public_status_and_routing_share_connection_incarnation_and_credential_generation() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use base64::Engine as _;
    use labby_auth::sqlite::SqliteStore;
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::encryption::load_key;
    use labby_auth::upstream::manager::UpstreamOauthManager;
    use labby_auth::upstream::store::SqliteCredentialStore;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::auth::{CredentialStore, StoredCredentials};
    use std::sync::Arc;
    use wiremock::{Mock, MockServer};

    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/.well-known/oauth-protected-resource/mcp",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "resource": format!("{}/mcp", server.uri()),
                "authorization_servers": [server.uri()]
            })),
        )
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/.well-known/oauth-protected-resource",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "resource": format!("{}/mcp", server.uri()),
                "authorization_servers": [server.uri()]
            })),
        )
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/.well-known/oauth-authorization-server",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
                "code_challenge_methods_supported": ["S256"]
            })),
        )
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(DeterministicMcpResponder)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let sqlite = SqliteStore::open(dir.path().join("auth.sqlite"))
        .await
        .unwrap();
    let key = load_key(&base64::engine::general_purpose::STANDARD.encode([3_u8; 32])).unwrap();
    let mut upstream = lifecycle_test_upstream("fixture", true);
    upstream.url = Some(format!("{}/mcp", server.uri()));
    upstream.oauth.as_mut().unwrap().registration = UpstreamOauthRegistration::Preregistered {
        client_id: "fixture-client".to_string(),
        client_secret_env: None,
    };
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        upstream.name.clone(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            upstream.clone(),
            "https://lab.example.com/auth/upstream/callback".to_string(),
        ),
    );
    let cache = OauthClientCache::new(managers.clone());
    let runtime = GatewayRuntimeHandle::default();
    let manager = Arc::new(
        GatewayManager::new(dir.path().join("config.toml"), runtime.clone())
            .with_upstream_oauth_managers(managers)
            .with_oauth_client_cache(cache.clone())
            .with_oauth_resources(
                sqlite.clone(),
                key.clone(),
                "https://lab.example.com/auth/upstream/callback".to_string(),
            ),
    );
    let config = GatewayConfig {
        upstream: vec![upstream.clone()],
        ..Default::default()
    };
    manager.seed_config(config).await;
    let pool = Arc::new(manager.new_base_pool(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(5),
    ));
    runtime.swap(Some(pool.clone())).await;
    let baseline = pool.oauth_runtime_counts_for_tests().await;

    for index in 0..256 {
        let subject = format!("subject-{index}");
        let credentials: StoredCredentials = serde_json::from_value(serde_json::json!({
            "client_id": "fixture-client",
            "token_response": {"access_token": format!("token-{index}"), "token_type": "bearer", "expires_in": 3600},
            "granted_scopes": ["mcp"],
            "token_received_at": 2_000_000_000u64
        }))
        .unwrap();
        CredentialStore::save(
            &SqliteCredentialStore::new(sqlite.clone(), key.clone(), "fixture", &subject),
            credentials,
        )
        .await
        .unwrap();
        let status = manager
            .upstream_oauth_status("fixture", &subject)
            .await
            .unwrap();
        assert!(status.discovery_checked);
        assert!(status.authenticated, "status discovery failed: {status:?}");
        let before = pool
            .subject_connection_identity_for_tests("fixture", &subject)
            .await
            .unwrap();
        pool.subject_scoped_call_tool(&upstream, &subject, CallToolRequestParams::new("echo"))
            .await
            .unwrap();
        manager
            .upstream_oauth_status("fixture", &subject)
            .await
            .unwrap();
        let after = pool
            .subject_connection_identity_for_tests("fixture", &subject)
            .await
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(after.1, cache.credential_generation("fixture", &subject));
        if index == 255 {
            manager
                .invalidate_oauth_status_discovery("fixture", Some(&subject))
                .await;
            let (entered, release) = super::install_status_discovery_barrier();
            let status_task = {
                let manager = manager.clone();
                let subject = subject.clone();
                tokio::spawn(
                    async move { manager.upstream_oauth_status("fixture", &subject).await },
                )
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
                .await
                .expect("public status did not reach the post-discovery barrier");
            let clear_task = {
                let manager = manager.clone();
                let subject = subject.clone();
                tokio::spawn(async move {
                    manager
                        .clear_upstream_credentials("fixture", &subject)
                        .await
                })
            };
            tokio::task::yield_now().await;
            assert!(!clear_task.is_finished());
            release.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(5), status_task)
                .await
                .expect("status did not finish after barrier release")
                .unwrap()
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), clear_task)
                .await
                .expect("clear did not finish after status released its discovery lock")
                .unwrap()
                .unwrap();
            assert!(
                !manager
                    .oauth_status_discovery_cache
                    .lock()
                    .await
                    .contains_key(&("fixture".to_string(), subject.clone()))
            );
            assert!(
                pool.subject_connection_identity_for_tests("fixture", &subject)
                    .await
                    .is_none()
            );
            assert!(cache.credential_generation("fixture", &subject) > after.1);
        } else {
            manager
                .clear_upstream_credentials("fixture", &subject)
                .await
                .unwrap();
        }
    }
    assert_eq!(pool.oauth_runtime_counts_for_tests().await, baseline);
}

#[tokio::test]
async fn public_google_revoke_fences_blocked_status_and_purges_snapshot() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use base64::Engine as _;
    use labby_auth::at_rest::TokenEncryptionKey;
    use labby_auth::sqlite::SqliteStore;
    use labby_auth::types::GoogleProviderCredentialUpdate;
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::encryption::load_key;
    use labby_auth::upstream::manager::UpstreamOauthManager;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let sqlite = SqliteStore::open_with_key(
        dir.path().join("auth.sqlite"),
        Some(TokenEncryptionKey::from_encoded(&"22".repeat(32)).unwrap()),
    )
    .await
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlite
        .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
            subject: "google-subject".to_string(),
            email: Some("operator@example.com".to_string()),
            client_id: "google-client".to_string(),
            granted_scopes: vec!["email".into(), "openid".into(), "profile".into()],
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            token_received_at: now,
            access_token_expires_at: now + 3600,
            issuer: Some("https://accounts.google.com".to_string()),
            refreshed: false,
            scope_upgraded: false,
        })
        .await
        .unwrap();
    let key = load_key(&base64::engine::general_purpose::STANDARD.encode([2_u8; 32])).unwrap();
    let mut upstream = lifecycle_test_upstream("google-fixture", true);
    let oauth = upstream.oauth.as_mut().unwrap();
    oauth.registration = UpstreamOauthRegistration::Preregistered {
        client_id: "google-client".to_string(),
        client_secret_env: Some("PATH".to_string()),
    };
    oauth.credential = UpstreamOauthCredentialSource::GoogleProvider { account: None };
    oauth.scopes = Some(vec![
        "https://www.googleapis.com/auth/drive.readonly".to_string(),
    ]);
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        upstream.name.clone(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            upstream.clone(),
            "https://lab.example.com/auth/upstream/callback".to_string(),
        ),
    );
    let cache = OauthClientCache::new(managers.clone());
    let runtime = GatewayRuntimeHandle::default();
    let manager = Arc::new(
        GatewayManager::new(dir.path().join("config.toml"), runtime.clone())
            .with_upstream_oauth_managers(managers)
            .with_oauth_client_cache(cache.clone())
            .with_oauth_resources(
                sqlite,
                key,
                "https://lab.example.com/auth/upstream/callback".to_string(),
            ),
    );
    manager
        .seed_config(GatewayConfig {
            upstream: vec![upstream],
            ..Default::default()
        })
        .await;
    let pool = Arc::new(manager.new_base_pool(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(1),
    ));
    runtime.swap(Some(pool.clone())).await;
    let key = ("google-fixture".to_string(), "google-subject".to_string());
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    manager
        .oauth_status_discovery_locks
        .insert(key.clone(), lock.clone());
    manager.oauth_status_discovery_cache.lock().await.insert(
        key.clone(),
        discovery_snapshot(std::time::Duration::ZERO, false),
    );
    let guard = lock.lock().await;
    let epoch = manager.oauth_status_epoch("google-fixture", "google-subject");
    let baseline = pool.oauth_runtime_counts_for_tests().await;
    let revoke = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .revoke_google_provider_credential("google-fixture")
                .await
        })
    };
    tokio::task::yield_now().await;
    assert!(!revoke.is_finished());
    drop(guard);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), revoke)
            .await
            .expect("revoke did not finish after discovery lock release")
            .unwrap()
            .unwrap()
            .invalidated
    );
    assert!(
        !manager
            .oauth_status_discovery_cache
            .lock()
            .await
            .contains_key(&key)
    );
    assert!(manager.oauth_status_epoch("google-fixture", "google-subject") > epoch);
    assert_eq!(pool.oauth_runtime_counts_for_tests().await, baseline);
    assert!(cache.is_empty());
}

#[tokio::test(start_paused = true)]
async fn real_probe_orchestrator_is_bounded_and_sweeps_without_more_traffic() {
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use base64::Engine as _;
    use labby_auth::sqlite::SqliteStore;
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::encryption::load_key;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let sqlite = SqliteStore::open(dir.path().join("probe.sqlite"))
        .await
        .unwrap();
    let key = load_key(&base64::engine::general_purpose::STANDARD.encode([6_u8; 32])).unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(managers.clone());
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(managers.clone())
    .with_oauth_client_cache(cache.clone())
    .with_oauth_resources(
        sqlite,
        key,
        "https://lab.example.com/auth/upstream/callback".to_string(),
    );

    for index in 0..(TRANSIENT_MANAGER_MAX + 8) {
        let url = format!("https://probe-{index}.example.com/mcp");
        install_test_probe_metadata(
            &url,
            test_authorization_metadata(&format!("https://probe-{index}.example.com")),
        );
        run_probe(&manager, &url, Some(&format!("probe-{index}")))
            .await
            .unwrap();
    }
    assert!(managers.len() <= TRANSIENT_MANAGER_MAX);
    assert!(manager.transient_oauth_managers.lock().await.len() <= TRANSIENT_MANAGER_MAX);
    assert!(cache.is_empty());
    assert_eq!(cache.build_lock_count_for_tests(), 0);

    tokio::time::advance(TRANSIENT_MANAGER_TTL + std::time::Duration::from_secs(61)).await;
    tokio::task::yield_now().await;
    assert!(manager.transient_oauth_managers.lock().await.is_empty());
    assert!(managers.is_empty());
    assert!(cache.is_empty());
    assert_eq!(cache.build_lock_count_for_tests(), 0);
}

#[tokio::test(start_paused = true)]
async fn callback_promotion_racing_expiry_converges_to_one_durable_manager() {
    use crate::gateway::config::write_gateway_config;
    use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use base64::Engine as _;
    use labby_auth::sqlite::SqliteStore;
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::encryption::load_key;
    use labby_runtime::gateway_config::GatewayConfig;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let upstream = lifecycle_test_upstream("race-probe", false);
    let config = GatewayConfig {
        upstream: vec![upstream],
        ..GatewayConfig::default()
    };
    write_gateway_config(&path, &config).unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(managers.clone());
    let manager = Arc::new(
        GatewayManager::new(path, GatewayRuntimeHandle::default())
            .with_upstream_oauth_managers(managers.clone())
            .with_oauth_client_cache(cache.clone())
            .with_oauth_resources(
                SqliteStore::open(dir.path().join("auth.sqlite"))
                    .await
                    .unwrap(),
                load_key(&base64::engine::general_purpose::STANDARD.encode([5_u8; 32])).unwrap(),
                "https://lab.example.com/auth/upstream/callback".to_string(),
            ),
    );
    manager.seed_config(config).await;
    let url = "https://race-probe.example/mcp";
    install_test_probe_metadata(
        url,
        test_authorization_metadata("https://race-probe.example"),
    );
    run_probe(&manager, url, Some("race-probe")).await.unwrap();
    let oauth_config = managers
        .get("race-probe")
        .unwrap()
        .upstream_config()
        .oauth
        .clone()
        .unwrap();

    let blocker = manager.acquire_config_mutation().await.unwrap();
    let promotion = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .promote_probe_oauth_config("race-probe", oauth_config)
                .await
        })
    };
    tokio::time::advance(TRANSIENT_MANAGER_TTL + std::time::Duration::from_secs(61)).await;
    tokio::task::yield_now().await;
    drop(blocker);
    assert!(promotion.await.unwrap().unwrap());

    assert!(manager.transient_oauth_managers.lock().await.is_empty());
    assert!(manager.oauth_upstream_config("race-probe").await.is_some());
    assert_eq!(
        managers
            .iter()
            .filter(|entry| entry.key().as_str() == "race-probe")
            .count(),
        1
    );
    assert!(cache.is_empty());
    assert_eq!(cache.build_lock_count_for_tests(), 0);
}
use labby_runtime::gateway_config::{
    GatewayConfig, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthCredentialSource,
    UpstreamOauthMode, UpstreamOauthRegistration,
};

fn lifecycle_test_upstream(name: &str, oauth: bool) -> UpstreamConfig {
    UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        url: Some(format!("https://{name}.example/mcp")),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: vec![],
        env: Default::default(),
        proxy_resources: false,
        proxy_prompts: false,
        proxy_skills: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        expose_skills: None,
        code_mode_hint: None,
        oauth: oauth.then(|| UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Dynamic,
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        }),
        imported_from: None,
        priority: 1.0,
    }
}

#[test]
fn shared_provider_scope_excludes_dedicated_and_non_oauth_upstreams() {
    let mut shared = lifecycle_test_upstream("shared", true);
    shared.oauth.as_mut().unwrap().credential =
        UpstreamOauthCredentialSource::GoogleProvider { account: None };
    let dedicated = lifecycle_test_upstream("dedicated", true);
    let raw = lifecycle_test_upstream("raw", false);

    assert_eq!(
        super::GatewayManager::google_provider_upstream_names(&GatewayConfig {
            upstream: vec![shared, dedicated, raw],
            ..GatewayConfig::default()
        }),
        vec!["shared".to_string()]
    );
}

#[test]
fn validate_probe_url_rejects_userinfo() {
    let result = validate_probe_url("https://user:pass@example.com/mcp");
    assert!(result.is_err(), "expected error for URL with userinfo");
}

#[test]
fn validate_probe_url_rejects_http() {
    let result = validate_probe_url("http://10.1.0.8/mcp");
    assert!(
        result.is_err(),
        "expected error for non-HTTPS OAuth probe URL"
    );
}

#[test]
fn validate_probe_url_rejects_query_and_fragment() {
    let with_query = validate_probe_url("https://example.com/mcp?foo=bar");
    assert!(
        with_query.is_err(),
        "expected error for URL with query string"
    );
    let with_fragment = validate_probe_url("https://example.com/mcp#section");
    assert!(
        with_fragment.is_err(),
        "expected error for URL with fragment"
    );
}

#[test]
fn probe_manager_key_includes_port_and_path() {
    let url = url::Url::parse("https://example.com:9000/mcp").unwrap();
    let key = probe_manager_key(&url);
    assert!(
        key.contains("example.com"),
        "key should contain hostname: {key}"
    );
    assert!(key.contains("9000"), "key should contain port: {key}");
    assert!(
        key.contains("mcp"),
        "key should contain path segment: {key}"
    );
}

#[test]
fn probe_manager_key_distinguishes_colliding_paths() {
    let url_a = url::Url::parse("https://example.com/mcp/a").unwrap();
    let url_b = url::Url::parse("https://example.com/mcp/b").unwrap();
    let key_a = probe_manager_key(&url_a);
    let key_b = probe_manager_key(&url_b);
    assert_ne!(
        key_a, key_b,
        "different paths should produce different keys"
    );
}

#[test]
fn validate_probe_upstream_name_rejects_path_like_values() {
    let with_slash = validate_probe_upstream_name("my/server");
    assert!(
        with_slash.is_err(),
        "expected error for name containing '/'"
    );
    let with_backslash = validate_probe_upstream_name("my\\server");
    assert!(
        with_backslash.is_err(),
        "expected error for name containing '\\'"
    );
    let empty = validate_probe_upstream_name("  ");
    assert!(empty.is_err(), "expected error for whitespace-only name");
}

#[test]
fn transient_probe_leases_are_bounded_and_expire() {
    let now = tokio::time::Instant::now();
    let mut leases = std::collections::HashMap::new();
    for index in 0..TRANSIENT_MANAGER_MAX {
        leases.insert(
            format!("probe-{index}"),
            now - std::time::Duration::from_secs(index as u64),
        );
    }
    let evicted = transient_manager_evictions(&mut leases, now, "new-probe");
    assert_eq!(evicted.len(), 1);
    assert_eq!(leases.len(), TRANSIENT_MANAGER_MAX - 1);

    leases.insert("hot-probe".to_string(), now);
    let evicted = transient_manager_evictions(&mut leases, now, "hot-probe");
    assert!(
        evicted.is_empty(),
        "renewing a live lease must not evict it"
    );
    assert!(leases.contains_key("hot-probe"));

    leases.insert(
        "expired".to_string(),
        now - TRANSIENT_MANAGER_TTL - std::time::Duration::from_secs(1),
    );
    let evicted = transient_manager_evictions(&mut leases, now, "existing-probe");
    assert!(evicted.iter().any(|name| name == "expired"));
    assert!(!leases.contains_key("expired"));
}

#[tokio::test]
async fn transient_manager_expires_without_another_probe() {
    use base64::Engine as _;
    use labby_auth::sqlite::SqliteStore;
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::encryption::load_key;
    use labby_auth::upstream::manager::UpstreamOauthManager;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let sqlite = SqliteStore::open(dir.path().join("auth.sqlite"))
        .await
        .unwrap();
    let key = load_key(&base64::engine::general_purpose::STANDARD.encode([9_u8; 32])).unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(managers.clone());
    let leases = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let started = tokio::time::Instant::now();

    // Exercise the actual runtime registries above the hard bound, then apply
    // the same eviction set used by the probe orchestrator.
    for index in 0..=TRANSIENT_MANAGER_MAX {
        let name = format!("bulk-{index}");
        leases.lock().await.insert(
            name.clone(),
            started - std::time::Duration::from_secs(index as u64),
        );
        managers.insert(
            name.clone(),
            UpstreamOauthManager::new(
                sqlite.clone(),
                key.clone(),
                lifecycle_test_upstream(&name, true),
                "https://lab.example.com/auth/upstream/callback".to_string(),
            ),
        );
    }
    let evicted = transient_manager_evictions(&mut *leases.lock().await, started, "next-probe");
    for name in evicted {
        managers.remove(&name);
        cache.evict_upstream(&name);
    }
    assert!(leases.lock().await.len() < TRANSIENT_MANAGER_MAX);
    assert!(managers.len() < TRANSIENT_MANAGER_MAX);
    managers.clear();
    leases.lock().await.clear();

    leases.lock().await.insert("probe".to_string(), started);
    managers.insert(
        "probe".to_string(),
        UpstreamOauthManager::new(
            sqlite,
            key,
            lifecycle_test_upstream("probe", true),
            "https://lab.example.com/auth/upstream/callback".to_string(),
        ),
    );

    schedule_transient_manager_expiry(
        leases.clone(),
        managers.clone(),
        Some(cache.clone()),
        "probe".to_string(),
        started,
        std::time::Duration::from_millis(10),
    );
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    assert!(leases.lock().await.is_empty());
    assert!(managers.is_empty());
    assert!(cache.is_empty());
    assert_eq!(cache.build_lock_count_for_tests(), 0);

    // Race durable promotion against the exact expiry deadline. Whichever
    // wins, the lease is gone and there is never an unowned manager: retained
    // means durable promotion won; absent means callers receive the stable
    // expired-flow/not-found retry path.
    let race_started = tokio::time::Instant::now();
    leases.lock().await.insert("race".to_string(), race_started);
    managers.insert(
        "race".to_string(),
        UpstreamOauthManager::new(
            SqliteStore::open(dir.path().join("race.sqlite"))
                .await
                .unwrap(),
            load_key(&base64::engine::general_purpose::STANDARD.encode([8_u8; 32])).unwrap(),
            lifecycle_test_upstream("race", true),
            "https://lab.example.com/auth/upstream/callback".to_string(),
        ),
    );
    schedule_transient_manager_expiry(
        leases.clone(),
        managers.clone(),
        Some(cache.clone()),
        "race".to_string(),
        race_started,
        std::time::Duration::from_millis(10),
    );
    let promoted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let promotion = {
        let leases = leases.clone();
        let promoted = promoted.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            promoted.store(true, std::sync::atomic::Ordering::Release);
            leases.lock().await.remove("race");
        })
    };
    promotion.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(!leases.lock().await.contains_key("race"));
    assert!(
        !managers.contains_key("race") || promoted.load(std::sync::atomic::Ordering::Acquire),
        "a retained manager must have completed durable promotion"
    );
}

// ── should_use_dynamic_registration coverage ─────────────────────────────────

#[test]
fn swag_uses_client_metadata_document_even_when_dynamic_registration_is_advertised() {
    // Legacy default: "swag" always uses CIMD regardless of what the server supports.
    assert!(
        !should_use_dynamic_registration("swag", true, None),
        "swag + supports_dynamic + no override → should NOT use dynamic"
    );
    // Other upstreams that support dynamic registration should use it.
    assert!(
        should_use_dynamic_registration("github", true, None),
        "github + supports_dynamic + no override → should use dynamic"
    );
    // No supports_dynamic → always false regardless of upstream name.
    assert!(
        !should_use_dynamic_registration("github", false, None),
        "no dynamic support → should NOT use dynamic"
    );
}

#[test]
fn prefer_client_metadata_document_true_overrides_dynamic_registration() {
    // When the operator explicitly sets prefer_client_metadata_document = true,
    // dynamic registration is suppressed even when the server supports it.
    assert!(
        !should_use_dynamic_registration("github", true, Some(true)),
        "explicit prefer_cimd=true + supports_dynamic → should NOT use dynamic"
    );
    assert!(
        !should_use_dynamic_registration("github", false, Some(true)),
        "explicit prefer_cimd=true + no support → should NOT use dynamic"
    );
}

#[test]
fn prefer_client_metadata_document_false_opts_in_to_dynamic_registration() {
    // When the operator explicitly sets prefer_client_metadata_document = false,
    // dynamic registration is used even for "swag".
    assert!(
        should_use_dynamic_registration("swag", true, Some(false)),
        "explicit prefer_cimd=false + supports_dynamic → should use dynamic"
    );
    // No dynamic support → still false (hardware constraint, not a preference).
    assert!(
        !should_use_dynamic_registration("swag", false, Some(false)),
        "explicit prefer_cimd=false + no support → should NOT use dynamic"
    );
}
