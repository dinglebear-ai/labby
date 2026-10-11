//! Exercise manual reload and restart against durable task metadata and HTTP peers.
use super::super::super::tasks::tests::{create_task_response, task_pool};
use super::super::super::{TaskRouteAuthorization, TaskRouteStore};
use crate::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
use labby_runtime::gateway_config::GatewayConfig;
use rmcp::model::{
    CallToolResponse, CancelTaskParams, ClientCapabilities, DetailedTask, DiscoverResult,
    GetTaskParams, GetTaskResult, InputResponses, ListToolsResult, ProtocolVersion,
    ServerCapabilities, Task, TaskAckResult, TaskPayload, TaskStatus, UpdateTaskParams,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Clone, Default)]
struct HttpTasks {
    requests: Arc<AtomicUsize>,
}
impl Respond for HttpTasks {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let result = match body["method"].as_str().unwrap() {
            "server/discover" => serde_json::to_value(DiscoverResult::new(
                vec![ProtocolVersion::V_2026_07_28],
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tasks()
                    .build(),
            ))
            .unwrap(),
            "tools/list" => {
                serde_json::to_value(ListToolsResult::with_all_items(Vec::new())).unwrap()
            }
            "tasks/get" => {
                self.requests.fetch_add(1, Ordering::SeqCst);
                serde_json::to_value(GetTaskResult::new(DetailedTask::new(
                    Task::new(
                        body["params"]["taskId"].as_str().unwrap(),
                        TaskStatus::Working,
                        "2026-07-31T00:00:00Z",
                        "2026-07-31T00:00:00Z",
                    ),
                    TaskPayload::Working,
                )))
                .unwrap()
            }
            "tasks/update" | "tasks/cancel" => {
                self.requests.fetch_add(1, Ordering::SeqCst);
                serde_json::to_value(TaskAckResult::new()).unwrap()
            }
            other => panic!("unexpected fixture request {other}"),
        };
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }
}

#[tokio::test]
async fn manual_reload_permanently_revokes_changed_and_removed_tasks_but_preserves_restart_recovery()
 {
    for change in ["remove", "replace"] {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let route_path = dir.path().join("routes.db");
        let http = MockServer::start().await;
        let responder = HttpTasks::default();
        Mock::given(wiremock::matchers::method("POST"))
            .respond_with(responder.clone())
            .mount(&http)
            .await;
        let mut config = GatewayConfig::default();
        for name in ["affected", "unaffected"] {
            let mut upstream = super::super::super::testsupport::test_upstream_config();
            upstream.name = name.into();
            upstream.url = Some(format!("{}/mcp", http.uri()));
            config.upstream.push(upstream);
        }
        crate::gateway::config::write_gateway_config(&config_path, &config).unwrap();
        let routes = Arc::new(TaskRouteStore::open(route_path.clone()).await.unwrap());
        let manager = GatewayManager::new(config_path.clone(), GatewayRuntimeHandle::default())
            .with_task_route_store(Arc::clone(&routes));
        manager
            .seed_config_unchecked_for_tests(config.clone())
            .await;
        manager.reload_with_origin(None, None).await.unwrap();
        let (fixture_pool, _server, downstream, _key) = task_pool().await;
        let pool = manager.current_pool_sync().unwrap();
        let capabilities = ClientCapabilities::builder().enable_tasks().build();
        let mut ids = Vec::new();
        for upstream in &config.upstream {
            let key = (
                upstream.name.clone(),
                7,
                None,
                super::super::super::relay_cache::capability_fingerprint(&capabilities),
            );
            pool.acquire_relay_for_key(
                upstream,
                None,
                downstream.peer().clone(),
                capabilities.clone(),
                key.clone(),
            )
            .await
            .unwrap();
            let response = pool
                .register_task_response(
                    &key,
                    &crate::gateway::code_mode::catalog_cache::fingerprint(upstream),
                    Some("alice"),
                    TaskRouteAuthorization::root(),
                    create_task_response(),
                )
                .await
                .unwrap();
            let CallToolResponse::Task(created) = response else {
                unreachable!()
            };
            ids.push(created.task.task_id);
        }
        // Force a full replacement with unchanged upstream definitions, then
        // prove the retained handle actually reconnects and reaches HTTP.
        let mut same_definitions = config.clone();
        same_definitions.upstream_request_timeout_ms = Some(30_100);
        crate::gateway::config::write_gateway_config(&config_path, &same_definitions).unwrap();
        manager.reload_with_origin(None, None).await.unwrap();
        manager
            .current_pool_sync()
            .unwrap()
            .get_task_routed(
                GetTaskParams::new(&ids[0]),
                Some("alice"),
                &TaskRouteAuthorization::root(),
                downstream.peer().clone(),
            )
            .await
            .unwrap();
        let mut changed = same_definitions.clone();
        if change == "remove" {
            changed.upstream.remove(0);
        } else {
            changed.upstream[0]
                .headers
                .insert("X-Fixture-Revision".into(), "replacement".into());
        }
        crate::gateway::config::write_gateway_config(&config_path, &changed).unwrap();
        manager.reload_with_origin(None, None).await.unwrap();
        assert!(routes.get(&ids[0]).await.unwrap().is_none());
        assert!(routes.get(&ids[1]).await.unwrap().is_some());
        crate::gateway::config::write_gateway_config(&config_path, &same_definitions).unwrap();
        manager.reload_with_origin(None, None).await.unwrap();
        let before = responder.requests.load(Ordering::SeqCst);
        let current = manager.current_pool_sync().unwrap();
        let auth = TaskRouteAuthorization::root();
        assert_eq!(
            current
                .get_task_routed(
                    GetTaskParams::new(&ids[0]),
                    Some("alice"),
                    &auth,
                    downstream.peer().clone()
                )
                .await
                .unwrap_err(),
            "task not found"
        );
        assert_eq!(
            current
                .update_task_routed(
                    UpdateTaskParams::new(&ids[0], InputResponses::new()),
                    Some("alice"),
                    &auth,
                    &ids[0],
                    downstream.peer().clone()
                )
                .await
                .unwrap_err(),
            "task not found"
        );
        assert_eq!(
            current
                .cancel_task_routed(
                    CancelTaskParams::new(&ids[0]),
                    Some("alice"),
                    &auth,
                    &ids[0],
                    downstream.peer().clone()
                )
                .await
                .unwrap_err(),
            "task not found"
        );
        assert_eq!(
            responder.requests.load(Ordering::SeqCst),
            before,
            "retired ID must never dispatch"
        );
        let reopened = Arc::new(TaskRouteStore::open(route_path).await.unwrap());
        assert!(reopened.get(&ids[0]).await.unwrap().is_none());
        let restarted = GatewayManager::new(config_path, GatewayRuntimeHandle::default())
            .with_task_route_store(reopened);
        restarted
            .seed_config_unchecked_for_tests(same_definitions)
            .await;
        restarted.reload_with_origin(None, None).await.unwrap();
        let recovered = restarted.current_pool_sync().unwrap();
        recovered
            .get_task_routed(
                GetTaskParams::new(&ids[1]),
                Some("alice"),
                &auth,
                downstream.peer().clone(),
            )
            .await
            .unwrap();
        assert_eq!(responder.requests.load(Ordering::SeqCst), before + 1);
        recovered.drain_for_swap("test.complete").await;
        current.drain_for_swap("test.complete").await;
        pool.drain_for_swap("test.complete").await;
        fixture_pool.drain_for_swap("test.complete").await;
        downstream.cancel().await.unwrap();
    }
}
