//! Upstream OAuth manager/cache reconciliation tests across reloads.

use crate::gateway::config::write_gateway_config;
use labby_auth::upstream::cache::OauthClientCache;
use labby_auth::upstream::manager::UpstreamOauthManager;
use labby_runtime::gateway_config::{
    UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::*;

#[tokio::test]
async fn reload_evicts_removed_upstream_oauth_clients() {
    // Keep the shared client fixture covered without exposing a production
    // cache-insertion seam solely for downstream tests.
    drop(dummy_auth_client().await);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut kept_upstream = fixture_http_upstream("kept");
    kept_upstream.url = Some("https://fixture.example.com:7001".to_string());
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![kept_upstream],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    managers.insert(
        "removed".to_string(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            fixture_oauth_upstream("removed", "https://removed.example.com/mcp"),
            redirect_uri.clone(),
        ),
    );

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default())
        .with_upstream_oauth_managers(Arc::clone(&managers))
        .with_oauth_client_cache(cache.clone())
        .with_oauth_resources(sqlite, key, redirect_uri);
    let mut removed_upstream = fixture_http_upstream("removed");
    removed_upstream.url = Some("http://127.0.0.1:7000".to_string());
    removed_upstream.oauth = Some(UpstreamOauthConfig {
        mode: UpstreamOauthMode::AuthorizationCodePkce,
        registration: UpstreamOauthRegistration::Dynamic,
        scopes: None,
        credential: Default::default(),
        additional_endpoint_origins: vec![],
        prefer_client_metadata_document: None,
    });
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream: vec![removed_upstream],
            ..GatewayConfig::default()
        })
        .await;

    assert!(managers.contains_key("removed"));
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");
    assert!(cache.is_empty());
    assert!(!managers.contains_key("removed"));
}

#[tokio::test]
async fn reload_registers_new_upstream_oauth_manager() {
    let dir = tempfile::tempdir().expect("tempdir");
    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(Arc::clone(&managers))
    .with_oauth_client_cache(cache)
    .with_oauth_resources(sqlite, key, redirect_uri);

    manager.reconcile_upstream_oauth_managers(&GatewayConfig {
        upstream: vec![fixture_oauth_upstream(
            "new-oauth",
            "https://127.0.0.1:9/mcp",
        )],
        ..GatewayConfig::default()
    });

    assert!(managers.contains_key("new-oauth"));
    assert_eq!(
        managers
            .get("new-oauth")
            .expect("oauth manager")
            .upstream_config()
            .url
            .as_deref(),
        Some("https://127.0.0.1:9/mcp")
    );
}

#[tokio::test]
async fn reload_replaces_changed_upstream_oauth_manager_and_evicts_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        "changed-oauth".to_string(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            fixture_oauth_upstream("changed-oauth", "https://old.example.com/mcp"),
            redirect_uri.clone(),
        ),
    );
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(Arc::clone(&managers))
    .with_oauth_client_cache(cache.clone())
    .with_oauth_resources(sqlite, key, redirect_uri);

    assert_eq!(cache.len(), 0);
    manager.reconcile_upstream_oauth_managers(&GatewayConfig {
        upstream: vec![fixture_oauth_upstream(
            "changed-oauth",
            "https://new.example.com/mcp",
        )],
        ..GatewayConfig::default()
    });

    assert!(cache.is_empty());
    assert_eq!(
        managers
            .get("changed-oauth")
            .expect("oauth manager")
            .upstream_config()
            .url
            .as_deref(),
        Some("https://new.example.com/mcp")
    );
}

async fn reconcile_fixture(
    dir: &tempfile::TempDir,
) -> (
    GatewayManager,
    OauthClientCache,
    Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
) {
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(dir).await;
    let managers = Arc::new(dashmap::DashMap::new());
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(Arc::clone(&managers))
    .with_oauth_client_cache(cache.clone())
    .with_oauth_resources(sqlite, key, redirect_uri);
    (manager, cache, managers)
}

fn config_with(upstream: UpstreamConfig) -> GatewayConfig {
    GatewayConfig {
        upstream: vec![upstream],
        ..GatewayConfig::default()
    }
}

