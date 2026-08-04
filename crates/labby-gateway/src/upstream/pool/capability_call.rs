//! Generic timed-capability-call skeleton shared by `tools_call`, `resources_read`,
//! and `prompts_get`.
//!
//! The three capability modules each follow the same structure:
//!
//! 1. Record start time + build `UpstreamRequestLog`.
//! 2. Optionally acquire a subject-scoped peer (OAuth path) or the pool peer (normal path).
//! 3. Issue the RPC with `tokio::time::timeout`.
//! 4. On success: check the response size cap, record circuit-breaker success, log finish.
//! 5. On upstream error: distinguish valid MCP application errors from broken
//!    transport/protocol state, updating connection health only for the latter.
//! 6. On timeout: record circuit-breaker failure, evict subject connection, log error.
//!
//! `timed_capability_call` encapsulates steps 3–6 so each capability module only
//! declares its own peer-acquisition and response-normalization logic.

use std::future::Future;
use std::time::Instant;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::helpers::max_response_bytes;
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_finish};
use super::usage_record::record_usage_call;

/// Structured failure from an upstream capability call.
///
/// Most gateway surfaces preserve their historical string errors by calling
/// `to_string()`. Code Mode consumes this typed form so JSON-RPC/MCP error
/// codes survive the pool boundary instead of collapsing into
/// `upstream_error`.
#[derive(Debug)]
pub(crate) enum CapabilityCallError {
    Mcp {
        data: rmcp::model::ErrorData,
        message: String,
    },
    Timeout {
        message: String,
    },
    QueueSaturated {
        message: String,
    },
    ResponseTooLarge {
        message: String,
    },
    Transport {
        message: String,
    },
    Protocol {
        message: String,
    },
    Other {
        message: String,
    },
}

impl CapabilityCallError {
    fn from_service_error(error: rmcp::ServiceError, message: String) -> Self {
        match error {
            rmcp::ServiceError::McpError(data) => Self::Mcp { data, message },
            rmcp::ServiceError::Timeout { .. } => Self::Timeout { message },
            rmcp::ServiceError::TransportSend(_)
            | rmcp::ServiceError::TransportClosed
            | rmcp::ServiceError::SubscriptionLagged { .. } => Self::Transport { message },
            rmcp::ServiceError::UnexpectedResponse => Self::Protocol { message },
            _ => Self::Other { message },
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Mcp { message, .. }
            | Self::Timeout { message }
            | Self::QueueSaturated { message }
            | Self::ResponseTooLarge { message }
            | Self::Transport { message }
            | Self::Protocol { message }
            | Self::Other { message } => message,
        }
    }
}

impl std::fmt::Display for CapabilityCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for CapabilityCallError {}

/// Outcome of a timed capability call before size-cap enforcement.
pub(super) enum RawCallOutcome<R> {
    Ok(R),
    /// The upstream returned an error (not a timeout).
    UpstreamError(rmcp::ServiceError),
    /// The tokio timeout elapsed.
    Timeout,
}

/// Helper to convert a `tokio::time::timeout` + `rmcp` result pair into
/// `RawCallOutcome`.
pub(super) fn classify_timeout_result<R>(
    result: Result<Result<R, rmcp::ServiceError>, tokio::time::error::Elapsed>,
) -> RawCallOutcome<R> {
    match result {
        Ok(Ok(r)) => RawCallOutcome::Ok(r),
        Ok(Err(e)) => RawCallOutcome::UpstreamError(e),
        Err(_) => RawCallOutcome::Timeout,
    }
}

/// Whether a service error indicates that the MCP connection itself is unhealthy.
///
/// A valid JSON-RPC/MCP error proves the peer is reachable and the protocol is
/// functioning. Tool-level authorization, validation, and execution failures must
/// remain visible to the caller and request logs without turning the whole upstream
/// red. Transport/protocol errors continue to feed the circuit breaker.
pub(super) fn service_error_affects_connection_health(error: &rmcp::ServiceError) -> bool {
    !matches!(error, rmcp::ServiceError::McpError(_))
}

