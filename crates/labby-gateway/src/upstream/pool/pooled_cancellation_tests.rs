//! Cancellation behaviour of the *pooled* capability path.
//!
//! The relay path has carried cancellation since it was introduced
//! (`relay_cancellation.rs`); the pooled hot path did not. A downstream client
//! that disconnected — or whose request was killed by the HTTP transport
//! timeout — left the pooled call running to completion with nobody to receive
//! the result, and never told the upstream to stop. For a gateway whose
//! upstreams execute real side effects (shell, PowerShell) that meant a client
//! could see a failure, retry, and have the original call still running.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use tokio_util::sync::CancellationToken;

use super::super::types::UpstreamHealth;
use super::capability_call::CapabilityCallError;
use super::testsupport::catalog_pool_with_server;

/// Upstream that reports whether its in-flight `tools/call` was cancelled by
/// the caller or ran all the way to completion.
///
/// `context.ct` is cancelled by rmcp when a `notifications/cancelled` naming
/// this request arrives, so observing it here proves the notification actually
/// crossed the MCP boundary rather than the gateway merely dropping a future.
#[derive(Clone)]
struct CancelObservingServer {
    started: Arc<AtomicBool>,
    observed_cancel: Arc<AtomicBool>,
    ran_to_completion: Arc<AtomicBool>,
}

impl ServerHandler for CancelObservingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(Vec::new()))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.started.store(true, Ordering::SeqCst);
        tokio::select! {
            () = context.ct.cancelled() => {
                self.observed_cancel.store(true, Ordering::SeqCst);
                Err(ErrorData::internal_error("cancelled by caller", None))
            }
            () = tokio::time::sleep(Duration::from_secs(30)) => {
                self.ran_to_completion.store(true, Ordering::SeqCst);
                Ok(CallToolResult::success(Vec::new()).into())
            }
        }
    }
}

/// Poll `predicate` until it holds, or fail after `label`'s deadline.
async fn wait_for(label: &str, predicate: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {label}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Cancelling the downstream request must abandon the pooled call *and* tell
/// the upstream to stop, rather than letting it run to completion unheard.
#[tokio::test]
async fn cancelling_a_pooled_tool_call_aborts_it_and_notifies_the_upstream() {
    let started = Arc::new(AtomicBool::new(false));
    let observed_cancel = Arc::new(AtomicBool::new(false));
    let ran_to_completion = Arc::new(AtomicBool::new(false));

    let pool = catalog_pool_with_server(
        "cancel-test",
        CancelObservingServer {
            started: Arc::clone(&started),
            observed_cancel: Arc::clone(&observed_cancel),
            ran_to_completion: Arc::clone(&ran_to_completion),
        },
    )
    .await;

    let cancel = CancellationToken::new();
    let call = tokio::spawn({
        let pool = Arc::clone(&pool);
        let cancel = cancel.clone();
        async move {
            pool.call_tool_once_classified(
                "cancel-test",
                CallToolRequestParams::new("slow"),
                Some(&cancel),
            )
            .await
        }
    });

    // Only cancel once the upstream is genuinely mid-call, so the test cannot
    // pass by racing the request to the upstream in the first place.
    wait_for("upstream tool call to start", || {
        started.load(Ordering::SeqCst)
    })
    .await;
    cancel.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("cancelled call returns promptly instead of running to completion")
        .expect("call task did not panic");

    assert!(
        matches!(outcome, Some(Err(CapabilityCallError::Cancelled { .. }))),
        "cancelled pooled call should surface as Cancelled, got {outcome:?}"
    );

    wait_for("upstream to observe notifications/cancelled", || {
        observed_cancel.load(Ordering::SeqCst)
    })
    .await;
    assert!(
        !ran_to_completion.load(Ordering::SeqCst),
        "upstream work must not run to completion after the caller cancelled"
    );
}

/// The token is only a cancellation signal — an uncancelled call must behave
/// exactly as before, so wiring it in cannot regress the hot path.
#[tokio::test]
async fn an_uncancelled_pooled_tool_call_still_completes_normally() {
    let pool =
        catalog_pool_with_server("normal-test", super::testsupport::SlowResponseServer).await;
    let cancel = CancellationToken::new();

    let outcome = pool
        .call_tool_once_classified(
            "normal-test",
            CallToolRequestParams::new("quick"),
            Some(&cancel),
        )
        .await;

    assert!(
        matches!(outcome, Some(Ok(_))),
        "uncancelled call should succeed, got {outcome:?}"
    );
}

/// The Code Mode path reaches the pool through `call_tool_classified`, which
/// takes no token — it cancels by *dropping* the call future
/// (`labby-codemode/src/execute.rs`). The guard has to cover that drop, or the
/// exact path the production incident came from stays uncovered.
#[tokio::test]
async fn dropping_a_code_mode_style_call_still_notifies_the_upstream() {
    let started = Arc::new(AtomicBool::new(false));
    let observed_cancel = Arc::new(AtomicBool::new(false));
    let ran_to_completion = Arc::new(AtomicBool::new(false));

    let pool = catalog_pool_with_server(
        "drop-test",
        CancelObservingServer {
            started: Arc::clone(&started),
            observed_cancel: Arc::clone(&observed_cancel),
            ran_to_completion: Arc::clone(&ran_to_completion),
        },
    )
    .await;

    // Abandon the future the way Code Mode's `tokio::select!` does.
    let abandoned = tokio::time::timeout(
        Duration::from_millis(250),
        pool.call_tool_classified("drop-test", CallToolRequestParams::new("slow")),
    )
    .await;
    assert!(abandoned.is_err(), "call should still have been in flight");

    wait_for("upstream to observe notifications/cancelled", || {
        observed_cancel.load(Ordering::SeqCst)
    })
    .await;
    assert!(
        !ran_to_completion.load(Ordering::SeqCst),
        "upstream work must not run to completion after the caller went away"
    );
}

/// `biased;` in the select is load-bearing, not style. `rpc_future` is lazy:
/// with the cancel branch polled first, an already-cancelled token means the
/// request is never polled and so never reaches the wire. Without `biased`
/// tokio picks at random and roughly half the time writes the request out
/// before noticing — executing a side effect for a caller already gone.
#[tokio::test]
async fn a_call_cancelled_before_dispatch_never_reaches_the_upstream() {
    let started = Arc::new(AtomicBool::new(false));
    let pool = catalog_pool_with_server(
        "pre-cancel-test",
        CancelObservingServer {
            started: Arc::clone(&started),
            observed_cancel: Arc::new(AtomicBool::new(false)),
            ran_to_completion: Arc::new(AtomicBool::new(false)),
        },
    )
    .await;

    let cancel = CancellationToken::new();
    cancel.cancel();

    let outcome = pool
        .call_tool_once_classified(
            "pre-cancel-test",
            CallToolRequestParams::new("must-not-run"),
            Some(&cancel),
        )
        .await;

    assert!(
        matches!(outcome, Some(Err(CapabilityCallError::Cancelled { .. }))),
        "pre-cancelled call should surface as Cancelled, got {outcome:?}"
    );
    // Give any erroneously-dispatched request time to land before asserting.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !started.load(Ordering::SeqCst),
        "an already-cancelled call must never reach the upstream at all"
    );
}

