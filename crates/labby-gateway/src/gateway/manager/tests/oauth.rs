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
