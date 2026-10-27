//! Manager rollback must complete the same durable intent it prepared.
use super::*;
use crate::gateway::{
    config_store::{GatewayConfigStore, StoreFuture},
    manager::{GatewayManager, GatewayRuntimeHandle},
};
use labby_runtime::{
    error::ToolError,
    gateway_config::{GatewayConfig, ResolvedPublicUrls},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

struct RollbackStore {
    config: PathBuf,
    routes: PathBuf,
    fail_next: AtomicBool,
    fail_cleanup: bool,
}

impl GatewayConfigStore for RollbackStore {
    fn public_urls(&self) -> ResolvedPublicUrls {
        ResolvedPublicUrls::default()
    }
    fn set_process_code_mode_enabled(&self, _: bool) {}
    fn env_path(&self) -> PathBuf {
        self.config.with_file_name(".env")
    }
    fn persist(&self, config: &GatewayConfig) -> Result<(), ToolError> {
        crate::gateway::config::write_gateway_config(&self.config, config)?;
        if self.fail_next.swap(false, Ordering::SeqCst) {
            if self.fail_cleanup {
                rusqlite::Connection::open(&self.routes).unwrap().execute_batch(
                    "CREATE TRIGGER fail_route_cleanup BEFORE DELETE ON task_routes BEGIN SELECT RAISE(FAIL, 'injected cleanup failure'); END;"
                ).unwrap();
            }
            std::fs::write(&self.config, "invalid toml = [").unwrap();
        }
        Ok(())
    }
    fn persist_gateway_bearer_token<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        Box::pin(async { Ok(()) })
    }
    fn persist_service_env<'a>(
        &'a self,
        _: &'a str,
        _: &'a std::collections::BTreeMap<String, String>,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        Box::pin(async { Ok(()) })
    }
}

async fn rollback_case(fail_cleanup: bool) {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    let route_path = dir.path().join("routes.db");
    let routes = Arc::new(TaskRouteStore::open(route_path.clone()).await.unwrap());
    let mut affected = record();
    affected.ttl_ms = None;
    affected.upstream_name = "affected".into();
    let mut unaffected = affected.clone();
    unaffected.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    unaffected.upstream_name = "unaffected".into();
    routes.insert(affected.clone()).await.unwrap();
    routes.insert(unaffected.clone()).await.unwrap();
    let mut initial = GatewayConfig::default();
    for name in ["affected", "unaffected"] {
        let mut upstream = crate::upstream::pool::testsupport::test_upstream_config();
        upstream.name = name.into();
        upstream.enabled = false;
        upstream.command = Some("npx".into());
        initial.upstream.push(upstream);
    }
    crate::gateway::config::write_gateway_config(&config_path, &initial).unwrap();
    let store = Arc::new(RollbackStore {
        config: config_path.clone(),
        routes: route_path.clone(),
        fail_next: AtomicBool::new(false),
        fail_cleanup,
    });
    let manager = GatewayManager::with_store(
        config_path.clone(),
        GatewayRuntimeHandle::default(),
        store.clone(),
    )
    .with_task_route_store(routes.clone());
    manager.seed_config_unchecked_for_tests(initial).await;
    manager.reload_with_origin(None, None).await.unwrap();
    store.fail_next.store(true, Ordering::SeqCst);
    let error = manager
        .update(
            "affected",
            serde_json::from_value(serde_json::json!({"args":["changed"]})).unwrap(),
            None,
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(
        crate::gateway::config::load_gateway_config(&config_path)
            .unwrap()
            .upstream[0]
            .args
            .is_empty()
    );
    let db = rusqlite::Connection::open(&route_path).unwrap();
    let pending: i64 = db
        .query_row("SELECT COUNT(*) FROM task_route_revocations", [], |row| {
            row.get(0)
        })
        .unwrap();
    if fail_cleanup {
        assert!(
            error
                .to_string()
                .contains("rollback task revocation failed"),
            "{error}"
        );
        assert!(pending > 0);
        assert!(
            routes
                .get(&unaffected.public_task_id)
                .await
                .unwrap_err()
                .contains("quarantined")
        );
        return;
    }
    assert_eq!(
        pending, 0,
        "successful rollback must durably finish prepared intent"
    );
    assert!(
        routes
            .get(&affected.public_task_id)
            .await
            .unwrap()
            .is_none(),
        "affected tasks remain conservatively revoked"
    );
    assert!(
        routes
            .get(&unaffected.public_task_id)
            .await
            .unwrap()
            .is_some(),
        "unaffected acknowledged tasks remain routable"
    );
    let mut fresh = unaffected.clone();
    fresh.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    routes.insert(fresh.clone()).await.unwrap();
    manager
        .update(
            "affected",
            serde_json::from_value(serde_json::json!({"args":["later"]})).unwrap(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let reopened = TaskRouteStore::open(route_path).await.unwrap();
    assert!(
        reopened
            .get(&affected.public_task_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        reopened
            .get(&unaffected.public_task_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(reopened.get(&fresh.public_task_id).await.unwrap().is_some());
}

#[tokio::test]
async fn successful_manager_rollback_completes_prepared_tasks() {
    rollback_case(false).await;
}

#[tokio::test]
async fn failed_manager_rollback_cleanup_preserves_quarantine() {
    rollback_case(true).await;
}
