//! Successful intent fences only its upstream; failed persistence is global.
use super::*;

#[tokio::test]
async fn prepared_intent_keeps_unaffected_reads_observations_and_registrations_available() {
    let store = TaskRouteStore::open_in_memory().await.unwrap();
    let mut affected = record();
    affected.ttl_ms = None;
    let mut unaffected = affected.clone();
    unaffected.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    unaffected.upstream_name = "unaffected".into();
    unaffected.authorization = TaskRouteAuthorization::root();
    store.insert(affected.clone()).await.unwrap();
    store.insert(unaffected.clone()).await.unwrap();
    store
        .prepare_upstream_revocation(vec![affected.upstream_name.clone()])
        .await
        .unwrap();
    assert!(store.get(&affected.public_task_id).await.unwrap().is_none());
    assert_eq!(
        store
            .get_for_caller(
                &unaffected.public_task_id,
                Some("alice"),
                &TaskRouteAuthorization::root()
            )
            .await
            .unwrap(),
        Some(unaffected.clone())
    );
    assert!(store.update_hints(affected.clone()).await.is_err());
    unaffected.updated_at_unix_ms += 1;
    store.update_hints(unaffected.clone()).await.unwrap();
    let mut new_affected = affected.clone();
    new_affected.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    assert!(
        store
            .insert(new_affected)
            .await
            .unwrap_err()
            .contains("fenced")
    );
    let mut new_unaffected = unaffected.clone();
    new_unaffected.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    store.insert(new_unaffected.clone()).await.unwrap();
    // An unrelated OAuth revocation cannot promote prepared intent to global
    // quarantine or consume the configuration fence.
    store
        .remove_oauth_subject("different", "oauth-alice")
        .await
        .unwrap();
    assert!(store.get(&affected.public_task_id).await.unwrap().is_none());
    assert_eq!(
        store.get(&unaffected.public_task_id).await.unwrap(),
        Some(unaffected.clone())
    );
    store.complete_prepared_revocations().await.unwrap();
    assert!(store.get(&affected.public_task_id).await.unwrap().is_none());
    assert_eq!(
        store.get(&unaffected.public_task_id).await.unwrap(),
        Some(unaffected)
    );
    assert_eq!(
        store.get(&new_unaffected.public_task_id).await.unwrap(),
        Some(new_unaffected)
    );
}

#[tokio::test]
async fn prepared_cleanup_failure_globally_quarantines_until_durable_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let mut affected = record();
    affected.ttl_ms = None;
    let mut unaffected = affected.clone();
    unaffected.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    unaffected.upstream_name = "unaffected".into();
    store.insert(affected.clone()).await.unwrap();
    store.insert(unaffected.clone()).await.unwrap();
    store
        .prepare_upstream_revocation(vec![affected.upstream_name.clone()])
        .await
        .unwrap();
    let database = rusqlite::Connection::open(&path).unwrap();
    database.execute_batch("CREATE TRIGGER fail_cleanup BEFORE DELETE ON task_routes BEGIN SELECT RAISE(ABORT, 'failure'); END;").unwrap();
    assert!(store.complete_prepared_revocations().await.is_err());
    assert!(
        store
            .get(&unaffected.public_task_id)
            .await
            .unwrap_err()
            .contains("quarantined")
    );
    assert!(
        store
            .insert(record())
            .await
            .unwrap_err()
            .contains("quarantined")
    );
    database.execute_batch("DROP TRIGGER fail_cleanup").unwrap();
    store.complete_prepared_revocations().await.unwrap();
    assert!(store.get(&affected.public_task_id).await.unwrap().is_none());
    assert!(
        store
            .get(&unaffected.public_task_id)
            .await
            .unwrap()
            .is_some()
    );
    drop(store);
    let reopened = TaskRouteStore::open(path).await.unwrap();
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
}

#[tokio::test]
async fn interrupted_prepared_intent_replays_only_affected_routes_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routes.db");
    let store = TaskRouteStore::open(path.clone()).await.unwrap();
    let mut affected = record();
    affected.ttl_ms = None;
    let mut unaffected = affected.clone();
    unaffected.public_task_id = format!("labby-task-{}", uuid::Uuid::new_v4().simple());
    unaffected.upstream_name = "unaffected".into();
    store.insert(affected.clone()).await.unwrap();
    store.insert(unaffected.clone()).await.unwrap();
    store
        .prepare_upstream_revocation(vec![affected.upstream_name.clone()])
        .await
        .unwrap();
    assert!(
        store
            .get(&unaffected.public_task_id)
            .await
            .unwrap()
            .is_some()
    );
    drop(store);
    let reopened = TaskRouteStore::open(path).await.unwrap();
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
}
