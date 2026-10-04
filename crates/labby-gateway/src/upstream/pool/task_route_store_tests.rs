//! Durable store regression tests.

use std::collections::BTreeSet;

use super::{TaskRouteRecord, TaskRouteStore};
use crate::upstream::pool::TaskRouteAuthorization;

fn record() -> TaskRouteRecord {
    TaskRouteRecord {
        public_task_id: format!("labby-task-{}", uuid::Uuid::new_v4().simple()),
        native_task_id: "native-42".to_string(),
        upstream_name: "example".to_string(),
        caller_subject: Some("alice".to_string()),
        oauth_subject: Some("oauth-alice".to_string()),
        authorization: TaskRouteAuthorization::new(
            "protected:test",
            Some(BTreeSet::from(["example".to_string()])),
        ),
        config_fingerprint: "a".repeat(64),
        created_at_unix_ms: 100,
        updated_at_unix_ms: 101,
        ttl_ms: Some(60_000),
        poll_interval_ms: Some(250),
    }
}

#[tokio::test]
async fn future_schema_rejection_does_not_create_or_modify_tables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE task_route_meta(schema_version INTEGER NOT NULL); INSERT INTO task_route_meta VALUES(999);").unwrap();
    assert!(TaskRouteStore::open(path).await.is_err());
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'task_routes'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "future-schema rejection must happen before schema mutation"
    );
}

#[tokio::test]
async fn malformed_current_schema_is_rejected_at_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("malformed.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA user_version=1; CREATE TABLE task_route_meta(schema_version INTEGER NOT NULL); INSERT INTO task_route_meta VALUES(1); CREATE TABLE task_routes(public_task_id TEXT PRIMARY KEY, upstream_name TEXT, oauth_subject TEXT);").unwrap();
    assert!(
        TaskRouteStore::open(path).await.is_err(),
        "current version does not excuse missing required columns"
    );
}

#[tokio::test]
async fn route_survives_store_reopen() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("task-routes.db");
    let expected = record();

    {
        let store = TaskRouteStore::open(path.clone())
            .await
            .expect("store opens");
        store
            .insert(expected.clone())
            .await
            .expect("route persists");
    }

    let reopened = TaskRouteStore::open(path.clone())
        .await
        .expect("store reopens");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path)
                .expect("task route database metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        reopened
            .get(&expected.public_task_id)
            .await
            .expect("route lookup succeeds"),
        Some(expected)
    );
}

#[tokio::test]
async fn concurrent_first_open_has_one_schema_version_row() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("concurrent.db");
    let (a, b) = tokio::join!(
        TaskRouteStore::open(path.clone()),
        TaskRouteStore::open(path.clone())
    );
    let (_a, _b) = (a.unwrap(), b.unwrap());
    let db = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM task_route_meta", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn relative_and_memory_paths_are_not_durable_stores() {
    assert!(TaskRouteStore::open(":memory:".into()).await.is_err());
    assert!(
        TaskRouteStore::open("relative-task-routes.db".into())
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn database_leaf_symlink_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target.db");
    let link = directory.path().join("link.db");
    let _store = TaskRouteStore::open(target.clone()).await.unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(TaskRouteStore::open(link).await.is_err());
}

#[tokio::test]
async fn owner_quota_rejects_without_evicting_acknowledged_routes() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    let mut first = record();
    first.ttl_ms = None;
    store.insert(first.clone()).await.unwrap();
    for _ in 1..super::MAX_OWNER_ROUTES {
        let mut next = first.clone();
        next.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
        store.insert(next).await.unwrap();
    }
    let mut overflow = first.clone();
    overflow.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    assert!(store.insert(overflow.clone()).await.is_err());
    assert!(store.get(&first.public_task_id).await.unwrap().is_some());
    overflow.caller_subject = Some("bob".to_string());
    store.insert(overflow).await.unwrap();
}

