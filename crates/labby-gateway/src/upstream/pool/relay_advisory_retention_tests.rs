//! Early advisories must not retain arbitrary upstream payloads indefinitely.

use super::*;

fn token(value: &str) -> ProgressToken {
    ProgressToken(rmcp::model::NumberOrString::String(value.to_owned().into()))
}

#[tokio::test]
async fn oversized_unknown_progress_is_not_retained() {
    let routes = RelayRouteState::default();
    let id = token("unknown");
    let params = ProgressNotificationParam::new(id.clone(), 0.5).with_message("x".repeat(65_537));
    assert!(routes.translate_or_queue_progress(params).await.is_none());
    assert!(routes.progress.lock().await.pending.get(&id).is_none());

    let huge_id = token(&"k".repeat(65_537));
    routes
        .translate_or_queue_progress(ProgressNotificationParam::new(huge_id.clone(), 0.5))
        .await;
    assert!(routes.progress.lock().await.pending.get(&huge_id).is_none());
}

#[tokio::test]
async fn oversized_unknown_cancellation_is_not_retained() {
    let routes = RelayRouteState::default();
    let id = RequestId::String("unknown".into());
    routes
        .translate_or_queue_cancellation(CancelledNotificationParam::new(
            Some(id.clone()),
            Some("x".repeat(65_537)),
        ))
        .await;
    let pending = routes
        .register_request(
            id,
            RequestId::String("client".into()),
            token("progress"),
            None,
        )
        .await;
    assert!(pending.cancellation.is_none());
}

#[tokio::test(start_paused = true)]
async fn expired_early_advisories_do_not_attach_to_a_later_request() {
    let routes = RelayRouteState::default();
    let id = RequestId::String("early-request".into());
    let progress = token("early-progress");
    routes
        .translate_or_queue_progress(ProgressNotificationParam::new(progress.clone(), 0.5))
        .await;
    routes
        .translate_or_queue_cancellation(CancelledNotificationParam::new(
            Some(id.clone()),
            Some("early".into()),
        ))
        .await;
    tokio::time::advance(Duration::from_secs(31)).await;
    let pending = routes
        .register_request(
            id,
            RequestId::String("client".into()),
            progress.clone(),
            Some(token("client-progress")),
        )
        .await;
    assert!(pending.cancellation.is_none());
    assert!(
        routes
            .take_pending_progress_batch_or_activate(&progress)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn aggregate_early_progress_retention_has_a_byte_budget() {
    let routes = RelayRouteState::default();
    let mut ids = Vec::new();
    for i in 0..64 {
        let id = token(&format!("unknown-{i}"));
        routes
            .translate_or_queue_progress(
                ProgressNotificationParam::new(id.clone(), 0.5).with_message("x".repeat(8_192)),
            )
            .await;
        ids.push(id);
    }
    let progress = routes.progress.lock().await;
    let retained: usize = ids
        .iter()
        .filter_map(|id| progress.pending.get(id))
        .flatten()
        .map(|params| serde_json::to_vec(params).unwrap().len())
        .sum();
    assert!(retained > 0, "ordinary early progress remains useful");
    assert!(
        retained <= 65_536,
        "retained {retained} bytes of unknown progress"
    );
}

#[tokio::test]
async fn early_advisories_drain_once_in_order_and_release_capacity() {
    let routes = RelayRouteState::default();
    let progress = token("early");
    for value in [0.25, 0.5] {
        routes
            .translate_or_queue_progress(ProgressNotificationParam::new(progress.clone(), value))
            .await;
    }
    let id = RequestId::String("request".into());
    routes
        .translate_or_queue_cancellation(CancelledNotificationParam::new(
            Some(id.clone()),
            Some("early".into()),
        ))
        .await;
    let pending = routes
        .register_request(
            id.clone(),
            RequestId::String("client".into()),
            progress.clone(),
            Some(token("client-progress")),
        )
        .await;
    assert_eq!(
        pending.cancellation.unwrap().request_id,
        Some(RequestId::String("client".into()))
    );
    let values = routes
        .take_pending_progress_batch_or_activate(&progress)
        .await
        .unwrap();
    assert_eq!(
        values
            .iter()
            .map(|value| value.progress)
            .collect::<Vec<_>>(),
        [0.25, 0.5]
    );
    assert!(
        values
            .iter()
            .all(|value| value.progress_token == token("client-progress"))
    );
    assert!(
        routes
            .take_pending_progress_batch_or_activate(&progress)
            .await
            .is_none()
    );
    routes.unregister_request(&id).await;
    assert_eq!(routes.progress.lock().await.pending.len(), 0);
    assert_eq!(routes.requests.lock().await.pending_cancellations.len(), 0);
}
