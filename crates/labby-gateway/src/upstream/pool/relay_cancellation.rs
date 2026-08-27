//! Bounded, acknowledgement-aware cancellation for relayed upstream requests.

use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use rmcp::RoleClient;
use rmcp::model::{
    CancelledNotificationParam, ClientRequest, CustomRequest, RequestId, ServerResult,
};
use rmcp::service::{Peer, ServiceError};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::MCP_RELAY_CANCELLATION_REQUEST_METHOD;

use super::http_cancellation::HttpCancellationSender;

const RELAY_CANCELLATION_ATTEMPT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(250);
/// Ceiling on best-effort delivery of an upstream cancellation.
///
/// Shared with the pooled tool-call guard (`tool_call_cancel.rs`): both
/// detach delivery, so both need the same bound to stop a wedged upstream
/// accumulating parked tasks.
pub(super) const CANCELLATION_DELIVERY_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(1);
const RELAY_CANCELLATION_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::from_millis(10),
    std::time::Duration::from_millis(25),
    std::time::Duration::from_millis(50),
];

async fn send_labby_relay_cancellation(
    peer: &Peer<RoleClient>,
    reason: &str,
    token: &str,
) -> Result<bool, ServiceError> {
    let result = peer
        .send_request(ClientRequest::CustomRequest(CustomRequest::new(
            MCP_RELAY_CANCELLATION_REQUEST_METHOD,
            Some(serde_json::json!({
                "reason": reason,
                "token": token,
            })),
        )))
        .await?;
    match result {
        ServerResult::CustomResult(rmcp::model::CustomResult(result)) => result
            .get("cancelled")
            .and_then(serde_json::Value::as_bool)
            .ok_or(ServiceError::UnexpectedResponse),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}

pub(super) struct PendingRelayRequestId {
    id: watch::Sender<Option<RequestId>>,
}

impl Default for PendingRelayRequestId {
    fn default() -> Self {
        let (id, _) = watch::channel(None);
        Self { id }
    }
}

impl PendingRelayRequestId {
    pub(super) fn set(&self, request_id: RequestId) {
        self.id.send_replace(Some(request_id));
    }

    async fn wait(&self, timeout: std::time::Duration) -> Option<RequestId> {
        let mut id = self.id.subscribe();
        if let Some(request_id) = id.borrow().clone() {
            return Some(request_id);
        }
        tokio::time::timeout(timeout, async move {
            loop {
                id.changed().await.ok()?;
                if let Some(request_id) = id.borrow().clone() {
                    return Some(request_id);
                }
            }
        })
        .await
        .ok()
        .flatten()
    }
}

async fn send_peer_relay_cancellation_once(
    peer: &Peer<RoleClient>,
    reason: &str,
    token: &str,
) -> bool {
    match tokio::time::timeout(
        RELAY_CANCELLATION_ATTEMPT_TIMEOUT,
        send_labby_relay_cancellation(peer, reason, token),
    )
    .await
    {
        Ok(Ok(correlated)) => correlated,
        Ok(Err(error)) => {
            tracing::debug!(
                error = %error,
                "upstream did not accept the Labby relay cancellation request"
            );
            false
        }
        Err(_) => false,
    }
}

async fn deliver_peer_relay_cancellation(
    peer: Peer<RoleClient>,
    request_id: Arc<PendingRelayRequestId>,
    reason: String,
    token: String,
) -> bool {
    if send_peer_relay_cancellation_once(&peer, &reason, &token).await {
        return true;
    }
    if request_id
        .wait(CANCELLATION_DELIVERY_TIMEOUT)
        .await
        .is_none()
    {
        return false;
    }
    for delay in RELAY_CANCELLATION_RETRY_DELAYS {
        tokio::time::sleep(delay).await;
        if send_peer_relay_cancellation_once(&peer, &reason, &token).await {
            return true;
        }
    }
    false
}

async fn send_http_relay_cancellation_once(
    sender: &HttpCancellationSender,
    reason: &str,
    token: &str,
) -> bool {
    match tokio::time::timeout(
        RELAY_CANCELLATION_ATTEMPT_TIMEOUT,
        sender.send_relay_token(reason, token),
    )
    .await
    {
        Ok(Ok(correlated)) => correlated,
        Ok(Err(error)) => {
            tracing::debug!(
                error = %error,
                "best-effort HTTP relay-token cancellation failed"
            );
            false
        }
        Err(_) => false,
    }
}

pub(super) async fn deliver_http_relay_cancellation(
    sender: HttpCancellationSender,
    request_id: Arc<PendingRelayRequestId>,
    reason: String,
    token: String,
) -> bool {
    if send_http_relay_cancellation_once(&sender, &reason, &token).await {
        return true;
    }
    if request_id
        .wait(CANCELLATION_DELIVERY_TIMEOUT)
        .await
        .is_none()
    {
        return false;
    }
    for delay in RELAY_CANCELLATION_RETRY_DELAYS {
        tokio::time::sleep(delay).await;
        if send_http_relay_cancellation_once(&sender, &reason, &token).await {
            return true;
        }
    }
    false
}

pub(super) fn dispatch_relay_cancellation(
    peer: &Peer<RoleClient>,
    cancellation_sender: Option<&HttpCancellationSender>,
    request_id: &Arc<PendingRelayRequestId>,
    reason: &str,
    token: &str,
    dispatched: &AtomicBool,
) {
    if dispatched.swap(true, Ordering::AcqRel) {
        return;
    }

    // Start the independently correlated HTTP request before waiting for the
    // upstream request id. Stateless HTTP can assign a different session to
    // this side channel, so the relay token is the only correlation value that
    // survives that boundary and must not be delayed by compatibility paths.
    if let Some(sender) = cancellation_sender {
        let sender = sender.clone();
        let request_id = Arc::clone(request_id);
        let reason = reason.to_string();
        let token = token.to_string();
        tokio::spawn(async move {
            match tokio::time::timeout(
                CANCELLATION_DELIVERY_TIMEOUT,
                deliver_http_relay_cancellation(sender, request_id, reason, token),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => tracing::debug!(
                    "HTTP relay-token cancellation did not correlate after bounded retries"
                ),
                Err(_) => tracing::debug!("HTTP relay-token cancellation delivery timed out"),
            }
        });
    }

    let peer_for_token = peer.clone();
    let request_id_for_token = Arc::clone(request_id);
    let reason_for_peer = reason.to_string();
    let token_for_peer = token.to_string();
    tokio::spawn(async move {
        match tokio::time::timeout(
            CANCELLATION_DELIVERY_TIMEOUT,
            deliver_peer_relay_cancellation(
                peer_for_token,
                request_id_for_token,
                reason_for_peer,
                token_for_peer,
            ),
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => tracing::debug!(
                "Labby relay cancellation request did not correlate after bounded retries"
            ),
            Err(_) => tracing::debug!("Labby relay cancellation request timed out"),
        }
    });

    // Standard cancellation remains a compatibility path for peers that do
    // not understand the Labby relay token. Waiting for rmcp to assign the
    // request id must not delay either token-based side channel or local
    // RequestHandle cancellation, so resolve and notify entirely off-path.
    let request_id = Arc::clone(request_id);
    let peer = peer.clone();
    let cancellation_sender = cancellation_sender.cloned();
    let reason = reason.to_string();
    let token = token.to_string();
    tokio::spawn(async move {
        let Some(request_id) = request_id.wait(CANCELLATION_DELIVERY_TIMEOUT).await else {
            tracing::debug!("upstream request id was unavailable for standard cancellation");
            return;
        };

        if let Some(sender) = cancellation_sender {
            let request_id_for_http = request_id.clone();
            let reason_for_http = reason.clone();
            let token = token.clone();
            tokio::spawn(async move {
                match tokio::time::timeout(
                    CANCELLATION_DELIVERY_TIMEOUT,
                    sender.send(request_id_for_http.clone(), Some(reason_for_http), &token),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::debug!(
                            request_id = ?request_id_for_http,
                            error = %error,
                            "best-effort standard HTTP cancellation failed"
                    ),
                    Err(_) => tracing::debug!(
                            request_id = ?request_id_for_http,
                            "best-effort standard HTTP cancellation timed out"
                    ),
                }
            });
        }

        let notification = peer.notify_cancelled(CancelledNotificationParam::new(
            Some(request_id.clone()),
            Some(reason),
        ));
        match tokio::time::timeout(CANCELLATION_DELIVERY_TIMEOUT, notification).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::debug!(
                request_id = ?request_id,
                error = %error,
                "standard upstream cancellation notification failed"
            ),
            Err(_) => tracing::debug!(
                request_id = ?request_id,
                "standard upstream cancellation notification timed out"
            ),
        }
    });
}