#[tokio::test]
async fn operation_admission_is_bounded_and_recovers() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    let permits = store
        .admission
        .clone()
        .acquire_many_owned(super::MAX_PENDING_OPERATIONS as u32)
        .await
        .unwrap();
    assert_eq!(
        store.get(&record().public_task_id).await.unwrap_err(),
        "task route store busy"
    );
    drop(permits);
    assert!(store.get(&record().public_task_id).await.unwrap().is_none());
}

#[tokio::test]
async fn corrupted_row_does_not_reveal_existence_to_another_owner() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    let route = record();
    store.insert(route.clone()).await.unwrap();
    store
        .with_connection(|connection| {
            connection
                .execute("UPDATE task_routes SET config_fingerprint = 'invalid'", [])
                .map_err(super::sqlite_error)?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .get_for_caller(&route.public_task_id, Some("bob"), &route.authorization)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_for_caller(&route.public_task_id, Some("alice"), &route.authorization)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn overflowed_ttl_and_older_observations_fail_closed() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    let mut route = record();
    route.ttl_ms = Some(u64::MAX);
    assert!(store.insert(route.clone()).await.is_err());
    route.ttl_ms = None;
    route.updated_at_unix_ms = 500;
    store.insert(route.clone()).await.unwrap();
    let mut older = route.clone();
    older.updated_at_unix_ms = 400;
    assert!(store.update_hints(older).await.is_err());
    assert_eq!(store.get(&route.public_task_id).await.unwrap(), Some(route));
}

#[tokio::test]
async fn global_quota_rejects_without_deleting_existing_routes() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    store.with_connection(|connection| {
        connection.execute("WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n < ?1)
            INSERT INTO task_routes (public_task_id, native_task_id, upstream_name, caller_subject,
                oauth_subject, route_key, allowed_upstreams_json, config_fingerprint, created_at_unix_ms, updated_at_unix_ms, ttl_ms, poll_interval_ms)
            SELECT printf('labby-task-%08x000040008000000000000000', n), 'native', 'example', printf('owner-%d', n),
                NULL, 'root', NULL, ?2, 0, 0, NULL, NULL FROM ids",
            rusqlite::params![super::MAX_ROUTES, "a".repeat(64)]).map_err(super::sqlite_error)?;
        Ok(())
    }).await.unwrap();
    let mut route = record();
    route.ttl_ms = None;
    assert!(store.insert(route).await.is_err());
    let count = store
        .with_connection(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM task_routes", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(super::sqlite_error)
        })
        .await
        .unwrap();
    assert_eq!(count, super::MAX_ROUTES);
}

#[tokio::test]
async fn persisted_fingerprint_does_not_persist_configuration_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let secret = "fixture-only-secret-do-not-persist";
    let config: labby_runtime::gateway_config::UpstreamConfig =
        serde_json::from_value(serde_json::json!({
            "name": "example", "url": "https://example.invalid/mcp",
            "headers": { "X-Api-Key": secret }, "env": { "TOKEN": secret }
        }))
        .unwrap();
    let mut route = record();
    route.config_fingerprint = crate::gateway::code_mode::catalog_cache::fingerprint(&config);
    store.insert(route).await.unwrap();
    for file in [
        path.clone(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
    ] {
        if file.exists() {
            let bytes = std::fs::read(file).unwrap();
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
            );
        }
    }
}

#[tokio::test]
async fn failed_revocation_quarantines_reads_and_later_writes() {
    let store = TaskRouteStore::open_in_memory().await.expect("store");
    let mut route = record();
    route.ttl_ms = None;
    store.insert(route.clone()).await.expect("route");
    store.with_connection(|connection| {
        connection.execute_batch("CREATE TRIGGER deny_revocation BEFORE DELETE ON task_routes BEGIN SELECT RAISE(ABORT, 'revocation denied'); END;")
            .map_err(super::sqlite_error)
    }).await.expect("fault injection");
    assert!(
        store
            .remove_oauth_subject("example", "oauth-alice")
            .await
            .is_err()
    );
    assert!(
        store
            .get_for_caller(&route.public_task_id, Some("alice"), &route.authorization)
            .await
            .is_err()
    );
    assert!(store.insert(record()).await.is_err());
}

