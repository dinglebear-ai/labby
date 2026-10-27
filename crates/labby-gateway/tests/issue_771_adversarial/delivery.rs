use crate::fixture::{BUDGET, Downstream, Harness, OWNER};
use labby_gateway::upstream::pool::TaskRouteAuthorization;
use rmcp::model::{CancelTaskParams, InputResponses, TaskStatus, UpdateTaskParams};

#[tokio::test]
async fn absent_notifications_do_not_turn_mutation_ack_into_failure_or_terminal_state() {
    let h = Harness::new().await;
    let downstream = Downstream::new().await;
    let id = h.create(&downstream, None).await;
    let auth = TaskRouteAuthorization::root();
    tokio::time::timeout(
        BUDGET,
        h.pool.update_task_routed(
            UpdateTaskParams::new(&id, InputResponses::default()),
            Some(OWNER),
            &auth,
            &id,
            downstream.peer.clone(),
        ),
    )
    .await
    .expect("bounded update notification barrier")
    .unwrap();
    tokio::time::timeout(
        BUDGET,
        h.pool.cancel_task_routed(
            CancelTaskParams::new(&id),
            Some(OWNER),
            &auth,
            &id,
            downstream.peer.clone(),
        ),
    )
    .await
    .expect("bounded cancel notification barrier")
    .unwrap();
    assert_eq!(h.row_count(), 1);
    assert_eq!(
        h.get(&downstream, &id).await.unwrap().task.status(),
        TaskStatus::Working,
        "cooperative cancellation acknowledgement is not terminal-state proof"
    );
    let state = h.backend.0.lock().unwrap();
    assert_eq!(
        state.task_calls,
        [
            ("tasks/update".to_owned(), "native-task-1".to_owned()),
            ("tasks/cancel".to_owned(), "native-task-1".to_owned()),
            ("tasks/get".to_owned(), "native-task-1".to_owned()),
        ]
    );
    assert_eq!(
        state.creates, 1,
        "notification loss does not replay creation or mutations"
    );
}

#[tokio::test]
async fn duplicate_task_notifications_are_complete_public_states_not_exactly_once_events() {
    let h = Harness::new().await;
    let mut downstream = Downstream::new().await;
    h.backend.0.lock().unwrap().notifications = 2;
    let id = h.create(&downstream, None).await;
    let first = downstream.notification().await;
    let second = downstream.notification().await;
    assert_eq!(first.task.task.task_id, id);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    assert_eq!(first.task.task.created_at, "2026-07-31T00:00:00Z");
    assert_eq!(first.task.status(), TaskStatus::Working);
    assert_eq!(h.row_count(), 1);
    assert_eq!(h.backend.0.lock().unwrap().creates, 1);
    // Repeated observations are not instructions to replay tools/call or input.
    assert_eq!(h.get(&downstream, &id).await.unwrap().task.task.task_id, id);
}

#[tokio::test]
async fn failed_notification_delivery_keeps_the_route_and_poll_rebinds_a_new_downstream() {
    let h = Harness::new().await;
    let mut dead = Downstream::new().await;
    let id = h.create(&dead, None).await;
    dead.disconnect().await;
    h.backend.0.lock().unwrap().notifications = 2;
    // The ordered upstream SSE notifications arrive before the get response.
    // Their downstream delivery fails, but neither poll nor durable route is lost.
    let result = h.get(&dead, &id).await.unwrap();
    assert_eq!(result.task.task.task_id, id);
    assert_eq!(h.row_count(), 1);
    let mut recovered = Downstream::new().await;
    let result = h.get(&recovered, &id).await.unwrap();
    assert_eq!(result.task.task.task_id, id);
    assert_eq!(recovered.notification().await.task.task.task_id, id);
    assert_eq!(recovered.notification().await.task.task.task_id, id);
    assert_eq!(h.backend.0.lock().unwrap().creates, 1);
    // This is downstream rebinding over the retained upstream peer, not D3's
    // future subscriptions/listen reconnect and not a notification replay store.
}
