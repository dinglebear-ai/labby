//! Recovery follows committed pool/config ownership across failed mutations.
use super::*;
use crate::gateway::config::{load_gateway_config, write_gateway_config};
use crate::gateway::params::GatewayUpdatePatch;

fn recovery_config(name: &str) -> GatewayConfig {
    let mut cfg = GatewayConfig::default();
    cfg.gateway.auto_reconnect = true;
    cfg.code_mode.enabled = true;
    cfg.upstream.push(fixture_http_upstream(name));
    cfg
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