#[tokio::test]
async fn failed_file_revocation_remains_denied_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let mut route = record();
    route.ttl_ms = None;
    store.insert(route.clone()).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER deny_revocation BEFORE DELETE ON task_routes BEGIN SELECT RAISE(ABORT, 'denied'); END;").unwrap();
    assert!(
        store
            .remove_oauth_subject("example", "oauth-alice")
            .await
            .is_err()
    );
    drop(store);
    assert!(
        TaskRouteStore::open(path.clone()).await.is_err(),
        "pending revocation must prevent reopening while deletion fails"
    );
    db.execute_batch("DROP TRIGGER deny_revocation").unwrap();
    let recovered = TaskRouteStore::open(path).await.unwrap();
    assert!(
        recovered
            .get(&route.public_task_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn cancelled_revocation_worker_retains_durable_intent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let mut route = record();
    route.ttl_ms = None;
    store.insert(route).await.unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER deny_revocation BEFORE DELETE ON task_routes BEGIN SELECT RAISE(ABORT, 'denied'); END; BEGIN IMMEDIATE;").unwrap();
    let worker_store = store.clone();
    let request = tokio::spawn(async move {
        worker_store
            .remove_oauth_subject("example", "oauth-alice")
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while store.connection.try_lock().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    db.execute_batch("COMMIT").unwrap();
    // Acquiring the connection waits for the uncancelled SQLite worker to finish.
    drop(store.connection.lock().await);
    drop(store);
    assert!(
        TaskRouteStore::open(path).await.is_err(),
        "caller cancellation must not forget the pending revocation"
    );
}

#[tokio::test]
async fn version_one_migration_preserves_routes_and_installs_revocation_intents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let route = record();
    store.insert(route.clone()).await.unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP TABLE task_route_revocations; UPDATE task_route_meta SET schema_version = 1;",
    )
    .unwrap();
    drop(db);
    let upgraded = TaskRouteStore::open(path).await.unwrap();
    assert_eq!(
        upgraded.get(&route.public_task_id).await.unwrap(),
        Some(route)
    );
    assert_eq!(
        upgraded
            .remove_oauth_subject("example", "oauth-alice")
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn queued_lookup_cannot_bypass_failed_revocation_quarantine() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    let mut route = record();
    route.ttl_ms = None;
    store.insert(route.clone()).await.unwrap();
    store.with_connection(|db| db.execute_batch("CREATE TRIGGER deny_revocation BEFORE DELETE ON task_routes BEGIN SELECT RAISE(ABORT, 'denied'); END;").map_err(super::sqlite_error)).await.unwrap();
    let held = store.connection.lock().await;
    let revoking = store.clone();
    let revoke = tokio::spawn(async move {
        revoking
            .remove_oauth_subject("example", "oauth-alice")
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while store.admission.available_permits() != super::MAX_PENDING_OPERATIONS - 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let reading = store.clone();
    let read = tokio::spawn(async move { reading.get(&route.public_task_id).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while store.admission.available_permits() != super::MAX_PENDING_OPERATIONS - 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(held);
    assert!(revoke.await.unwrap().is_err());
    assert!(read.await.unwrap().is_err());
}

#[tokio::test]
async fn prepared_config_revocation_fences_registration_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let route = record();
    store.insert(route.clone()).await.unwrap();
    store
        .prepare_upstream_revocation(vec!["example".into()])
        .await
        .unwrap();
    assert!(store.insert(record()).await.is_err());
    drop(store);
    let reopened = TaskRouteStore::open(path).await.unwrap();
    assert!(reopened.get(&route.public_task_id).await.unwrap().is_none());
}

#[tokio::test]
async fn unrelated_oauth_revocation_cannot_consume_prepared_config_fence() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    let mut old = record();
    old.upstream_name = "config-a".into();
    old.ttl_ms = None;
    store.insert(old.clone()).await.unwrap();
    store
        .prepare_upstream_revocation(vec!["config-a".into()])
        .await
        .unwrap();
    store
        .remove_oauth_subject("oauth-b", "oauth-alice")
        .await
        .unwrap();
    let pending: i64 = store
        .with_connection_unchecked(|db| {
            db.query_row(
                "SELECT COUNT(*) FROM task_route_revocations WHERE prepared = 1",
                [],
                |row| row.get(0),
            )
            .map_err(super::sqlite_error)
        })
        .await
        .unwrap();
    assert_eq!(
        pending, 1,
        "ordinary revocation cannot consume prepared intent"
    );
    let mut late = old.clone();
    late.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    assert!(
        store.insert(late).await.is_err(),
        "unrelated OAuth deletion must retain the config registration fence"
    );
    store.complete_prepared_revocations().await.unwrap();
    assert!(store.get(&old.public_task_id).await.unwrap().is_none());
    let remaining: i64 = store
        .with_connection(|db| {
            db.query_row("SELECT COUNT(*) FROM task_routes", [], |row| row.get(0))
                .map_err(super::sqlite_error)
        })
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn version_two_pending_intent_is_replayed_during_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let route = record();
    store.insert(route.clone()).await.unwrap();
    store
        .prepare_upstream_revocation(vec!["example".into()])
        .await
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("ALTER TABLE task_route_revocations DROP COLUMN prepared; UPDATE task_route_meta SET schema_version = 2;").unwrap();
    drop(db);
    let upgraded = TaskRouteStore::open(path).await.unwrap();
    assert!(upgraded.get(&route.public_task_id).await.unwrap().is_none());
}

#[tokio::test]
async fn routed_task_resolution_obeys_cancellation_and_deadline_while_sqlite_is_blocked() {
    use super::super::TaskCallContext;
    use super::super::tasks::tests::{create_task_response, task_pool};
    use rmcp::model::{
        CallToolResponse, CancelTaskParams, GetTaskParams, InputResponses, UpdateTaskParams,
    };

    for operation in ["get", "update", "cancel"] {
        for cancel in [false, true] {
            let (mut pool, server, downstream, key) = task_pool().await;
            pool.request_timeout = std::time::Duration::from_millis(60);
            let authorization = TaskRouteAuthorization::root();
            let response = pool
                .register_task_response(
                    &key,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    Some("alice"),
                    authorization.clone(),
                    create_task_response(),
                )
                .await
                .unwrap();
            let CallToolResponse::Task(created) = response else {
                unreachable!()
            };
            let id = created.task.task_id;
            let store = pool.task_route_store.as_ref().unwrap();
            let held = store.connection.lock().await;
            let context = TaskCallContext::default();
            let call = async {
                match operation {
                    "get" => pool
                        .get_task_routed_with_context(
                            GetTaskParams::new(&id),
                            Some("alice"),
                            &authorization,
                            downstream.peer().clone(),
                            context.clone(),
                        )
                        .await
                        .map(|_| ()),
                    "update" => {
                        pool.update_task_routed_with_context(
                            UpdateTaskParams::new(&id, InputResponses::new()),
                            Some("alice"),
                            &authorization,
                            &id,
                            downstream.peer().clone(),
                            context.clone(),
                        )
                        .await
                    }
                    _ => {
                        pool.cancel_task_routed_with_context(
                            CancelTaskParams::new(&id),
                            Some("alice"),
                            &authorization,
                            &id,
                            downstream.peer().clone(),
                            context.clone(),
                        )
                        .await
                    }
                }
            };
            let cancellation = async {
                if cancel {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    context.cancellation.cancel();
                }
            };
            let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                tokio::join!(call, cancellation)
            })
            .await
            .expect("blocked durable lookup must respect the caller budget");
            assert_eq!(
                result.unwrap_err(),
                if cancel {
                    "task request cancelled"
                } else {
                    "task request completion timed out"
                },
                "{operation}, cancel={cancel}"
            );
            assert_eq!(
                server.get_calls.load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            assert!(server.updates.lock().await.is_empty());
            assert!(server.cancellations.lock().await.is_empty());
            drop(held);
            // Drain the uncancelled SQLite worker before fixture shutdown.
            drop(store.connection.lock().await);
            pool.close_task_companions("test.complete").await;
            downstream.cancel().await.unwrap();
        }
    }
}
