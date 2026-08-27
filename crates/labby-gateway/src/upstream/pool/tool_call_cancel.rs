//! `tools/call` that tells the upstream to stop when the caller withdraws.
//!
//! rmcp has no drop-based cancellation for ordinary requests — only the
//! explicit `RequestHandle::cancel`. Dropping the response future therefore
//! abandons the *local* await while the upstream keeps executing, and the
//! gateway's own `tools/call` is exactly the request most likely to be running
//! real side effects (shell, PowerShell, writes) on the far side. A client that
//! sees a cancelled call and retries would otherwise run the same tool twice.
//!
//! Sending `notifications/cancelled` is the MCP-defined way to say so, and is
//! what the relay path already does via `relay_cancellation.rs`. This is the
//! pooled-path equivalent, scoped to tool calls.
//!
//! The guard fires on *any* early drop, not just caller cancellation: a local
//! deadline elapsing, or the surrounding task being dropped, both leave the
//! upstream working on a result nobody will read, and both deserve the same
//! notification.

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResponse, CallToolResult,
    CancelledNotificationParam, ClientRequest, RequestId, ServerResult,
};
use rmcp::service::{Peer, PeerRequestOptions};
use rmcp::{RoleClient, ServiceError};

use super::relay_cancellation::CANCELLATION_DELIVERY_TIMEOUT;

/// Reason string sent upstream when the in-flight call is abandoned.
const CANCEL_REASON: &str = "downstream caller cancelled the request";

/// Sends `notifications/cancelled` for an in-flight request unless disarmed.
///
/// Armed for exactly the window between the request reaching the wire and its
/// response arriving, so a completed call never emits a spurious cancellation.
struct CancelUpstreamOnDrop {
    /// `None` once disarmed — the response arrived and nothing needs cancelling.
    peer: Option<Peer<RoleClient>>,
    upstream: String,
    id: RequestId,
}

impl CancelUpstreamOnDrop {
    fn armed(peer: Peer<RoleClient>, upstream: &str, id: RequestId) -> Self {
        Self {
            peer: Some(peer),
            upstream: upstream.to_string(),
            id,
        }
    }

    fn disarm(&mut self) {
        self.peer = None;
    }
}

impl Drop for CancelUpstreamOnDrop {
    fn drop(&mut self) {
        let Some(peer) = self.peer.take() else {
            return;
        };
        // `Drop` cannot await, so delivery is detached — but it must be
        // *bounded*. `notify_cancelled` resolves only once the transport has
        // flushed, so a wedged upstream (a stdio child that stopped draining
        // its stdin — precisely the failure that motivated this code) would
        // otherwise park a task holding a `Peer` clone forever, one per
        // cancelled call, with no ceiling on how many accumulate.
        //
        // `try_current` rather than `tokio::spawn`: this guard is dropped
        // wherever its future is dropped, including runtime teardown, and
        // `tokio::spawn` panics with no reactor. A cancellation we cannot send
        // because the runtime is going away is one the upstream connection is
        // about to lose anyway.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let upstream = std::mem::take(&mut self.upstream);
        let id = self.id.clone();
        runtime.spawn(async move {
            let delivery = peer.notify_cancelled(CancelledNotificationParam::new(
                Some(id.clone()),
                Some(CANCEL_REASON.to_string()),
            ));
            // Whether the upstream was actually told is the question an
            // operator asks when a tool appears to have run twice, so record
            // every outcome rather than discarding the result.
            match tokio::time::timeout(CANCELLATION_DELIVERY_TIMEOUT, delivery).await {
                Ok(Ok(())) => tracing::debug!(
                    upstream = %upstream,
                    request_id = ?id,
                    action = "tool.cancel.notify",
                    outcome = "sent",
                    "told upstream to cancel the abandoned tool call"
                ),
                Ok(Err(error)) => tracing::warn!(
                    upstream = %upstream,
                    request_id = ?id,
                    action = "tool.cancel.notify",
                    outcome = "failed",
                    error = %error,
                    "could not tell upstream to cancel the abandoned tool call"
                ),
                Err(_) => tracing::warn!(
                    upstream = %upstream,
                    request_id = ?id,
                    action = "tool.cancel.notify",
                    outcome = "timeout",
                    "timed out telling upstream to cancel the abandoned tool call"
                ),
            }
        });
    }
}

/// Issue one `tools/call` and hold a cancel-on-drop guard over the response
/// wait.
///
/// The guard's arming window is deliberately zero-width: the last await before
/// the handle exists is rmcp's cancel-safe `tx.send`, so a drop there delivers
/// nothing to cancel, and the guard is constructed synchronously after it.
async fn guarded_call_tool(
    peer: &Peer<RoleClient>,
    upstream: &str,
    params: CallToolRequestParams,
) -> Result<ServerResult, ServiceError> {
    let handle = peer
        .send_request_with_option(
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            PeerRequestOptions::no_options(),
        )
        .await?;

    let mut guard = CancelUpstreamOnDrop::armed(handle.peer.clone(), upstream, handle.id.clone());
    let result = handle.await_response().await;
    // Disarm on every terminal outcome, errors included: an upstream that
    // already answered — even with a failure — has nothing left to cancel.
    guard.disarm();
    result
}

/// Cancel-aware equivalent of rmcp's `Peer::call_tool`.
///
/// `Peer::call_tool` is a single `tools/call` round trip (the MRTR round-driving
/// loop lives on `RunningService`, which the pool never holds), so one guard
/// covers it exactly.
pub(super) async fn call_tool_cancel_aware(
    peer: &Peer<RoleClient>,
    upstream: &str,
    params: CallToolRequestParams,
) -> Result<CallToolResult, ServiceError> {
    match guarded_call_tool(peer, upstream, params).await? {
        ServerResult::CallToolResult(result) => Ok(result),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}

/// Cancel-aware equivalent of rmcp's `Peer::call_tool_once`.
///
/// Single-shot: MRTR `input_required` and task results are preserved for the
/// caller to drive rather than resolved here.
pub(super) async fn call_tool_once_cancel_aware(
    peer: &Peer<RoleClient>,
    upstream: &str,
    params: CallToolRequestParams,
) -> Result<CallToolResponse, ServiceError> {
    match guarded_call_tool(peer, upstream, params).await? {
        ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
        ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
        // SEP-2663 Tasks extension: the server materialized a task.
        ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}
