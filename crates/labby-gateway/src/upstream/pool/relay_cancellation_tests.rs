use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Instant;

use rmcp::model::{ProtocolVersion, RequestId};
use rmcp::service::ServiceError;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::MCP_RELAY_CANCELLATION_REQUEST_METHOD;

use super::http_cancellation::build_http_cancellation_sender;
use super::relay_cancellation::{
    PendingRelayRequestId, RelayPermitOutcome, RelaySendOutcome, await_relay_permit,
    await_relay_send, deliver_http_relay_cancellation, spawn_bounded_handle_cancellation,
};

#[derive(Clone)]
struct RelayCancellationAckResponder {
    request_visible: Arc<AtomicBool>,
    false_responses: Arc<AtomicUsize>,
    true_responses: Arc<AtomicUsize>,
}

impl Respond for RelayCancellationAckResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let request: serde_json::Value =
            serde_json::from_slice(&request.body).expect("relay cancellation request JSON");
        let correlated = self.request_visible.load(Ordering::SeqCst);
        if correlated {
            self.true_responses.fetch_add(1, Ordering::SeqCst);
        } else {
            self.false_responses.fetch_add(1, Ordering::SeqCst);
        }
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": {"cancelled": correlated},
            }))
    }
}

#[tokio::test]
async fn http_relay_cancellation_retries_false_ack_after_request_publication() {
    let request_visible = Arc::new(AtomicBool::new(false));
    let false_responses = Arc::new(AtomicUsize::new(0));
    let true_responses = Arc::new(AtomicUsize::new(0));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header(
            "mcp-protocol-version",
            ProtocolVersion::V_2026_07_28.as_str(),
        ))
        .and(header("mcp-method", MCP_RELAY_CANCELLATION_REQUEST_METHOD))
        .respond_with(RelayCancellationAckResponder {
            request_visible: Arc::clone(&request_visible),
            false_responses: Arc::clone(&false_responses),
            true_responses: Arc::clone(&true_responses),
        })
        .mount(&server)
        .await;

    let mut config = super::testsupport::test_upstream_config();
    config.url = Some(format!("{}/mcp", server.uri()));
    let sender = build_http_cancellation_sender(&config, None, None, None)
        .await
        .expect("build cancellation sender")
        .expect("HTTP upstream has a cancellation sender");
    let pending_request_id = Arc::new(PendingRelayRequestId::default());
    let delivery = tokio::spawn(deliver_http_relay_cancellation(
        sender,
        Arc::clone(&pending_request_id),
        "downstream request cancelled".to_string(),
        "ack-retry-token".to_string(),
    ));

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while false_responses.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("early cancellation receives cancelled=false");
    request_visible.store(true, Ordering::SeqCst);
    pending_request_id.set(RequestId::String("published-request".into()));

    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), delivery)
            .await
            .expect("bounded relay cancellation delivery")
            .expect("delivery task joins"),
        "delivery must continue until the server confirms cancelled=true"
    );
    assert_eq!(false_responses.load(Ordering::SeqCst), 1);
    assert!(
        true_responses.load(Ordering::SeqCst) >= 1,
        "request publication must trigger an acknowledged retry"
    );
}

#[tokio::test]
async fn downstream_cancellation_interrupts_a_delayed_relay_send() {
    let cancellation = CancellationToken::new();
    let cancellation_for_send = cancellation.clone();
    let send_completed = Arc::new(AtomicBool::new(false));
    let send_completed_in_future = Arc::clone(&send_completed);
    let send = async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        send_completed_in_future.store(true, Ordering::SeqCst);
        Ok::<(), ServiceError>(())
    };
    let relay_send = tokio::spawn(async move {
        await_relay_send(
            send,
            &cancellation_for_send,
            tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .await
    });

    tokio::task::yield_now().await;
    cancellation.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), relay_send)
        .await
        .expect("cancellation interrupts blocked relay send")
        .expect("relay send task joins");

    assert!(matches!(outcome, RelaySendOutcome::Cancelled));
    assert!(
        !send_completed.load(Ordering::SeqCst),
        "cancellation must drop the blocked send future before it can enqueue"
    );
}

#[tokio::test]
async fn downstream_cancellation_interrupts_queued_permit_acquisition() {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
    let held = Arc::clone(&semaphore)
        .acquire_owned()
        .await
        .expect("initial permit");
    let cancellation = CancellationToken::new();
    let cancellation_for_wait = cancellation.clone();
    let queued = tokio::spawn(async move {
        await_relay_permit(
            semaphore.acquire_owned(),
            &cancellation_for_wait,
            tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        )
        .await
    });

    tokio::task::yield_now().await;
    cancellation.cancel();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("cancelled queue wait exits promptly")
        .expect("queue task joins");
    assert!(matches!(outcome, RelayPermitOutcome::Cancelled));
    drop(held);
}

// Paused time: every wait here is a tokio timer against a `tokio::time::Instant`
// deadline, so the budget arithmetic this test pins is exact under the virtual
// clock. Measured against the wall clock it failed intermittently under
// parallel test load, when scheduler overshoot on the 40ms remaining budget
// pushed the observed send past the 75ms ceiling.
#[tokio::test(start_paused = true)]
async fn permit_wait_and_send_share_one_absolute_deadline() {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(120);
    let cancellation = CancellationToken::new();
    let permit = await_relay_permit(
        async {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            Ok::<_, ()>(())
        },
        &cancellation,
        deadline,
    )
    .await;
    assert!(matches!(permit, RelayPermitOutcome::Acquired(Ok(()))));

    // Virtual-clock instant: under `start_paused`, wall-clock elapsed is
    // near-zero regardless of behavior, so a `std::time::Instant` here would
    // assert nothing. On the virtual clock the arithmetic is exact — the send
    // gets the 40ms remaining from the 120ms absolute deadline, never its own
    // fresh 80ms.
    let started_send = tokio::time::Instant::now();
    let send = await_relay_send(
        async {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            Ok::<_, ServiceError>(())
        },
        &cancellation,
        deadline,
    )
    .await;
    assert!(matches!(send, RelaySendOutcome::TimedOut));
    assert!(
        started_send.elapsed() < std::time::Duration::from_millis(75),
        "send receives only the remaining absolute-deadline budget, got {:?}",
        started_send.elapsed()
    );
}

#[tokio::test]
async fn stalled_handle_cleanup_is_detached_and_bounded() {
    let cleanup_completed = Arc::new(AtomicBool::new(false));
    let cleanup_completed_in_future = Arc::clone(&cleanup_completed);
    let stalled_cleanup = async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        cleanup_completed_in_future.store(true, Ordering::SeqCst);
        Ok::<(), ServiceError>(())
    };

    let started = Instant::now();
    let cleanup = spawn_bounded_handle_cancellation(stalled_cleanup);
    // Two constraints pin this ceiling. It must sit far above scheduler jitter
    // (a 100ms ceiling failed correct runs on a loaded machine), but it must
    // also stay below RELAY_CANCELLATION_DELIVERY_TIMEOUT (1s): a regression
    // that awaited the bounded cleanup before returning would come back at
    // ~1s when that internal timer fires, and only a sub-second ceiling
    // catches it.
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "stalled rmcp cleanup must not delay the caller's Cancelled/Timeout outcome: {:?}",
        started.elapsed()
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), cleanup)
        .await
        .expect("cleanup task honors the one-second bound")
        .expect("cleanup task joins");
    assert!(
        !cleanup_completed.load(Ordering::SeqCst),
        "the stalled cleanup future must be dropped at the bound"
    );
}
