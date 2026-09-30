//! Lavra regressions for durable task acknowledgement and authorization.

use std::sync::Arc;

use rmcp::model::{
    CallToolResponse, CancelTaskParams, GetTaskParams, InputResponses, UpdateTaskParams,
};

use super::super::{TaskRouteAuthorization, TaskRouteStore};
use super::tests::{create_task_response, task_pool};

#[tokio::test]
async fn unusable_authorization_snapshot_is_not_acknowledged() {
    let (pool, _server, _downstream, key) = task_pool().await;
    let authorization =
        TaskRouteAuthorization::new("protected:other", Some(["other".to_string()].into()));
    let result = pool
        .register_task_response(
            &key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            authorization,
            create_task_response(),
        )
        .await;
    assert!(
        result.is_err(),
        "a task the owner cannot resolve must not be acknowledged"
    );
    assert!(pool.task_routes.read().await.is_empty());
}

#[tokio::test]
async fn changed_configuration_rejects_every_task_operation() {
    let (pool, server, downstream, key) = task_pool().await;
    let auth = TaskRouteAuthorization::root();
    let result = pool
        .register_task_response(
            &key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            auth.clone(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = result else {
        unreachable!()
    };
    let id = created.task.task_id;
    pool.upstream_config_fingerprints
        .insert(key.0.clone(), "changed-config".to_string());
    let get = pool
        .get_task_routed(
            GetTaskParams::new(&id),
            Some("alice"),
            &auth,
            downstream.peer().clone(),
        )
        .await;
    let update = pool
        .update_task_routed(
            UpdateTaskParams::new(&id, InputResponses::new()),
            Some("alice"),
            &auth,
            &id,
            downstream.peer().clone(),
        )
        .await;
    let cancel = pool
        .cancel_task_routed(
            CancelTaskParams::new(&id),
            Some("alice"),
            &auth,
            &id,
            downstream.peer().clone(),
        )
        .await;
    assert_eq!(get.unwrap_err(), super::task_not_found());
    assert_eq!(update.unwrap_err(), super::task_not_found());
    assert_eq!(cancel.unwrap_err(), super::task_not_found());
    assert!(server.updates.lock().await.is_empty());
    assert!(server.cancellations.lock().await.is_empty());
}

#[tokio::test]
async fn removed_durable_route_cannot_be_updated_or_cancelled_through_live_cache() {
    let (mut pool, server, downstream, key) = task_pool().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path.clone()).await.unwrap()));
    let auth = TaskRouteAuthorization::root();
    let result = pool
        .register_task_response(
            &key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            auth.clone(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = result else {
        unreachable!()
    };
    let id = created.task.task_id;
    rusqlite::Connection::open(path)
        .unwrap()
        .execute("DELETE FROM task_routes WHERE public_task_id = ?1", [&id])
        .unwrap();
    let update = pool
        .update_task_routed(
            UpdateTaskParams::new(&id, InputResponses::new()),
            Some("alice"),
            &auth,
            &id,
            downstream.peer().clone(),
        )
        .await;
    let cancel = pool
        .cancel_task_routed(
            CancelTaskParams::new(&id),
            Some("alice"),
            &auth,
            &id,
            downstream.peer().clone(),
        )
        .await;
    assert_eq!(update.unwrap_err(), super::task_not_found());
    assert_eq!(cancel.unwrap_err(), super::task_not_found());
    assert!(server.updates.lock().await.is_empty());
    assert!(server.cancellations.lock().await.is_empty());
}

#[tokio::test]
async fn ttl_origin_is_the_upstream_creation_time_not_registration_time() {
    let (pool, _server, _downstream, key) = task_pool().await;
    let result = pool
        .register_task_response(
            &key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = result else {
        unreachable!()
    };
    let record = pool
        .task_route_store
        .as_ref()
        .unwrap()
        .get(&created.task.task_id)
        .await
        .unwrap()
        .unwrap();
    let created_at: jiff::Timestamp = created.task.created_at.parse().unwrap();
    assert_eq!(record.created_at_unix_ms, created_at.as_millisecond());
}

#[tokio::test]
async fn expired_task_is_not_acknowledged() {
    let (pool, _server, _downstream, key) = task_pool().await;
    let CallToolResponse::Task(mut response) = create_task_response() else {
        unreachable!()
    };
    response.task.ttl_ms = Some(0);
    let result = pool
        .register_task_response(
            &key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            TaskRouteAuthorization::root(),
            CallToolResponse::Task(response),
        )
        .await;
    assert!(
        result.is_err(),
        "an already expired task is not immediately resolvable"
    );
}

#[tokio::test]
async fn real_sqlite_write_failure_prevents_ack_and_allows_retry() {
    let (mut pool, _server, _downstream, key) = task_pool().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path.clone()).await.unwrap()));
    let database = rusqlite::Connection::open(path).unwrap();
    database.execute_batch("CREATE TRIGGER reject_route BEFORE INSERT ON task_routes BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
    let result = pool
        .register_task_response(
            &key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        database
            .query_row("SELECT COUNT(*) FROM task_routes", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(pool.task_routes.read().await.is_empty());
    assert!(pool.relay_connections.read().await.contains_key(&key));
    database.execute_batch("DROP TRIGGER reject_route").unwrap();
    pool.register_task_response(
        &key,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        Some("alice"),
        TaskRouteAuthorization::root(),
        create_task_response(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn reopened_store_resolves_immediate_get_and_preserves_owner_binding() {
    let (mut pool, _server, downstream, key) = task_pool().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path.clone()).await.unwrap()));
    let auth = TaskRouteAuthorization::root();
    let result = pool
        .register_task_response(
            &key,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some("alice"),
            auth.clone(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = result else {
        unreachable!()
    };
    let id = created.task.task_id;
    pool.task_route_store = None;
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path).await.unwrap()));
    let owned = pool
        .get_task_routed(
            GetTaskParams::new(&id),
            Some("alice"),
            &auth,
            downstream.peer().clone(),
        )
        .await
        .unwrap();
    assert_eq!(owned.task.task.task_id, id);
    assert_eq!(
        pool.get_task_routed(
            GetTaskParams::new(&id),
            Some("bob"),
            &auth,
            downstream.peer().clone()
        )
        .await
        .unwrap_err(),
        super::task_not_found()
    );
    assert_eq!(
        pool.update_task_routed(
            UpdateTaskParams::new(&id, InputResponses::new()),
            Some("bob"),
            &auth,
            &id,
            downstream.peer().clone()
        )
        .await
        .unwrap_err(),
        super::task_not_found()
    );
    assert_eq!(
        pool.cancel_task_routed(
            CancelTaskParams::new(&id),
            Some("bob"),
            &auth,
            &id,
            downstream.peer().clone()
        )
        .await
        .unwrap_err(),
        super::task_not_found()
    );
}

#[test]
fn opaque_handles_have_random_uuid_v4_shape_and_do_not_collide() {
    let mut ids = std::collections::HashSet::new();
    for _ in 0..8192 {
        let id = super::mint_task_handle();
        let suffix = id.strip_prefix("labby-task-").unwrap();
        assert_eq!(suffix.len(), 32);
        let parsed = uuid::Uuid::parse_str(suffix).unwrap();
        assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
        assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
        assert!(ids.insert(id));
    }
}
