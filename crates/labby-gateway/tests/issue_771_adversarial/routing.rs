use crate::fixture::{Downstream, Harness, OAUTH_OWNER, OWNER, UPSTREAM};
use labby_gateway::upstream::pool::TaskRouteAuthorization;
use rmcp::model::{
    CallToolResponse, CancelTaskParams, GetTaskParams, InputResponses, UpdateTaskParams,
};
use std::collections::HashSet;

async fn assert_hidden(
    h: &Harness,
    downstream: &Downstream,
    id: &str,
    owner: Option<&str>,
    auth: &TaskRouteAuthorization,
) {
    let before = h.backend.task_call_count();
    assert_eq!(
        h.pool
            .get_task_routed(GetTaskParams::new(id), owner, auth, downstream.peer.clone())
            .await
            .unwrap_err(),
        "task not found"
    );
    assert_eq!(
        h.pool
            .update_task_routed(
                UpdateTaskParams::new(id, InputResponses::default()),
                owner,
                auth,
                id,
                downstream.peer.clone()
            )
            .await
            .unwrap_err(),
        "task not found"
    );
    assert_eq!(
        h.pool
            .cancel_task_routed(
                CancelTaskParams::new(id),
                owner,
                auth,
                id,
                downstream.peer.clone()
            )
            .await
            .unwrap_err(),
        "task not found"
    );
    assert_eq!(
        h.backend.task_call_count(),
        before,
        "rejection precedes upstream I/O for all three methods"
    );
}

#[tokio::test]
async fn durable_ack_is_immediately_pollable_from_a_new_downstream() {
    let h = Harness::new().await;
    let mut creator = Downstream::new().await;
    let id = h.create(&creator, None).await;
    let parsed = uuid::Uuid::parse_str(id.strip_prefix("labby-task-").unwrap()).unwrap();
    assert_eq!(parsed.get_version_num(), 4);
    assert_ne!(id, "native-task-1");
    creator.disconnect().await;
    let replacement = Downstream::new().await;
    let result = h.get(&replacement, &id).await.unwrap();
    assert_eq!(result.task.task.task_id, id);
    assert_eq!(
        h.backend.0.lock().unwrap().task_calls,
        [("tasks/get".to_string(), "native-task-1".to_string())]
    );
    assert_eq!(
        h.backend.0.lock().unwrap().creates,
        1,
        "downstream reconnect never replays creation"
    );
}

#[tokio::test]
async fn guesses_wrong_owners_and_changed_routes_are_indistinguishable_without_dispatch() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    let root = TaskRouteAuthorization::root();
    let long_id = "x".repeat(4096);
    for guessed in [
        "",
        "native-task-1",
        "labby-task-1",
        "../routes.db",
        "' OR 1=1--",
        "labby-task-00000000000000000000000000000000",
        "☃",
        long_id.as_str(),
    ] {
        assert_hidden(&h, &downstream, guessed, Some(OWNER), &root).await;
    }
    for owner in [None, Some("another-owner")] {
        assert_hidden(&h, &downstream, &id, owner, &root).await;
    }
    for auth in [
        TaskRouteAuthorization::new("another-route", None),
        TaskRouteAuthorization::new("root", Some(Default::default())),
    ] {
        assert_hidden(&h, &downstream, &id, Some(OWNER), &auth).await;
    }
    h.get(&downstream, &id).await.unwrap();
}