/// Execute `rpc_future` under the pool's request timeout, enforce the
/// response-size cap (using `size_fn`), and emit structured log events.
///
/// # Parameters
///
/// - `pool` — used for circuit-breaker recording and `request_timeout`.
/// - `upstream_name` — name of the upstream, for logs and circuit-breaker keys.
/// - `capability` — which MCP capability is being exercised.
/// - `event` — pre-built `UpstreamRequestLog` (caller sets capability/item/transport).
/// - `start` — `Instant` recorded *before* peer acquisition (caller owns it so
///   elapsed time includes the peer-acquire step).
/// - `rpc_future` — the actual MCP call (`peer.call_tool(…)` / `peer.read_resource(…)` / …).
/// - `size_fn` — extracts the byte count from a successful response; use
///   `estimate_response_size` / `estimate_resource_response_size`.
/// - `subject` — `Some(subject)` when this is a subject-scoped OAuth call so that a
///   broken connection is evicted on error; `None` for the normal pool path.
/// - `error_message_fn` — builds the user-visible error string from the upstream
///   error display value.
/// - `timeout_message` — user-visible error string for the timeout case.
///
/// Returns `Ok(R)` on success and a typed `CapabilityCallError` on failure.
#[allow(clippy::too_many_arguments)]
pub(super) async fn timed_capability_call<R, Fut, SizeFn>(
    pool: &UpstreamPool,
    upstream_name: &str,
    capability: UpstreamCapability,
    event: UpstreamRequestLog<'_>,
    start: Instant,
    rpc_future: Fut,
    size_fn: SizeFn,
    subject: Option<&str>,
    error_message_fn: impl Fn(&dyn std::fmt::Display) -> String,
    timeout_message: String,
) -> Result<R, CapabilityCallError>
where
    Fut: Future<Output = Result<R, rmcp::ServiceError>>,
    SizeFn: Fn(&R) -> usize,
{
    // Enforce one wall-clock budget across peer acquisition, bulkhead wait, and
    // the RPC itself. Waiting for a permit must not extend the configured
    // upstream timeout, and queue pressure must not poison connection health.
    let gate_remaining = pool.request_timeout.saturating_sub(start.elapsed());
    if gate_remaining.is_zero() {
        log_upstream_request_error(
            event,
            start.elapsed().as_millis(),
            "queue_saturated",
            None,
            None,
            None,
        );
        record_usage_call(
            pool,
            event,
            subject,
            "queue_saturated",
            start.elapsed().as_millis(),
        );
        return Err(CapabilityCallError::QueueSaturated {
            message: format!(
                "upstream `{upstream_name}` concurrency queue exhausted the request timeout"
            ),
        });
    }
    let _permit = match tokio::time::timeout(
        gate_remaining,
        pool.acquire_upstream_call_permit(upstream_name),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        Ok(Err(error)) => return Err(CapabilityCallError::Other { message: error }),
        Err(_) => {
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "queue_saturated",
                None,
                None,
                None,
            );
            record_usage_call(
                pool,
                event,
                subject,
                "queue_saturated",
                start.elapsed().as_millis(),
            );
            return Err(CapabilityCallError::QueueSaturated {
                message: format!("upstream `{upstream_name}` concurrency queue timed out"),
            });
        }
    };

    let generation = pool.connection_generation(upstream_name, subject).await;
    let _stdio_inflight = super::stdio_transport::register_inflight(event, generation);
    let rpc_remaining = pool.request_timeout.saturating_sub(start.elapsed());
    let outcome = if rpc_remaining.is_zero() {
        RawCallOutcome::Timeout
    } else {
        classify_timeout_result(tokio::time::timeout(rpc_remaining, rpc_future).await)
    };

    match outcome {
        RawCallOutcome::Ok(result) => {
            let response_size = size_fn(&result);
            let max_bytes = max_response_bytes();
            if response_size > max_bytes {
                // The peer returned a complete, valid response. Rejecting it at
                // the gateway policy boundary must not mark the connection down.
                pool.record_success_for(upstream_name, capability).await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "response_too_large",
                    None,
                    Some(response_size),
                    Some(max_bytes),
                );
                record_usage_call(
                    pool,
                    event,
                    subject,
                    "response_too_large",
                    start.elapsed().as_millis(),
                );
                return Err(CapabilityCallError::ResponseTooLarge {
                    message: format!(
                        "upstream response too large ({response_size} bytes, max {max_bytes})"
                    ),
                });
            }
            pool.record_success_for(upstream_name, capability).await;
            log_upstream_request_finish(event, start.elapsed().as_millis(), Some(response_size));
            record_usage_call(pool, event, subject, "ok", start.elapsed().as_millis());
            Ok(result)
        }
        RawCallOutcome::UpstreamError(error) => {
            let message = error_message_fn(&error);
            if service_error_affects_connection_health(&error) {
                pool.record_failure_for(upstream_name, capability, message.clone())
                    .await;
                if let Some(subj) = subject {
                    pool.evict_subject_connection(upstream_name, subj).await;
                }
            } else {
                // A valid MCP error response confirms the connection is alive.
                pool.record_success_for(upstream_name, capability).await;
            }
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "upstream_error",
                Some(&error),
                None,
                None,
            );
            record_usage_call(
                pool,
                event,
                subject,
                "upstream_error",
                start.elapsed().as_millis(),
            );
            Err(CapabilityCallError::from_service_error(error, message))
        }
        RawCallOutcome::Timeout => {
            pool.record_failure_for(upstream_name, capability, timeout_message.clone())
                .await;
            if let Some(subj) = subject {
                pool.evict_subject_connection(upstream_name, subj).await;
            }
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "timeout",
                None,
                None,
                None,
            );
            record_usage_call(pool, event, subject, "timeout", start.elapsed().as_millis());
            Err(CapabilityCallError::Timeout {
                message: timeout_message,
            })
        }
    }
}