/// Connect-time scope binding must agree with reload reconciliation: an edit
/// that leaves the OAuth authority (name, url, oauth) unchanged keeps the
/// registered manager, so discovery for the edited config must not be refused.
#[tokio::test]
async fn reload_with_non_oauth_edit_keeps_manager_usable_for_connect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (manager, cache, _managers) = reconcile_fixture(&dir).await;
    let original = fixture_oauth_upstream("linear", "https://linear.example.com/mcp");
    manager.reconcile_upstream_oauth_managers(&config_with(original.clone()));
    cache
        .scope_challenge_handler(&original, "alice")
        .expect("registered config binds its scope handler");

    let mut edited = original.clone();
    edited.proxy_resources = !edited.proxy_resources;
    edited.expose_tools = Some(vec!["list_issues".into()]);
    edited.bearer_token_env = Some("LINEAR_TOKEN".into());
    edited.display_name = Some("Linear".into());
    manager.reconcile_upstream_oauth_managers(&config_with(edited.clone()));

    cache
        .scope_challenge_handler(&edited, "alice")
        .expect("a non-OAuth edit must not leave discovery needing reauthorization");
}

/// Removing an upstream and adding it back under the same name, url and OAuth
/// settings but different non-OAuth defaults must leave it discoverable, both
/// when the reload sees the removal and when it only sees the final config.
#[tokio::test]
async fn readded_oauth_upstream_with_different_defaults_is_usable_for_connect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (manager, cache, managers) = reconcile_fixture(&dir).await;
    let mut original = fixture_oauth_upstream("linear", "https://linear.example.com/mcp");
    original.enabled = false;
    original.proxy_prompts = true;
    manager.reconcile_upstream_oauth_managers(&config_with(original.clone()));

    let readded = fixture_oauth_upstream("linear", "https://linear.example.com/mcp");
    // Re-add observed only as the final config of a single reload.
    manager.reconcile_upstream_oauth_managers(&config_with(readded.clone()));
    cache
        .scope_challenge_handler(&readded, "alice")
        .expect("re-added upstream must not require reauthorization");

    // Re-add observed as a separate remove and add.
    manager.reconcile_upstream_oauth_managers(&GatewayConfig::default());
    assert!(!managers.contains_key("linear"));
    manager.reconcile_upstream_oauth_managers(&config_with(original.clone()));
    cache
        .scope_challenge_handler(&original, "alice")
        .expect("re-added upstream must not require reauthorization");
}

/// A real OAuth authority change still replaces the manager, and a connection
/// still holding the superseded config cannot bind to the replacement.
#[tokio::test]
async fn reload_with_oauth_authority_change_rejects_superseded_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (manager, cache, managers) = reconcile_fixture(&dir).await;
    let original = fixture_oauth_upstream("linear", "https://linear.example.com/mcp");

    let moved = fixture_oauth_upstream("linear", "https://linear-v2.example.com/mcp");
    let mut rescoped = original.clone();
    rescoped.oauth.as_mut().expect("oauth").scopes = Some(vec!["admin".into()]);

    for changed in [moved, rescoped] {
        manager.reconcile_upstream_oauth_managers(&config_with(original.clone()));
        manager.reconcile_upstream_oauth_managers(&config_with(changed.clone()));
        {
            let registered = managers.get("linear").expect("oauth manager");
            let registered = registered.upstream_config();
            assert_eq!(
                (&registered.url, &registered.oauth),
                (&changed.url, &changed.oauth),
                "an OAuth authority change must replace the manager"
            );
        }
        cache
            .scope_challenge_handler(&changed, "alice")
            .expect("the replacement manager serves the new authority");
        assert!(
            matches!(
                cache.scope_challenge_handler(&original, "alice"),
                Err(labby_auth::upstream::types::OauthError::NeedsReauth(_))
            ),
            "a superseded OAuth authority must not bind to the replacement manager"
        );
    }
}

