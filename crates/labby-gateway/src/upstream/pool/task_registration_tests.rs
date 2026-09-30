//! Concurrency regressions for durable-before-publication and rollback.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{CallToolResponse, GetTaskParams};

use super::super::{TaskRouteAuthorization, TaskRouteStore};
use super::tests::{create_task_response, task_pool};

const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[tokio::test]
async fn public_translation_waits_for_live_route_readiness() {
    let (mut pool, _server, downstream, key) = task_pool().await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("routes.db");
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path.clone()).await.unwrap()));
    let database = rusqlite::Connection::open(path).unwrap();
    let relay_routes = Arc::clone(
        &pool
            .relay_connections
            .read()
            .await
            .get(&key)
            .unwrap()
            .routes,
    );
    let pool = Arc::new(pool);
    let live_map_guard = pool.task_routes.write().await;
    let registering = Arc::clone(&pool);
    let task = tokio::spawn(async move {
        registering
            .register_task_response(
                &key,
                FINGERPRINT,
                Some("alice"),
                TaskRouteAuthorization::root(),
                create_task_response(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let count: i64 = database
                .query_row("SELECT COUNT(*) FROM task_routes", [], |row| row.get(0))
                .unwrap();
            if count == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("registration commits before live publication");
    let exposed = tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            if let Some(id) = relay_routes.gateway_task_id("native-task-1").await {
                break id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        exposed.is_err(),
        "no notification may expose an ID before its live route is ready"
    );
    assert!(
        !task.is_finished(),
        "acknowledgement waits for the live route"
    );
    drop(live_map_guard);
    let result = task.await.unwrap().unwrap();
    let CallToolResponse::Task(created) = result else {
        unreachable!()
    };
    pool.get_task_routed(
        GetTaskParams::new(&created.task.task_id),
        Some("alice"),
        &TaskRouteAuthorization::root(),
        downstream.peer().clone(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_commit_does_not_overwrite_a_replacement_relay() {
    let (mut pool, _server, _downstream, key) = task_pool().await;
    let (replacement_pool, _replacement_server, _replacement_downstream, replacement_key) =
        task_pool().await;
    let replacement = replacement_pool
        .relay_connections
        .write()
        .await
        .remove(&replacement_key)
        .unwrap();
    let replacement_routes = Arc::clone(&replacement.routes);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("routes.db");
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path.clone()).await.unwrap()));
    let database = rusqlite::Connection::open(path).unwrap();
    database.execute_batch("CREATE TRIGGER reject_route BEFORE INSERT ON task_routes BEGIN SELECT RAISE(ABORT, 'failure'); END; BEGIN IMMEDIATE;").unwrap();
    let pool = Arc::new(pool);
    let registering = Arc::clone(&pool);
    let registering_key = key.clone();
    let task = tokio::spawn(async move {
        registering
            .register_task_response(
                &registering_key,
                FINGERPRINT,
                Some("alice"),
                TaskRouteAuthorization::root(),
                create_task_response(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while pool.relay_connections.read().await.contains_key(&key) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("registration owns the old relay");
    pool.relay_connections
        .write()
        .await
        .insert(key.clone(), replacement);
    database.execute_batch("COMMIT").unwrap();
    assert!(task.await.unwrap().is_err());
    let cache = pool.relay_connections.read().await;
    assert!(Arc::ptr_eq(
        &cache.get(&key).unwrap().routes,
        &replacement_routes
    ));
    assert!(pool.task_routes.read().await.is_empty());
}

#[tokio::test]
async fn missing_store_rejects_tasks_but_preserves_synchronous_results() {
    let (mut pool, _server, _downstream, key) = task_pool().await;
    pool.task_route_store = None;
    assert!(
        pool.register_task_response(
            &key,
            FINGERPRINT,
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response()
        )
        .await
        .is_err()
    );
    let complete = CallToolResponse::Complete(rmcp::model::CallToolResult::success(vec![]));
    let result = pool
        .register_task_response(
            &key,
            FINGERPRINT,
            Some("alice"),
            TaskRouteAuthorization::root(),
            complete,
        )
        .await
        .unwrap();
    assert!(matches!(result, CallToolResponse::Complete(_)));
    assert!(pool.relay_connections.read().await.contains_key(&key));
}

#[tokio::test]
async fn hints_write_failure_is_not_returned_as_successful_poll() {
    let (mut pool, _server, downstream, key) = task_pool().await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("routes.db");
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path.clone()).await.unwrap()));
    let result = pool
        .register_task_response(
            &key,
            FINGERPRINT,
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = result else {
        unreachable!()
    };
    let database = rusqlite::Connection::open(path).unwrap();
    database.execute_batch("CREATE TRIGGER reject_hint BEFORE UPDATE ON task_routes BEGIN SELECT RAISE(ABORT, 'failure'); END;").unwrap();
    assert_eq!(
        pool.get_task_routed(
            GetTaskParams::new(&created.task.task_id),
            Some("alice"),
            &TaskRouteAuthorization::root(),
            downstream.peer().clone()
        )
        .await
        .unwrap_err(),
        "task routing unavailable"
    );
}

#[tokio::test]
async fn unchanged_poll_does_not_issue_a_sqlite_update() {
    let (mut pool, _server, downstream, key) = task_pool().await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("routes.db");
    pool.task_route_store = Some(Arc::new(TaskRouteStore::open(path.clone()).await.unwrap()));
    let result = pool
        .register_task_response(
            &key,
            FINGERPRINT,
            Some("alice"),
            TaskRouteAuthorization::root(),
            create_task_response(),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(created) = result else {
        unreachable!()
    };
    let id = created.task.task_id;
    pool.get_task_routed(
        GetTaskParams::new(&id),
        Some("alice"),
        &TaskRouteAuthorization::root(),
        downstream.peer().clone(),
    )
    .await
    .unwrap();
    let database = rusqlite::Connection::open(path).unwrap();
    database.execute_batch("CREATE TRIGGER reject_hint BEFORE UPDATE ON task_routes BEGIN SELECT RAISE(ABORT, 'unexpected write'); END;").unwrap();
    pool.get_task_routed(
        GetTaskParams::new(&id),
        Some("alice"),
        &TaskRouteAuthorization::root(),
        downstream.peer().clone(),
    )
    .await
    .unwrap();
}
