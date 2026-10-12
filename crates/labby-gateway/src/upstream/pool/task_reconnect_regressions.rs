//! Behavioral regressions for generation ownership and local task admission.

use rmcp::model::{
    CallToolResponse, CancelTaskParams, ClientCapabilities, GetTaskParams, InputResponses,
    UpdateTaskParams,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::super::relay_cache::capability_fingerprint;
use super::super::{TaskCallContext, TaskRouteAuthorization, UpstreamPool};
use super::tests::{create_task_response, task_pool, task_pool_with_capabilities};

async fn register(pool: &UpstreamPool, key: &super::super::relay_cache::RelayCacheKey) -> String {
    let fingerprint = pool
        .upstream_config_fingerprints
        .get(&key.0)
        .unwrap()
        .clone();
    let response = pool
        .register_task_response(
            key,
            &fingerprint,
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = response else {
        unreachable!()
    };
    created.task.task_id
}

fn install_definition(pool: &UpstreamPool, url: String) {
    let mut config = super::super::testsupport::test_upstream_config();
    config.name = "task-upstream".into();
    config.url = Some(url);
    pool.upstream_config_fingerprints.insert(
        config.name.clone(),
        crate::gateway::code_mode::catalog_cache::fingerprint(&config),
    );
    pool.configured_upstreams
        .insert(config.name.clone(), config);
}

#[tokio::test]
async fn capability_generation_preserves_inflight_task_mutations() {
    for operation in ["update", "cancel"] {
        let (pool, first, downstream, key) = task_pool().await;
        install_definition(&pool, "http://127.0.0.1:1/mcp".into());
        let id = register(&pool, &key).await;
        let capabilities = ClientCapabilities::builder()
            .enable_tasks()
            .enable_elicitation()
            .build();
        let (replacement_pool, second, replacement_downstream, replacement_key) =
            task_pool_with_capabilities(capabilities.clone()).await;
        let connection = replacement_pool
            .relay_connections
            .write()
            .await
            .remove(&replacement_key)
            .unwrap();
        let second_peer = connection.peer.clone();
        pool.relay_connections.write().await.insert(
            (
                key.0.clone(),
                0,
                None,
                format!("task:{id}:{}", capability_fingerprint(&capabilities)),
            ),
            connection,
        );
        first.hold_mutations.store(true, Ordering::SeqCst);
        let pool = Arc::new(pool);
        let mutating = Arc::clone(&pool);
        let mutation_id = id.clone();
        let mutation_downstream = downstream.peer().clone();
        let mutation = tokio::spawn(async move {
            let auth = TaskRouteAuthorization::root();
            if operation == "update" {
                mutating
                    .update_task_routed(
                        UpdateTaskParams::new(&mutation_id, InputResponses::new()),
                        Some("alice"),
                        &auth,
                        &mutation_id,
                        mutation_downstream,
                    )
                    .await
            } else {
                mutating
                    .cancel_task_routed(
                        CancelTaskParams::new(&mutation_id),
                        Some("alice"),
                        &auth,
                        &mutation_id,
                        mutation_downstream,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), first.mutation_entered.notified())
            .await
            .unwrap();
        let authorization = TaskRouteAuthorization::root();
        let poll = pool.get_task_routed_with_context(
            GetTaskParams::new(&id),
            Some("alice"),
            &authorization,
            downstream.peer().clone(),
            TaskCallContext {
                capabilities,
                ..Default::default()
            },
        );
        tokio::time::timeout(Duration::from_secs(5), poll)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.get_calls.load(Ordering::SeqCst), 1);
        assert_eq!(pool.task_routes.read().await[&id].companions.len(), 2);
        assert!(
            !mutation.is_finished(),
            "the first upstream mutation remains gated"
        );
        first.mutation_release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), mutation)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            first.updates.lock().await.len(),
            usize::from(operation == "update")
        );
        assert_eq!(
            first.cancellations.lock().await.len(),
            usize::from(operation == "cancel")
        );
        let peers = pool.task_routes.read().await[&id]
            .companions
            .iter()
            .map(|companion| companion.connection.peer.clone())
            .collect::<Vec<_>>();
        assert!(peers.iter().all(|peer| !peer.is_transport_closed()));
        // Definition retirement must close every retained generation.
        let names = std::iter::once(key.0.clone()).collect();
        let guards = pool.prepare_lazy_upstream_reconcile(&names).await;
        let cleanup = pool
            .apply_lazy_upstream_reconcile(&[], &names, &guards)
            .await;
        drop(guards);
        cleanup.finish("test.config.revoke").await;
        assert!(pool.task_routes.read().await.is_empty());
        assert!(peers.iter().all(|peer| peer.is_transport_closed()));
        assert!(second_peer.is_transport_closed());
        assert!(
            pool.task_route_store
                .as_ref()
                .unwrap()
                .get(&id)
                .await
                .unwrap()
                .is_none()
        );
        downstream.cancel().await.unwrap();
        replacement_downstream.cancel().await.unwrap();
    }
}