/// A cancelled call is the caller withdrawing, not the upstream misbehaving.
/// Feeding it to the circuit breaker would let a flaky client quarantine a
/// perfectly healthy upstream, so the no-breaker choice needs pinning.
#[tokio::test]
async fn repeated_cancellations_do_not_trip_the_circuit_breaker() {
    let started = Arc::new(AtomicBool::new(false));
    let pool = catalog_pool_with_server(
        "breaker-test",
        CancelObservingServer {
            started: Arc::clone(&started),
            observed_cancel: Arc::new(AtomicBool::new(false)),
            ran_to_completion: Arc::new(AtomicBool::new(false)),
        },
    )
    .await;

    // Comfortably past CIRCUIT_BREAKER_THRESHOLD (3, see `types.rs`).
    for _ in 0..6 {
        started.store(false, Ordering::SeqCst);
        let cancel = CancellationToken::new();
        let call = tokio::spawn({
            let pool = Arc::clone(&pool);
            let cancel = cancel.clone();
            async move {
                pool.call_tool_once_classified(
                    "breaker-test",
                    CallToolRequestParams::new("slow"),
                    Some(&cancel),
                )
                .await
            }
        });
        wait_for("upstream tool call to start", || {
            started.load(Ordering::SeqCst)
        })
        .await;
        cancel.cancel();
        drop(call.await.expect("call task did not panic"));
    }

    assert!(
        matches!(
            pool.upstream_tool_health("breaker-test").await,
            Some(UpstreamHealth::Healthy)
        ),
        "caller cancellations must not quarantine a healthy upstream"
    );
    assert_eq!(pool.upstream_tool_last_error("breaker-test").await, None);
}

/// The guard keys on the response future being dropped, not on *why*. A local
/// deadline elapsing abandons the upstream exactly as a caller withdrawal does,
/// so it must produce the same notification — otherwise the timeout path
/// reintroduces the orphan this change exists to remove.
#[tokio::test]
async fn a_timed_out_pooled_tool_call_also_notifies_the_upstream() {
    let started = Arc::new(AtomicBool::new(false));
    let observed_cancel = Arc::new(AtomicBool::new(false));
    let ran_to_completion = Arc::new(AtomicBool::new(false));

    let pool = super::testsupport::catalog_pool_with_server_and_timeout(
        "timeout-test",
        CancelObservingServer {
            started: Arc::clone(&started),
            observed_cancel: Arc::clone(&observed_cancel),
            ran_to_completion: Arc::clone(&ran_to_completion),
        },
        Some(Duration::from_millis(50)),
    )
    .await;

    let outcome = pool
        .call_tool_once_classified("timeout-test", CallToolRequestParams::new("slow"), None)
        .await
        .expect("upstream is connected");

    assert!(
        matches!(outcome, Err(CapabilityCallError::Timeout { .. })),
        "expected Timeout class, got {outcome:?}"
    );
    wait_for("upstream to observe notifications/cancelled", || {
        observed_cancel.load(Ordering::SeqCst)
    })
    .await;
    assert!(
        !ran_to_completion.load(Ordering::SeqCst),
        "a timed-out call must not leave the upstream running to completion"
    );
}