#[tokio::test]
async fn concurrent_owners_cannot_cross_read_update_or_cancel() {
    let h = Harness::new().await;
    let alice = Downstream::new().await;
    let bob = Downstream::new().await;
    let (one, two) = tokio::join!(
        h.call(
            &alice,
            "start",
            Some(OWNER),
            None,
            TaskRouteAuthorization::root()
        ),
        h.call(
            &bob,
            "start",
            Some("other-owner"),
            None,
            TaskRouteAuthorization::root()
        )
    );
    let CallToolResponse::Task(one) = one.unwrap() else {
        panic!("first task")
    };
    let CallToolResponse::Task(two) = two.unwrap() else {
        panic!("second task")
    };
    assert_ne!(one.task.task_id, two.task.task_id);
    assert_eq!(h.row_count(), 2);
    assert_hidden(
        &h,
        &bob,
        &one.task.task_id,
        Some("other-owner"),
        &TaskRouteAuthorization::root(),
    )
    .await;
    assert_hidden(
        &h,
        &alice,
        &two.task.task_id,
        Some(OWNER),
        &TaskRouteAuthorization::root(),
    )
    .await;
    h.get(&alice, &one.task.task_id).await.unwrap();
    h.pool
        .get_task_routed(
            GetTaskParams::new(&two.task.task_id),
            Some("other-owner"),
            &TaskRouteAuthorization::root(),
            bob.peer.clone(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn configuration_change_rejects_old_routes_before_network_io() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    let mut changed = h.config.clone();
    changed.url = Some(format!("{}/replacement", h.server.uri()));
    h.pool
        .reconcile_lazy_upstreams(
            &[changed],
            &HashSet::from([UPSTREAM.to_string()]),
            "lane-f.config.changed",
        )
        .await;
    assert_hidden(
        &h,
        &downstream,
        &id,
        Some(OWNER),
        &TaskRouteAuthorization::root(),
    )
    .await;
    assert_eq!(
        h.row_count(),
        0,
        "configuration replacement must permanently revoke stale bindings"
    );
    h.pool
        .reconcile_lazy_upstreams(
            &[h.config.clone()],
            &HashSet::from([UPSTREAM.to_string()]),
            "lane-f.config.restore",
        )
        .await;
    assert_hidden(
        &h,
        &downstream,
        &id,
        Some(OWNER),
        &TaskRouteAuthorization::root(),
    )
    .await;
}

#[tokio::test]
async fn oauth_subject_invalidation_closes_only_matching_task_companions() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    // Trusted subject context drives the real invalidation API. This fixture does
    // not claim to exercise a token endpoint, refresh, or durable revocation.
    let id = h.create(&downstream, Some(OAUTH_OWNER)).await;
    let unrelated = h
        .pool
        .invalidate_oauth_subject_sessions(
            UPSTREAM,
            "unrelated-credential-owner",
            "lane-f.unrelated",
        )
        .await;
    assert_eq!(unrelated.task_routes, 0);
    h.get(&downstream, &id).await.unwrap();
    let removed = h
        .pool
        .invalidate_oauth_subject_sessions(UPSTREAM, OAUTH_OWNER, "lane-f.invalidated")
        .await;
    assert_eq!(removed.task_routes, 1);
    let before = h.backend.task_call_count();
    assert_eq!(h.get(&downstream, &id).await.unwrap_err(), "task not found");
    assert_eq!(h.backend.task_call_count(), before);
    assert_eq!(h.row_count(), 0, "revocation removes durable authority");
    assert_hidden(
        &h,
        &downstream,
        &id,
        Some("other-owner"),
        &TaskRouteAuthorization::root(),
    )
    .await;
}

#[tokio::test]
async fn pool_replacement_preserves_authorization_and_does_not_replay_creation() {
    let mut h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    h.replace_pool().await;
    let before = h.backend.task_call_count();
    assert_eq!(h.get(&downstream, &id).await.unwrap().task.task.task_id, id);
    assert_eq!(h.row_count(), 1);
    assert_eq!(h.backend.task_call_count(), before + 1);
    assert_eq!(h.backend.0.lock().unwrap().creates, 1);
    assert_hidden(
        &h,
        &downstream,
        &id,
        Some("other-owner"),
        &TaskRouteAuthorization::root(),
    )
    .await;
}

#[tokio::test]
async fn pool_replacement_must_reacquire_and_resolve_the_same_task() {
    let mut h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    h.replace_pool().await;
    let result = h
        .get(&downstream, &id)
        .await
        .expect("Gate 1 requires reacquisition from the durable route");
    assert_eq!(result.task.task.task_id, id);
    assert_eq!(
        h.backend.0.lock().unwrap().creates,
        1,
        "never recreate the upstream task"
    );
}

#[tokio::test]
async fn expired_route_does_not_dispatch_any_task_method() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    // Creation time is fixed well in the past. This is an expiry rejection test,
    // not the exact now == expires_at boundary owned by B's clock/store fixtures.
    h.database()
        .execute(
            "UPDATE task_routes SET ttl_ms = 1 WHERE public_task_id = ?1",
            [&id],
        )
        .unwrap();
    assert_hidden(
        &h,
        &downstream,
        &id,
        Some(OWNER),
        &TaskRouteAuthorization::root(),
    )
    .await;
}