#[derive(Clone)]
struct StalledDiscovery {
    entered: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}
impl Respond for StalledDiscovery {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["method"], "server/discover");
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        let result = rmcp::model::DiscoverResult::new(
            vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .enable_tasks()
                .build(),
        );
        ResponseTemplate::new(200)
            .set_delay(Duration::from_mins(1))
            .set_body_json(serde_json::json!({
                "jsonrpc":"2.0", "id":body["id"], "result": result
            }))
    }
}

#[tokio::test]
async fn cold_task_handshake_does_not_block_warm_task_rpcs_and_is_single_flight() {
    let http = MockServer::start().await;
    let responder = StalledDiscovery {
        entered: Arc::new(tokio::sync::Notify::new()),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(responder.clone())
        .mount(&http)
        .await;
    let (pool, server, downstream, key) = task_pool().await;
    install_definition(&pool, format!("{}/mcp", http.uri()));
    let warm = register(&pool, &key).await;
    let mut cold = pool
        .task_route_store
        .as_ref()
        .unwrap()
        .get(&warm)
        .await
        .unwrap()
        .unwrap();
    cold.public_task_id = super::mint_task_handle();
    let cold_id = cold.public_task_id.clone();
    pool.task_route_store
        .as_ref()
        .unwrap()
        .insert(cold)
        .await
        .unwrap();
    let pool = Arc::new(pool);
    let context = TaskCallContext::default();
    let cold_call = |pool: Arc<UpstreamPool>| {
        let id = cold_id.clone();
        let context = context.clone();
        let peer = downstream.peer().clone();
        tokio::spawn(async move {
            pool.get_task_routed_with_context(
                GetTaskParams::new(id),
                Some("alice"),
                &TaskRouteAuthorization::root(),
                peer,
                context,
            )
            .await
        })
    };
    let first = cold_call(Arc::clone(&pool));
    tokio::time::timeout(Duration::from_secs(5), responder.entered.notified())
        .await
        .unwrap();
    let second = cold_call(Arc::clone(&pool));
    let warm_calls = async {
        let auth = TaskRouteAuthorization::root();
        pool.get_task_routed(
            GetTaskParams::new(&warm),
            Some("alice"),
            &auth,
            downstream.peer().clone(),
        )
        .await
        .unwrap();
        pool.update_task_routed(
            UpdateTaskParams::new(&warm, InputResponses::new()),
            Some("alice"),
            &auth,
            &warm,
            downstream.peer().clone(),
        )
        .await
        .unwrap();
        pool.cancel_task_routed(
            CancelTaskParams::new(&warm),
            Some("alice"),
            &auth,
            &warm,
            downstream.peer().clone(),
        )
        .await
        .unwrap();
    };
    tokio::time::timeout(Duration::from_secs(5), warm_calls)
        .await
        .expect("warm calls must dispatch before cold handshake settles");
    assert_eq!(server.get_calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.updates.lock().await.len(), 1);
    assert_eq!(server.cancellations.lock().await.len(), 1);
    assert_eq!(responder.calls.load(Ordering::SeqCst), 1);
    context.cancellation.cancel();
    for call in [first, second] {
        assert!(
            tokio::time::timeout(Duration::from_secs(5), call)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .contains("cancelled")
        );
    }
    pool.close_task_companions("test.complete").await;
    downstream.cancel().await.unwrap();
}

#[tokio::test]
async fn task_definition_queue_timeout_does_not_poison_upstream_health() {
    let (mut pool, server, downstream, key) = task_pool().await;
    let id = register(&pool, &key).await;
    pool.request_timeout = Duration::from_millis(50);
    pool.catalog_write().await.insert(
        key.0.clone(),
        super::super::entries::healthy_in_process_entry(
            Arc::from(key.0.as_str()),
            Default::default(),
        ),
    );
    let gate = pool.lazy_connect_lock(&key.0).await;
    let holder = gate.lock().await;
    let error = pool
        .get_task_routed(
            GetTaskParams::new(&id),
            Some("alice"),
            &TaskRouteAuthorization::root(),
            downstream.peer().clone(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("queue timed out"), "{error}");
    assert_eq!(server.get_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        pool.upstream_capability_health(
            &key.0,
            super::super::super::types::UpstreamCapability::Tools
        )
        .await,
        Some(super::super::super::types::UpstreamHealth::Healthy)
    );
    drop(holder);
    pool.close_task_companions("test.complete").await;
    downstream.cancel().await.unwrap();
}
