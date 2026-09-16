//! Recovery follows committed pool/config ownership across failed mutations.
use super::*;
use crate::gateway::config::{load_gateway_config, write_gateway_config};
use crate::gateway::params::GatewayUpdatePatch;
use serde_json::json;

fn recovery_config(name: &str) -> GatewayConfig {
    let mut cfg = GatewayConfig::default();
    cfg.gateway.auto_reconnect = true;
    cfg.code_mode.enabled = true;
    cfg.upstream.push(fixture_http_upstream(name));
    cfg
}

/// A held configuration-mutation lease is waited on for a bounded time only;
/// the waiter fails as `service_unavailable` and the lease stays usable once
/// the holder releases it.
#[tokio::test]
async fn config_mutation_wait_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let held = manager
        .acquire_config_mutation()
        .await
        .expect("first lease");
    let error = manager
        .acquire_config_mutation_within(Duration::from_millis(50))
        .await
        .err()
        .expect("a held lease must not be waited on indefinitely");
    assert_eq!(error.kind(), "service_unavailable");
    drop(held);
    assert!(
        manager
            .acquire_config_mutation_within(Duration::from_secs(5))
            .await
            .is_ok(),
        "lease is available again after the holder releases it"
    );
}

#[tokio::test]
async fn failed_full_reload_never_arms_private_recovery() {
    let name = "private-recovery-failure";
    UpstreamPool::reset_probe_task_schedule_count_for_tests(name);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    write_gateway_config(&path, &recovery_config(name)).unwrap();
    std::fs::create_dir(path.with_file_name("config.runtime.json")).unwrap();
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect_err("persistence must fail");
    assert!(manager.current_pool().await.is_none());
    assert_eq!(UpstreamPool::probe_task_schedule_count_for_tests(name), 0);
}

