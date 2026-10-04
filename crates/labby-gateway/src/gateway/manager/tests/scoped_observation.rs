//! OAuth status discovery and operator inspection must share one credential catalog.
use super::*;
use crate::gateway::params::GatewayEnrichmentScope;
use crate::gateway::view_models::CapabilityObservationState as State;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool},
    service::RequestContext,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone)]
struct ObservedCatalog {
    count: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    gate: Arc<tokio::sync::Notify>,
    blocked: Arc<AtomicBool>,
}
impl ServerHandler for ObservedCatalog {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if self.blocked.load(Ordering::SeqCst) {
            self.gate.notified().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(ErrorData::internal_error(
                "bounded mock tool discovery failed",
                None,
            ));
        }
        let tools = (0..self.count.load(Ordering::SeqCst))
            .map(|i| {
                Tool::new(
                    format!("tool_{i}"),
                    "test tool",
                    Arc::new(serde_json::Map::new()),
                )
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }
}
fn catalog() -> ObservedCatalog {
    ObservedCatalog {
        count: Arc::new(AtomicUsize::new(91)),
        requests: Arc::new(AtomicUsize::new(0)),
        fail: Arc::new(AtomicBool::new(false)),
        gate: Arc::new(tokio::sync::Notify::new()),
        blocked: Arc::new(AtomicBool::new(false)),
    }
}

#[tokio::test]
async fn oauth_status_observation_uses_runtime_catalog_across_all_passive_views() {
    let config = fixture_oauth_upstream("alpha", "https://fixture.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(config.clone()).await;
    let server = catalog();
    pool.install_test_subject_server_for_upstream(&config, "alice", server.clone())
        .await;
    let snapshot = manager
        .oauth_status_discovery("alpha", "alice", config.clone())
        .await;
    assert_eq!(snapshot.observation.tools.state, State::Known);
    assert_eq!(snapshot.observation.tools.discovered, Some(91));
    let calls = server.requests.load(Ordering::SeqCst);
    let scope = GatewayEnrichmentScope {
        oauth_subject: Some("alice".into()),
        ..Default::default()
    };
    let list = manager.list_scoped(&scope).await.unwrap();
    let single = manager.get_server_scoped("alpha", &scope).await.unwrap();
    let detail = manager.get_scoped("alpha", &scope).await.unwrap();
    let runtime = manager.status_scoped(Some("alpha"), &scope).await.unwrap();
    assert_eq!(
        list[0].capability_observation,
        single.capability_observation
    );
    assert_eq!(
        single.capability_observation,
        detail.runtime.capability_observation
    );
    assert_eq!(
        detail.runtime.capability_observation,
        runtime[0].capability_observation
    );
    assert_eq!(
        manager
            .discovered_tools_scoped("alpha", &scope)
            .await
            .unwrap()
            .len(),
        91
    );
    assert_eq!(
        server.requests.load(Ordering::SeqCst),
        calls,
        "passive inspection must perform no RPCs"
    );
    let other = GatewayEnrichmentScope {
        oauth_subject: Some("bob".into()),
        ..Default::default()
    };
    assert_eq!(
        manager
            .get_server_scoped("alpha", &other)
            .await
            .unwrap()
            .capability_observation
            .unwrap()
            .tools
            .state,
        State::Unknown
    );
    assert!(
        manager
            .discovered_tools_scoped("alpha", &other)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        pool.cached_upstream_summary("alpha")
            .await
            .unwrap()
            .discovered_tool_count,
        0,
        "credential tools never enter shared catalog"
    );
    server.count.store(92, Ordering::SeqCst);
    manager
        .oauth_status_discovery_cache
        .lock()
        .await
        .get_mut(&("alpha".into(), "alice".into()))
        .unwrap()
        .completed_at -= Duration::from_secs(31);
    assert_eq!(
        manager
            .oauth_status_discovery("alpha", "alice", config.clone())
            .await
            .observation
            .tools
            .discovered,
        Some(92)
    );
    server.fail.store(true, Ordering::SeqCst);
    manager.oauth_status_discovery_cache.lock().await.clear();
    let failed = manager
        .oauth_status_discovery("alpha", "alice", config.clone())
        .await;
    assert_eq!(failed.observation.tools.state, State::Failed);
    assert_eq!(
        failed.observation.tools.discovered,
        Some(92),
        "keep last successful measurement on failure"
    );
    assert_eq!(
        manager
            .get_server_scoped("alpha", &scope)
            .await
            .unwrap()
            .capability_observation
            .unwrap()
            .tools
            .state,
        State::Failed
    );
    server.fail.store(false, Ordering::SeqCst);
    pool.reprobe_tools_for_upstream_as(&config, Some("alice"), None)
        .await
        .unwrap();
    let recovered = manager
        .oauth_status_discovery("alpha", "alice", config.clone())
        .await;
    assert_eq!(recovered.observation.tools.state, State::Known);
    assert!(
        recovered.tool_error.is_none(),
        "status cooldown must not retain a failure after explicit refresh succeeds"
    );
    assert!(
        manager
            .oauth_status_discovery_cache
            .lock()
            .await
            .get(&("alpha".into(), "alice".into()))
            .unwrap()
            .tool_error
            .is_none(),
        "recovery must restore the normal discovery freshness window"
    );
}

#[tokio::test]
async fn oauth_discovery_does_not_publish_after_configuration_replacement() {
    let config = fixture_oauth_upstream("alpha", "https://fixture.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(config.clone()).await;
    let server = catalog();
    pool.install_test_subject_server_for_upstream(&config, "alice", server.clone())
        .await;
    server.blocked.store(true, Ordering::SeqCst);
    let discovery = {
        let manager = manager.clone();
        let config = config.clone();
        tokio::spawn(async move {
            manager
                .oauth_status_discovery("alpha", "alice", config.clone())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut replacement = config.clone();
    replacement.url = Some("https://replacement.invalid/mcp".into());
    pool.register_upstream_config_for_tests(&replacement);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            upstream: vec![replacement],
            ..Default::default()
        })
        .await;
    server.gate.notify_one();
    let snapshot = discovery.await.unwrap();
    assert_eq!(snapshot.observation.tools.state, State::Unknown);
    assert!(manager.oauth_status_discovery_cache.lock().await.is_empty());
    assert_eq!(
        pool.cached_subject_summary(&config, Some("alice"))
            .await
            .observation()
            .tools
            .discovered,
        None
    );
}

#[tokio::test]
async fn oauth_discovery_does_not_publish_after_credential_epoch_changes() {
    let config = fixture_oauth_upstream("alpha", "https://fixture.invalid/mcp");
    let (manager, _) = code_mode_manager_with_pool(config.clone()).await;
    let cache =
        labby_auth::upstream::cache::OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
    let pool = Arc::new(UpstreamPool::new().with_oauth_client_cache(cache.clone()));
    manager.runtime.swap(Some(pool.clone())).await;
    let manager = manager.with_oauth_client_cache(cache.clone());
    let server = catalog();
    pool.install_test_subject_server_for_upstream(&config, "alice", server.clone())
        .await;
    server.blocked.store(true, Ordering::SeqCst);
    let discovery = {
        let manager = manager.clone();
        let config = config.clone();
        tokio::spawn(async move {
            manager
                .oauth_status_discovery("alpha", "alice", config.clone())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.requests.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Advancing the lifecycle models credential replacement while preserving
    // the old peer so the late list reply actually reaches the publication fence.
    {
        let _writer = cache.invalidation_barrier().write_owned().await;
        cache.advance_subject_epoch("alpha", "alice");
    }
    server.gate.notify_one();
    let snapshot = discovery.await.unwrap();
    assert_eq!(snapshot.observation.tools.state, State::Unknown);
    assert!(manager.oauth_status_discovery_cache.lock().await.is_empty());
    assert_eq!(
        pool.cached_subject_summary(&config, Some("alice"))
            .await
            .summary
            .discovered_tool_count,
        0,
        "late pre-replacement tools must not publish"
    );
}

#[tokio::test]
async fn oauth_authentication_remains_valid_when_capability_discovery_fails() {
    let config = fixture_oauth_upstream("alpha", "https://fixture.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(config.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let managers = Arc::new(dashmap::DashMap::new());
    managers.insert(
        "alpha".into(),
        labby_auth::upstream::manager::UpstreamOauthManager::new(
            sqlite.clone(),
            key.clone(),
            config.clone(),
            redirect_uri.clone(),
        ),
    );
    let manager = manager
        .with_upstream_oauth_managers(managers)
        .with_oauth_resources(sqlite.clone(), key, redirect_uri);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlite
        .upsert_upstream_oauth_credentials(labby_auth::types::UpstreamOauthCredentialRow {
            upstream_name: "alpha".into(),
            subject: "alice".into(),
            client_id: "fixture-client".into(),
            granted_scopes_json: "[]".into(),
            token_blob: vec![1],
            token_blob_nonce: vec![0; 12],
            token_received_at: now,
            access_token_expires_at: now + 3600,
            refresh_token_present: false,
        })
        .await
        .unwrap();
    let server = catalog();
    server.fail.store(true, Ordering::SeqCst);
    pool.install_test_subject_server_for_upstream(&config, "alice", server)
        .await;
    let status = manager
        .upstream_oauth_status("alpha", "alice")
        .await
        .unwrap();
    assert!(
        status.authenticated,
        "valid persisted credentials remain authenticated despite discovery failure"
    );
    assert_eq!(
        status.state,
        crate::gateway::oauth::UpstreamOauthConnectionState::DiscoveryFailed
    );
    assert_eq!(
        status.capability_observation.unwrap().tools.state,
        State::Failed
    );
}

#[path = "scoped_observation_timeout.rs"]
mod timeout;
