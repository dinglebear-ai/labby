//! The shared dispatcher must fit inside API, CLI and MCP caller futures.
use super::*;
use crate::gateway::manager::GatewayRuntimeHandle;
use std::mem::size_of_val;

#[tokio::test]
async fn shared_dispatch_keeps_its_caller_future_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let scope = GatewayEnrichmentScope::default();
    let gateway = handle_gateway_actions(
        &manager,
        "gateway.test",
        serde_json::json!({}),
        scope.clone(),
    );
    let oauth = handle_oauth_actions(
        &manager,
        "gateway.oauth.status",
        serde_json::json!({}),
        scope.clone(),
    );
    let mcp = handle_mcp_actions(
        &manager,
        "gateway.mcp.toggle",
        serde_json::json!({}),
        scope.clone(),
    );
    let shared =
        dispatch_with_manager_scoped(&manager, "gateway.test", serde_json::json!({}), scope);
    eprintln!(
        "future bytes: shared={}, gateway={}, oauth={}, mcp={}",
        size_of_val(&shared),
        size_of_val(&gateway),
        size_of_val(&oauth),
        size_of_val(&mcp)
    );
    assert!(
        size_of_val(&gateway) < 4_096,
        "the gateway family must not embed every manager operation in its caller",
    );
    assert!(
        size_of_val(&shared) < 4_096,
        "the shared dispatcher must not embed every action-family state machine in callers"
    );
}

#[tokio::test]
async fn scoped_dispatch_executes_ephemeral_probe_under_caller_stack_budget() {
    let dir = tempfile::tempdir().unwrap();
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    // Match the API/CLI regression: a proposed stdio spec completes with a
    // failed transport observation rather than killing the serving worker.
    let result = dispatch_with_manager_scoped(
        &manager,
        "gateway.test",
        serde_json::json!({
            "spec": { "name": "fixture-stdio", "command": "echo", "args": ["hello"] }
        }),
        GatewayEnrichmentScope::default(),
    )
    .await
    .unwrap();
    assert_eq!(result["name"], "fixture-stdio");
    assert_eq!(result["connected"], false);
    assert!(result["capability_observation"].is_object());
}

#[test]
fn routed_probe_has_stack_headroom_for_serving_middleware() {
    // Leave 384 KiB of a 1 MiB serving stack for API and middleware callers.
    std::thread::Builder::new()
        .stack_size(640 * 1024)
        .spawn(scoped_dispatch_executes_ephemeral_probe_under_caller_stack_budget)
        .unwrap()
        .join()
        .unwrap();
}