#[tokio::test]
async fn cancelled_oauth_clear_finishes_client_and_status_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let upstream = fixture_oauth_upstream("cancelled-clear", "https://fixture.example.com/mcp");
    let subject = "clear-subject";
    sqlite
        .upsert_upstream_oauth_credentials(labby_auth::types::UpstreamOauthCredentialRow {
            upstream_name: upstream.name.clone(),
            subject: subject.into(),
            client_id: "fixture-client".into(),
            granted_scopes_json: "[]".into(),
            token_blob: vec![1],
            token_blob_nonce: vec![2],
            token_received_at: 1,
            access_token_expires_at: 2,
            refresh_token_present: false,
        })
        .await
        .unwrap();
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        upstream.name.clone(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            upstream.clone(),
            redirect_uri.clone(),
        ),
    );
    let cache = OauthClientCache::new(managers.clone());
    cache
        .publish_prebuilt(&upstream, subject, dummy_auth_client().await)
        .unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    )
    .with_upstream_oauth_managers(managers)
    .with_oauth_client_cache(cache.clone())
    .with_oauth_resources(sqlite.clone(), key, redirect_uri);
    let mut status_guard = manager.oauth_status_discovery_cache.lock().await;
    let cache_key = (upstream.name.clone(), subject.to_string());
    status_guard.insert(
        cache_key.clone(),
        super::super::OauthStatusDiscoverySnapshot {
            config_fingerprint: String::new(),
            lifecycle_epoch: None,
            pool_identity: None,
            observation: Default::default(),
            completed_at: tokio::time::Instant::now(),
            summary: None,
            tool_error: None,
            error: None,
        },
    );
    let clearing = manager.clone();
    let clear = tokio::spawn(async move {
        clearing
            .clear_upstream_credentials("cancelled-clear", "clear-subject")
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if sqlite
                .find_upstream_oauth_credentials("cancelled-clear", subject)
                .await
                .unwrap()
                .is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("clear committed identity deletion");
    clear.abort();
    assert!(clear.await.unwrap_err().is_cancelled());
    // Keep invalidation blocked until after caller cancellation, then release
    // the real lock. An owned clear must finish before its writer is released.
    drop(status_guard);
    let barrier = cache.invalidation_barrier();
    let settled = tokio::time::timeout(Duration::from_secs(5), barrier.read())
        .await
        .expect("clear lifecycle settles");
    assert!(
        !cache.contains_ready_client("cancelled-clear", subject),
        "durable delete must also evict live client after caller cancellation"
    );
    assert!(
        !manager
            .oauth_status_discovery_cache
            .lock()
            .await
            .contains_key(&cache_key)
    );
    drop(settled);
}

async fn google_revoke_invalidates_shared_clients_and_status(
    cancel_caller: bool,
    dedicated_request: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let (_, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let sqlite = SqliteStore::open_with_key(
        dir.path().join("google.sqlite"),
        Some(
            labby_auth::at_rest::TokenEncryptionKey::from_encoded(
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            )
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let upstream = fixture_oauth_upstream("cancelled-google", "https://fixture.example.com/mcp");
    let subject = "google-subject";
    sqlite
        .upsert_google_provider_token_bundle(labby_auth::types::GoogleProviderCredentialUpdate {
            subject: subject.into(),
            email: None,
            client_id: "fixture-client".into(),
            granted_scopes: vec!["openid".into()],
            access_token: "fixture-access".into(),
            refresh_token: "fixture-refresh".into(),
            token_received_at: 1,
            access_token_expires_at: i64::MAX,
            issuer: None,
            refreshed: false,
            scope_upgraded: false,
        })
        .await
        .unwrap();
    let mut upstream = upstream;
    upstream.oauth.as_mut().unwrap().credential =
        labby_runtime::gateway_config::UpstreamOauthCredentialSource::GoogleProvider {
            account: Some(subject.into()),
        };
    upstream.oauth.as_mut().unwrap().registration = UpstreamOauthRegistration::Preregistered {
        client_id: "fixture-client".into(),
        client_secret_env: None,
    };
    let mut peer = upstream.clone();
    peer.name = "google-peer".into();
    if dedicated_request {
        upstream.oauth.as_mut().unwrap().credential = Default::default();
    }
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        upstream.name.clone(),
        UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            upstream.clone(),
            redirect_uri.clone(),
        ),
    );
    let cache = OauthClientCache::new(managers.clone());
    cache
        .publish_prebuilt(&upstream, subject, dummy_auth_client().await)
        .unwrap();
    cache
        .publish_prebuilt(&peer, subject, dummy_auth_client().await)
        .unwrap();
    let (pool, retained_peers) = crate::upstream::pool::testsupport::retained_oauth_peers(
        &upstream.name,
        &peer.name,
        subject,
        cache.clone(),
    )
    .await;
    let runtime = GatewayRuntimeHandle::default();
    runtime.swap(Some(pool)).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime)
        .with_upstream_oauth_managers(managers)
        .with_oauth_client_cache(cache.clone())
        .with_oauth_resources(sqlite.clone(), key, redirect_uri);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream: vec![upstream.clone(), peer.clone()],
            ..GatewayConfig::default()
        })
        .await;
    let mut status_guard = manager.oauth_status_discovery_cache.lock().await;
    let cache_key = (upstream.name.clone(), subject.to_string());
    status_guard.insert(
        cache_key.clone(),
        super::super::OauthStatusDiscoverySnapshot {
            config_fingerprint: String::new(),
            lifecycle_epoch: None,
            pool_identity: None,
            observation: Default::default(),
            completed_at: tokio::time::Instant::now(),
            summary: None,
            tool_error: None,
            error: None,
        },
    );
    let peer_key = (peer.name.clone(), subject.to_string());
    status_guard.insert(
        peer_key.clone(),
        super::super::OauthStatusDiscoverySnapshot {
            config_fingerprint: String::new(),
            lifecycle_epoch: None,
            pool_identity: None,
            observation: Default::default(),
            completed_at: tokio::time::Instant::now(),
            summary: None,
            tool_error: None,
            error: None,
        },
    );
    let clearing = manager.clone();
    let clear = tokio::spawn(async move {
        clearing
            .revoke_google_provider_credential("cancelled-google")
            .await
    });
    if dedicated_request {
        drop(status_guard);
        let error = tokio::time::timeout(Duration::from_secs(5), clear)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), "oauth_shared_credential_protected");
        assert!(
            sqlite
                .find_google_provider_credential(subject)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            cache.contains_ready_client("cancelled-google", subject),
            "protected dedicated request must preserve its live client"
        );
        assert!(
            cache.contains_ready_client("google-peer", subject),
            "protected dedicated request must preserve unrelated shared Google live client"
        );
        let statuses = manager.oauth_status_discovery_cache.lock().await;
        assert!(
            statuses.contains_key(&cache_key),
            "dedicated status must survive admission rejection"
        );
        assert!(
            statuses.contains_key(&peer_key),
            "shared Google status must survive admission rejection"
        );
        assert!(
            retained_peers
                .iter()
                .all(|peer| !peer.is_transport_closed()),
            "source validation must not close existing generic/subject transports"
        );
        return;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if sqlite
                .find_google_provider_credential(subject)
                .await
                .unwrap()
                .is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("clear committed identity deletion");
    if cancel_caller {
        clear.abort();
        assert!(clear.await.unwrap_err().is_cancelled());
        drop(status_guard);
    } else {
        drop(status_guard);
        clear.await.unwrap().unwrap();
    }
    let barrier = cache.invalidation_barrier();
    let settled = tokio::time::timeout(Duration::from_secs(5), barrier.read())
        .await
        .expect("Google revoke lifecycle settles");
    assert!(
        !cache.contains_ready_client("cancelled-google", subject),
        "durable delete must also evict live client after caller cancellation"
    );
    assert!(
        !manager
            .oauth_status_discovery_cache
            .lock()
            .await
            .contains_key(&cache_key)
    );
    assert!(
        !cache.contains_ready_client("google-peer", subject),
        "shared provider revocation must evict the peer upstream's client"
    );
    assert!(
        !manager
            .oauth_status_discovery_cache
            .lock()
            .await
            .contains_key(&peer_key),
        "shared provider revocation must invalidate the peer's status snapshot"
    );
    drop(settled);
    tokio::time::timeout(Duration::from_secs(5), async {
        while retained_peers
            .iter()
            .any(|peer| !peer.is_transport_closed())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retained generic and subject transports close after the lifecycle writer releases");
}

#[tokio::test]
async fn cancelled_google_revoke_finishes_shared_client_and_status_invalidation() {
    google_revoke_invalidates_shared_clients_and_status(true, false).await;
}

#[tokio::test]
async fn successful_google_revoke_invalidates_peer_status_snapshot() {
    google_revoke_invalidates_shared_clients_and_status(false, false).await;
}

#[tokio::test]
async fn dedicated_google_revoke_rejection_preserves_all_clients_and_status() {
    google_revoke_invalidates_shared_clients_and_status(false, true).await;
}
