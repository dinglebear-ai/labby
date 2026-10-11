//! Capacity and credential retirement must respect invocation ownership.
use super::super::TaskRouteAuthorization;
use super::tests::{create_task_response, task_pool, task_pool_with_capabilities};
use rmcp::model::{CallToolResponse, ClientCapabilities};
use std::sync::Arc;

async fn connection(
    generation: usize,
) -> (
    super::super::relay_cache::RelayCachedConnection,
    rmcp::service::RunningService<rmcp::RoleServer, super::tests::DownstreamServer>,
) {
    let capabilities: ClientCapabilities = serde_json::from_value(serde_json::json!({
        "tasks": {}, "experimental": {"fixture-generation": {"ordinal": generation}}
    }))
    .unwrap();
    let (pool, _server, downstream, key) = task_pool_with_capabilities(capabilities).await;
    let connection = pool.relay_connections.write().await.remove(&key).unwrap();
    (connection, downstream)
}

#[tokio::test]
async fn task_generation_capacity_never_evicts_a_leased_peer() {
    let (pool, _server, downstream, key) = task_pool().await;
    let response = pool
        .register_task_response(
            &key,
            &"a".repeat(64),
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = response else {
        unreachable!()
    };
    let mut leases = Vec::new();
    let mut fixture_downstreams = Vec::new();
    let mut routes = pool.task_routes.write().await;
    let route = routes.get_mut(&created.task.task_id).unwrap();
    leases.push(Arc::clone(&route.companions[0].invocations));
    for generation in 1..super::companion::MAX_TASK_GENERATIONS {
        let (peer, fixture_downstream) = connection(generation).await;
        let (lease, retired) = route.install_companion(peer).unwrap();
        assert!(retired.is_none());
        leases.push(lease);
        fixture_downstreams.push(fixture_downstream);
    }
    let peers = route
        .companions
        .iter()
        .map(|companion| companion.connection.peer.clone())
        .collect::<Vec<_>>();
    let (extra, fixture_downstream) = connection(100).await;
    assert!(
        route
            .install_companion(extra)
            .err()
            .expect("busy generations reject admission")
            .contains("capacity")
    );
    fixture_downstreams.push(fixture_downstream);
    assert_eq!(
        route.companions.len(),
        super::companion::MAX_TASK_GENERATIONS
    );
    assert!(peers.iter().all(|peer| !peer.is_transport_closed()));
    // Only the released generation becomes eligible for bounded replacement.
    leases.remove(0);
    let (extra, fixture_downstream) = connection(101).await;
    let (_lease, retired) = route.install_companion(extra).unwrap();
    let retired = retired.expect("released generation is retired");
    assert_eq!(
        route.companions.len(),
        super::companion::MAX_TASK_GENERATIONS
    );
    assert!(peers.iter().skip(1).all(|peer| !peer.is_transport_closed()));
    drop(routes);
    retired._connection.shutdown(&key.0, "test.capacity").await;
    assert!(peers[0].is_transport_closed());
    pool.close_task_companions("test.complete").await;
    downstream.cancel().await.unwrap();
    fixture_downstreams.push(fixture_downstream);
    for fixture in fixture_downstreams {
        fixture.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn credential_revocation_closes_every_task_capability_generation() {
    let (pool, _server, downstream, key) = task_pool().await;
    let initial = pool.relay_connections.write().await.remove(&key).unwrap();
    let oauth_key = (
        key.0.clone(),
        key.1,
        Some("credential-subject".to_string()),
        key.3.clone(),
    );
    pool.relay_connections
        .write()
        .await
        .insert(oauth_key.clone(), initial);
    let response = pool
        .register_task_response(
            &oauth_key,
            &"a".repeat(64),
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = response else {
        unreachable!()
    };
    let (additional, additional_downstream) = connection(1).await;
    let mut routes = pool.task_routes.write().await;
    let route = routes.get_mut(&created.task.task_id).unwrap();
    let (_lease, retired) = route.install_companion(additional).unwrap();
    assert!(retired.is_none());
    let peers = route
        .companions
        .iter()
        .map(|companion| companion.connection.peer.clone())
        .collect::<Vec<_>>();
    drop(routes);
    assert_eq!(
        pool.invalidate_task_routes_for_oauth_subject(
            &key.0,
            "credential-subject",
            "oauth.credentials.clear"
        )
        .await
        .unwrap(),
        1
    );
    assert!(pool.task_routes.read().await.is_empty());
    assert!(peers.iter().all(|peer| peer.is_transport_closed()));
    assert!(
        pool.task_route_store
            .as_ref()
            .unwrap()
            .get(&created.task.task_id)
            .await
            .unwrap()
            .is_none()
    );
    downstream.cancel().await.unwrap();
    additional_downstream.cancel().await.unwrap();
}