pub(super) enum RelaySendOutcome<T> {
    Sent(Result<T, ServiceError>),
    Cancelled,
    TimedOut,
}

pub(super) enum RelayPermitOutcome<T, E> {
    Acquired(Result<T, E>),
    Cancelled,
    TimedOut,
}

pub(super) async fn await_relay_permit<T, E>(
    acquire: impl Future<Output = Result<T, E>>,
    downstream_cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> RelayPermitOutcome<T, E> {
    tokio::select! {
        biased;
        () = downstream_cancel.cancelled() => RelayPermitOutcome::Cancelled,
        () = tokio::time::sleep_until(deadline) => RelayPermitOutcome::TimedOut,
        result = acquire => RelayPermitOutcome::Acquired(result),
    }
}

pub(super) async fn await_relay_send<T>(
    send: impl Future<Output = Result<T, ServiceError>>,
    downstream_cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> RelaySendOutcome<T> {
    tokio::select! {
        biased;
        () = downstream_cancel.cancelled() => RelaySendOutcome::Cancelled,
        () = tokio::time::sleep_until(deadline) => RelaySendOutcome::TimedOut,
        result = send => RelaySendOutcome::Sent(result),
    }
}

pub(super) fn spawn_bounded_handle_cancellation(
    cancellation: impl Future<Output = Result<(), ServiceError>> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        match tokio::time::timeout(CANCELLATION_DELIVERY_TIMEOUT, cancellation).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::debug!(
                error = %error,
                "best-effort rmcp request-handle cancellation failed"
            ),
            Err(_) => tracing::debug!("best-effort rmcp request-handle cancellation timed out"),
        }
    })
}