#[tokio::test]
async fn stale_lazy_request_cannot_reenable_published_recovery() {
    let name = "stale-recovery-request";
    UpstreamPool::reset_probe_task_schedule_count_for_tests(name);
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut cfg = recovery_config(name);
    manager.seed_config(cfg.clone()).await;
    let guard = manager.publication_barrier.write().await;
    let request = manager.ensure_search_runtime_ready(false, None, None);
    tokio::pin!(request);
    assert!(futures::poll!(&mut request).is_pending());
    cfg.gateway.auto_reconnect = false;
    *manager.config.write().await = cfg.clone();
    let pool = Arc::new(manager.new_base_pool(
        cfg.upstream_request_timeout(),
        cfg.upstream_relay_timeout(),
        false,
    ));
    pool.seed_lazy_upstreams(&cfg.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;
    drop(guard);
    request.await.unwrap();
    assert_eq!(UpstreamPool::probe_task_schedule_count_for_tests(name), 0);
    pool.drain_for_swap("test.stale_recovery").await;
}

#[tokio::test]
async fn selective_rollback_rearms_restored_recovery() {
    let name = "rollback-recovery";
    UpstreamPool::reset_probe_task_schedule_count_for_tests(name);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = recovery_config(name);
    write_gateway_config(&path, &cfg).unwrap();
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager.seed_config(cfg.clone()).await;
    let pool = Arc::new(manager.new_base_pool(
        cfg.upstream_request_timeout(),
        cfg.upstream_relay_timeout(),
        true,
    ));
    pool.seed_lazy_upstreams(&cfg.upstream).await;
    pool.ensure_recovery_tasks(&cfg.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;
    std::fs::create_dir(path.with_file_name("config.runtime.json")).unwrap();
    manager
        .update(
            name,
            GatewayUpdatePatch {
                url: Some(Some("http://127.0.0.1:9100/mcp".into())),
                ..Default::default()
            },
            None,
            Some("test"),
            None,
        )
        .await
        .expect_err("persistence must fail");
    assert_eq!(
        manager.current_config().await.upstream[0].url,
        cfg.upstream[0].url
    );
    assert_eq!(
        load_gateway_config(&path).unwrap().upstream[0].url,
        cfg.upstream[0].url
    );
    // Initial task plus restored task. No candidate task should be armed.
    assert_eq!(UpstreamPool::probe_task_schedule_count_for_tests(name), 2);
    // Reconciliation is idempotent only if rollback already restored its task.
    pool.ensure_recovery_tasks(&cfg.upstream).await;
    assert_eq!(UpstreamPool::probe_task_schedule_count_for_tests(name), 2);
    pool.drain_for_swap("test.rollback_recovery").await;
}

#[tokio::test]
async fn cleanup_rearms_committed_recovery() {
    let name = "cleanup-recovery";
    UpstreamPool::reset_probe_task_schedule_count_for_tests(name);
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let cfg = recovery_config(name);
    manager.seed_config(cfg.clone()).await;
    let pool = Arc::new(manager.new_base_pool(
        cfg.upstream_request_timeout(),
        cfg.upstream_relay_timeout(),
        true,
    ));
    pool.seed_lazy_upstreams(&cfg.upstream).await;
    pool.ensure_recovery_tasks(&cfg.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;
    // Exercise the real cleanup reconciliation without scanning host processes.
    manager
        .reconcile_after_upstream_cleanup(name, false)
        .await
        .unwrap();
    let scheduled = UpstreamPool::probe_task_schedule_count_for_tests(name);
    pool.drain_for_swap("test.cleanup_recovery").await;
    assert_eq!(
        scheduled, 2,
        "cleanup must restore configured recurring recovery"
    );
}

async fn background_http_recovery(after_cleanup: bool) {
    use std::sync::atomic::AtomicBool;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let name = if after_cleanup {
        "http-cleanup-recovery"
    } else {
        "http-periodic-recovery"
    };
    let available = Arc::new(AtomicBool::new(false));
    let responder_available = Arc::clone(&available);
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            if !responder_available.load(Ordering::SeqCst) {
                return ResponseTemplate::new(503);
            }
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let result = match body["method"].as_str().unwrap() {
                "server/discover" => json!({
                    "resultType": "complete", "supportedVersions": ["2026-07-28"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "recovery-fixture", "version": "1"},
                    "ttlMs": 0, "cacheScope": "private"
                }),
                "notifications/initialized" => return ResponseTemplate::new(202),
                "tools/list" => json!({"tools": [{"name": "recovered", "description": "recovered catalog", "inputSchema": {"type": "object"}}]}),
                _ => return ResponseTemplate::new(500),
            };
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}))
        }).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut cfg = recovery_config(name);
    cfg.upstream[0].url = Some(format!("{}/mcp", server.uri()));
    manager.seed_config(cfg.clone()).await;
    let pool = Arc::new(manager.new_base_pool(
        cfg.upstream_request_timeout(),
        cfg.upstream_relay_timeout(),
        true,
    ));
    pool.seed_lazy_upstreams(&cfg.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;
    pool.ensure_tools_for_upstream(&cfg.upstream[0], None, None)
        .await
        .expect_err("fixture starts unavailable");
    assert!(pool.healthy_tools_for_upstream(name).await.is_empty());
    pool.ensure_recovery_tasks(&cfg.upstream).await;
    if after_cleanup {
        manager
            .reconcile_after_upstream_cleanup(name, false)
            .await
            .unwrap();
    }
    available.store(true, Ordering::SeqCst);
    // Only inspect the cached catalog: no request or explicit reprobe may cause
    // the reconnect. The real scheduled heartbeat must restore the transport.
    let recovered = tokio::time::timeout(Duration::from_secs(50), async {
        loop {
            let tools = pool.healthy_tools_for_upstream(name).await;
            if !tools.is_empty() {
                break tools;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    pool.drain_for_swap("test.background_http_recovery").await;
    let tools =
        recovered.expect("periodic recovery must restore the catalog without a user request");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool.name, "recovered");
}

#[tokio::test]
async fn periodic_recovery_restores_http_catalog_without_a_request() {
    background_http_recovery(false).await;
}

#[tokio::test]
async fn cleanup_preserves_background_http_catalog_recovery() {
    background_http_recovery(true).await;
}

#[tokio::test]
async fn queued_cleanup_respects_committed_disabled_recovery() {
    let name = "cleanup-disabled-recovery";
    UpstreamPool::reset_probe_task_schedule_count_for_tests(name);
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut cfg = recovery_config(name);
    manager.seed_config(cfg.clone()).await;
    let guard = manager.acquire_config_mutation().await.unwrap();
    let cleanup = manager.reconcile_after_upstream_cleanup(name, false);
    tokio::pin!(cleanup);
    assert!(futures::poll!(&mut cleanup).is_pending());
    cfg.gateway.auto_reconnect = false;
    manager.seed_config(cfg.clone()).await;
    let pool = Arc::new(manager.new_base_pool(
        cfg.upstream_request_timeout(),
        cfg.upstream_relay_timeout(),
        false,
    ));
    pool.seed_lazy_upstreams(&cfg.upstream).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;
    drop(guard);
    cleanup.await.unwrap();
    assert_eq!(UpstreamPool::probe_task_schedule_count_for_tests(name), 0);
    pool.drain_for_swap("test.cleanup_disabled").await;
}

#[tokio::test]
async fn recurring_recovery_excludes_disabled_and_subject_oauth_upstreams() {
    let mut disabled = fixture_http_upstream("recovery-disabled-upstream");
    disabled.enabled = false;
    let oauth = fixture_oauth_upstream("recovery-subject-oauth", "http://127.0.0.1:9/mcp");
    for cfg in [&disabled, &oauth] {
        UpstreamPool::reset_probe_task_schedule_count_for_tests(&cfg.name);
    }
    let pool = UpstreamPool::new().with_auto_reconnect(true);
    pool.ensure_recovery_tasks(&[disabled.clone(), oauth.clone()])
        .await;
    for cfg in [&disabled, &oauth] {
        assert_eq!(
            UpstreamPool::probe_task_schedule_count_for_tests(&cfg.name),
            0
        );
    }
    pool.drain_for_swap("test.excluded_recovery").await;
}

#[tokio::test]
async fn committed_seed_arms_the_existing_startup_pool() {
    let name = "seeded-startup-recovery";
    UpstreamPool::reset_probe_task_schedule_count_for_tests(name);
    let dir = tempfile::tempdir().unwrap();
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    let pool = Arc::new(UpstreamPool::new());
    let cfg = recovery_config(name);
    pool.seed_lazy_upstreams(&cfg.upstream).await;
    runtime.swap(Some(Arc::clone(&pool))).await;
    manager.try_seed_config(cfg.clone()).await.unwrap();
    let scheduled = UpstreamPool::probe_task_schedule_count_for_tests(name);
    assert!(Arc::ptr_eq(&manager.current_pool().await.unwrap(), &pool));
    pool.drain_for_swap("test.startup_seed").await;
    assert_eq!(
        scheduled, 1,
        "committed startup policy must arm the published pool"
    );
}

#[tokio::test]
async fn invalid_seed_never_arms_the_existing_startup_pool() {
    let name = "invalid-startup-recovery";
    UpstreamPool::reset_probe_task_schedule_count_for_tests(name);
    let dir = tempfile::tempdir().unwrap();
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let mut cfg = recovery_config(name);
    cfg.upstream[0].command = Some("forbidden-command".to_string());
    manager
        .try_seed_config(cfg)
        .await
        .expect_err("invalid configuration must not publish");
    assert_eq!(UpstreamPool::probe_task_schedule_count_for_tests(name), 0);
    pool.drain_for_swap("test.invalid_startup_seed").await;
}