#[tokio::test]
async fn storage_failure_withholds_ack_and_early_notifications_without_replaying_creation() {
    let h = Harness::new().await;
    let mut downstream = Downstream::new().await;
    h.backend.0.lock().unwrap().notifications = 2;
    h.database().execute_batch("CREATE TRIGGER lane_f_reject BEFORE INSERT ON task_routes BEGIN SELECT RAISE(ABORT, 'lane-f failure'); END;").unwrap();
    let error = h
        .call(
            &downstream,
            "start",
            Some(OWNER),
            None,
            TaskRouteAuthorization::root(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("upstream task registration failed"));
    assert!(!error.contains("native-task"));
    assert!(!error.contains("lane-f failure"));
    assert_eq!(h.row_count(), 0);
    assert_eq!(
        h.backend.0.lock().unwrap().creates,
        1,
        "upstream work may exist despite a rejected acknowledgement"
    );
    assert!(
        downstream.inbox.try_recv().is_err(),
        "uncommitted public task IDs are not exposed"
    );
    let sync = h
        .call(
            &downstream,
            "sync",
            Some(OWNER),
            None,
            TaskRouteAuthorization::root(),
        )
        .await
        .unwrap();
    assert!(matches!(sync, CallToolResponse::Complete(_)));
    assert_eq!(h.backend.0.lock().unwrap().creates, 1);
}

#[tokio::test]
async fn unreachable_upstream_does_not_delete_an_acknowledged_route_or_recreate_work() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    h.backend.0.lock().unwrap().unavailable = true;
    assert!(h.get(&downstream, &id).await.is_err());
    assert_eq!(h.row_count(), 1);
    assert_eq!(h.backend.0.lock().unwrap().creates, 1);
}

#[tokio::test]
async fn mutation_parameter_and_gateway_route_must_name_the_same_task() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    let other = h.create(&downstream, None).await;
    let auth = TaskRouteAuthorization::root();
    assert_eq!(
        h.pool
            .update_task_routed(
                UpdateTaskParams::new(&other, InputResponses::default()),
                Some(OWNER),
                &auth,
                &id,
                downstream.peer.clone()
            )
            .await
            .unwrap_err(),
        "task not found"
    );
    assert_eq!(
        h.pool
            .cancel_task_routed(
                CancelTaskParams::new(&other),
                Some(OWNER),
                &auth,
                &id,
                downstream.peer.clone()
            )
            .await
            .unwrap_err(),
        "task not found"
    );
    assert_eq!(
        h.backend.task_call_count(),
        0,
        "mismatched public routing never reaches upstream"
    );
    assert_eq!(h.row_count(), 2);
}

#[tokio::test]
async fn retention_write_failure_is_not_reported_as_a_successful_poll() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    h.backend.0.lock().unwrap().changed_hint = true;
    h.database().execute_batch("CREATE TRIGGER lane_f_reject_hint BEFORE UPDATE ON task_routes BEGIN SELECT RAISE(ABORT, 'lane-f hint failure'); END;").unwrap();
    assert_eq!(
        h.get(&downstream, &id).await.unwrap_err(),
        "task routing unavailable"
    );
    assert_eq!(h.row_count(), 1);
}

#[tokio::test]
async fn revocation_removes_cold_routes_and_cannot_be_undone_by_restart() {
    let mut h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, Some(OAUTH_OWNER)).await;
    h.replace_pool().await;
    h.pool
        .invalidate_oauth_subject_sessions(UPSTREAM, OAUTH_OWNER, "oauth.credentials.clear")
        .await;
    assert_eq!(
        h.row_count(),
        0,
        "revocation must remove durable routes without live companions"
    );
    h.replace_pool().await;
    assert_hidden(
        &h,
        &downstream,
        &id,
        Some(OWNER),
        &TaskRouteAuthorization::root(),
    )
    .await;
}

#[tokio::test]
async fn same_owner_refresh_preserves_durable_routes() {
    let mut h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, Some(OAUTH_OWNER)).await;
    h.pool
        .invalidate_oauth_subject_sessions(UPSTREAM, OAUTH_OWNER, "oauth.credentials.refresh")
        .await;
    assert_eq!(h.row_count(), 1);
    h.replace_pool().await;
    assert_eq!(h.get(&downstream, &id).await.unwrap().task.task.task_id, id);
    assert_eq!(h.backend.0.lock().unwrap().creates, 1);
}

#[tokio::test]
async fn cancelled_reconnect_never_dispatches_and_keeps_the_task_pollable() {
    let mut h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    h.replace_pool().await;
    let context = labby_gateway::upstream::pool::TaskCallContext::default();
    context.cancellation.cancel();
    let before = h.backend.task_call_count();
    assert_eq!(
        h.pool
            .get_task_routed_with_context(
                GetTaskParams::new(&id),
                Some(OWNER),
                &TaskRouteAuthorization::root(),
                downstream.peer.clone(),
                context
            )
            .await
            .unwrap_err(),
        "task request cancelled"
    );
    assert_eq!(h.backend.task_call_count(), before);
    assert_eq!(h.row_count(), 1);
    assert_eq!(h.get(&downstream, &id).await.unwrap().task.task.task_id, id);
    assert_eq!(h.backend.0.lock().unwrap().creates, 1);
}
