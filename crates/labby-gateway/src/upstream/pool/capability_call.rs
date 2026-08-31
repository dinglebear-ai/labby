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

use serde_json::Value;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::helpers::max_response_bytes;
use super::logging::{
    UpstreamRequestLog, log_upstream_request_cancelled, log_upstream_request_error,
    log_upstream_request_finish,
};
use super::usage_record::{record_usage_call, record_usage_call_with_response};

/// Structured failure from an upstream capability call.
///
/// Most gateway surfaces preserve their historical string errors by calling
/// `to_string()`. Code Mode and the MCP upstream proxy
/// (`crates/labby/src/mcp/call_tool_upstream.rs`) consume this typed form so
/// JSON-RPC/MCP error codes survive the pool boundary instead of collapsing
/// into `upstream_error` — and so callers can tell "the upstream rejected the
/// request over a healthy connection" (`Mcp`) apart from transport-class
/// failures the pool already recorded against the circuit breaker.
#[derive(Debug)]
pub enum CapabilityCallError {
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
    /// The upstream (or the local service layer) reported the request was
    /// cancelled. Non-retryable: the caller asked for the cancellation or the
    /// work was abandoned deliberately.
    Cancelled {
        message: String,
    },
    /// The upstream kept returning `input_required` past the MRTR round cap.
    /// Surfaced as `confirmation_required` so agents know an interactive
    /// confirmation loop, not infrastructure, blocked the call.
    InputRequiredRoundsExceeded {
        message: String,
    },
    Other {
        message: String,
    },
}

impl CapabilityCallError {
    pub(super) fn from_service_error(error: rmcp::ServiceError, message: String) -> Self {
        match error {
            rmcp::ServiceError::McpError(data) => Self::Mcp { data, message },
            rmcp::ServiceError::Timeout { .. } => Self::Timeout { message },
            rmcp::ServiceError::TransportSend(_)
            | rmcp::ServiceError::TransportClosed
            | rmcp::ServiceError::SubscriptionLagged { .. } => Self::Transport { message },
            rmcp::ServiceError::UnexpectedResponse => Self::Protocol { message },
            rmcp::ServiceError::Cancelled { .. } => Self::Cancelled { message },
            rmcp::ServiceError::InputRequiredRoundsExceeded { .. } => {
                Self::InputRequiredRoundsExceeded { message }
            }
            // `ServiceError` is `#[non_exhaustive]`; future variants fall back
            // to the generic upstream-error shape.
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
            | Self::Cancelled { message }
            | Self::InputRequiredRoundsExceeded { message }
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

/// Char cap for the upstream-authored JSON-RPC error message retained on
/// [`CapabilityCallError::Mcp`], embedded in stringified surface errors, and
/// interpolated into dispatch logs.
const UPSTREAM_ERROR_MESSAGE_CAP_CHARS: usize = 4096;

/// Byte cap for the upstream-authored `ErrorData.data` payload retained on
/// [`CapabilityCallError::Mcp`]. Matches the Code Mode consumer's
/// `UPSTREAM_ERROR_DATA_CAP_BYTES` (`code_mode_host.rs`) so the downstream
/// re-redaction is idempotent rather than double-truncating.
const UPSTREAM_ERROR_DATA_CAP_BYTES: usize = 2048;

/// Char cap for the `kind` hint carried across an over-cap `data` replacement.
/// Stable kinds are short identifiers; anything longer is not in the downstream
/// allowlist anyway.
const UPSTREAM_ERROR_KIND_CAP_CHARS: usize = 64;

/// Bound the upstream-controlled fields of a JSON-RPC error before it is
/// logged or stored.
///
/// `ErrorData`'s `Display` interpolates both `message` and the full serialized
/// `data` value, so an unbounded upstream error would otherwise flow multi-MB
/// payloads into every log line, stringified surface error, and the retained
/// [`CapabilityCallError::Mcp`] enum. Small payloads pass through byte-for-byte
/// unchanged — this is a size guard at the pool boundary, not a redaction pass;
/// consumer-boundary sanitization (secret redaction, prompt-marker stripping)
/// still happens in `code_mode_host.rs` / `tool_error.rs`.
pub(super) fn bound_upstream_service_error(error: rmcp::ServiceError) -> rmcp::ServiceError {
    match error {
        rmcp::ServiceError::McpError(data) => {
            rmcp::ServiceError::McpError(bound_upstream_error_data(data))
        }
        other => other,
    }
}

/// Bound an upstream error that is about to be *stringified* rather than
/// retained as structured data.
///
/// The fan-out list passes and the relay path stringify `ServiceError` straight
/// into `record_failure_for` (stored as `*_last_error` and surfaced verbatim by
/// `gateway.status`) and into `warn!` lines. `ErrorData`'s `Display`
/// interpolates the full serialized `data`, so an upstream failing
/// `prompts/list` with a multi-MB error would otherwise wedge that payload into
/// every subsequent status response. The transient `to_string` allocation is
/// unavoidable without deeper rmcp surgery; what matters is that nothing
/// oversized is stored or logged.
pub(super) fn bounded_service_error_text(error: &rmcp::ServiceError) -> String {
    let text = error.to_string();
    if text.len() > UPSTREAM_ERROR_MESSAGE_CAP_CHARS
        && text.chars().count() > UPSTREAM_ERROR_MESSAGE_CAP_CHARS
    {
        labby_runtime::agent_error::sanitize_error_text(&text, UPSTREAM_ERROR_MESSAGE_CAP_CHARS)
    } else {
        text
    }
}

fn bound_upstream_error_data(mut data: rmcp::model::ErrorData) -> rmcp::model::ErrorData {
    // Byte length is a cheap upper bound on char count: only scan and rewrite
    // when the message can actually exceed the cap.
    if data.message.len() > UPSTREAM_ERROR_MESSAGE_CAP_CHARS
        && data.message.chars().count() > UPSTREAM_ERROR_MESSAGE_CAP_CHARS
    {
        data.message = labby_runtime::agent_error::sanitize_error_text(
            &data.message,
            UPSTREAM_ERROR_MESSAGE_CAP_CHARS,
        )
        .into();
    }
    if let Some(payload) = data.data.as_ref() {
        let serialized_len = serde_json::to_vec(payload).map_or(usize::MAX, |bytes| bytes.len());
        if serialized_len > UPSTREAM_ERROR_DATA_CAP_BYTES {
            let mut bounded =
                labby_codemode::redact_trace_value(payload, UPSTREAM_ERROR_DATA_CAP_BYTES);
            // `redact_trace_value` replaces an over-cap payload wholesale with a
            // truncation stub, which would silently drop the `kind` hint that
            // `code_mode_mcp_error_kind` classifies on — an over-cap
            // `unknown_action` would degrade to a generic `upstream_error` and
            // hand the agent the wrong recovery action. Carry that one small
            // scalar across; it is allowlist-validated downstream, so a hostile
            // value cannot invent a kind.
            if let (Some(stub), Some(original)) = (bounded.as_object_mut(), payload.as_object())
                && let Some(kind) = original.get("kind").and_then(Value::as_str)
            {
                stub.insert(
                    "kind".to_string(),
                    Value::String(kind.chars().take(UPSTREAM_ERROR_KIND_CAP_CHARS).collect()),
                );
            }
            data.data = Some(bounded);
        }
    }
    data
}

/// Outcome of a timed capability call before size-cap enforcement.
pub(super) enum RawCallOutcome<R> {
    Ok(R),
    /// The upstream returned an error (not a timeout).
    UpstreamError(rmcp::ServiceError),
    /// The tokio timeout elapsed.
    Timeout,
    /// The caller's cancellation token fired before the upstream responded.
    ///
    /// Distinct from [`Self::Timeout`]: nothing is wrong with the upstream, so
    /// this must not feed the circuit breaker or evict the connection.
    Cancelled,
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
/// - `error_message_fn` — builds the user-visible error string from the typed
///   upstream error.
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
    error_message_fn: impl Fn(&rmcp::ServiceError) -> String,
    timeout_message: String,
) -> Result<R, CapabilityCallError>
where
    Fut: Future<Output = Result<R, rmcp::ServiceError>>,
    SizeFn: Fn(&R) -> usize,
{
    timed_capability_call_with_timeout(
        pool,
        pool.request_timeout,
        upstream_name,
        capability,
        event,
        start,
        rpc_future,
        size_fn,
        subject,
        error_message_fn,
        timeout_message,
        None,
    )
    .await
}

/// Variant of [`timed_capability_call`] for surfaces with a stricter local
/// budget than the general upstream request timeout.
#[allow(clippy::too_many_arguments)]
pub(super) async fn timed_capability_call_with_timeout<R, Fut, SizeFn>(
    pool: &UpstreamPool,
    request_timeout: std::time::Duration,
    upstream_name: &str,
    capability: UpstreamCapability,
    event: UpstreamRequestLog<'_>,
    start: Instant,
    rpc_future: Fut,
    size_fn: SizeFn,
    subject: Option<&str>,
    error_message_fn: impl Fn(&rmcp::ServiceError) -> String,
    timeout_message: String,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<R, CapabilityCallError>
where
    Fut: Future<Output = Result<R, rmcp::ServiceError>>,
    SizeFn: Fn(&R) -> usize,
{
    // Enforce one wall-clock budget across peer acquisition, bulkhead wait, and
    // the RPC itself. Waiting for a permit must not extend the configured
    // upstream timeout, and queue pressure must not poison connection health.
    let gate_remaining = request_timeout.saturating_sub(start.elapsed());
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
    // Queueing behind the bulkhead is the one place a caller can wait without
    // the upstream having been contacted yet, so honour cancellation here too
    // — the relay path already does (`RelayPermitOutcome::Cancelled`). Nothing
    // needs telling upstream: no request has been sent.
    let permit_wait = tokio::time::timeout(
        gate_remaining,
        pool.acquire_upstream_call_permit(upstream_name),
    );
    let permit_outcome = match cancel {
        Some(token) => tokio::select! {
            biased;
            () = token.cancelled() => {
                log_upstream_request_cancelled(event, start.elapsed().as_millis(), "cancelled");
                record_usage_call(pool, event, subject, "cancelled", start.elapsed().as_millis());
                return Err(CapabilityCallError::Cancelled {
                    message: format!(
                        "caller cancelled the `{upstream_name}` request while queued"
                    ),
                });
            }
            outcome = permit_wait => outcome,
        },
        None => permit_wait.await,
    };
    let _permit = match permit_outcome {
        Ok(Ok(permit)) => permit,
        Ok(Err(error)) => {
            // The per-upstream concurrency gate itself failed (semaphore
            // closed). Emit the same telemetry as the sibling saturation
            // branches and surface it as `queue_saturated` — the caller-facing
            // fact is identical: the local concurrency gate, not the upstream,
            // refused the call.
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "queue_saturated",
                Some(&error),
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
                    "upstream `{upstream_name}` concurrency gate unavailable: {error}"
                ),
            });
        }
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
    let rpc_remaining = request_timeout.saturating_sub(start.elapsed());
    let outcome = if rpc_remaining.is_zero() {
        RawCallOutcome::Timeout
    } else {
        let rpc = tokio::time::timeout(rpc_remaining, rpc_future);
        match cancel {
            // Dropping `rpc` here is what stops the local work. Capability
            // futures that carry side effects upstream arm their own
            // cancel-on-drop guard so the upstream is told to stop too.
            //
            // `biased` is load-bearing, not style. `rpc` is lazy: nothing
            // reaches the wire until it is polled. Polling the token first
            // means an already-cancelled caller never dispatches the request
            // at all. Without `biased` tokio picks at random and roughly half
            // the time writes the request out before noticing — executing a
            // side effect for a caller that is already gone. Pinned by
            // `a_call_cancelled_before_dispatch_never_reaches_the_upstream`.
            // Losing a tie to an already-arrived response is harmless: that
            // result had no reader, and MCP receivers ignore unknown ids.
            Some(token) => tokio::select! {
                biased;
                () = token.cancelled() => RawCallOutcome::Cancelled,
                result = rpc => classify_timeout_result(result),
            },
            None => classify_timeout_result(rpc.await),
        }
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
                record_usage_call_with_response(
                    pool,
                    event,
                    subject,
                    "response_too_large",
                    start.elapsed().as_millis(),
                    Some(response_size),
                );
                return Err(CapabilityCallError::ResponseTooLarge {
                    message: format!(
                        "upstream response too large ({response_size} bytes, max {max_bytes})"
                    ),
                });
            }
            pool.record_success_for(upstream_name, capability).await;
            log_upstream_request_finish(event, start.elapsed().as_millis(), Some(response_size));
            record_usage_call_with_response(
                pool,
                event,
                subject,
                "ok",
                start.elapsed().as_millis(),
                Some(response_size),
            );
            Ok(result)
        }
        RawCallOutcome::UpstreamError(error) => {
            // Bound upstream-authored error payloads (JSON-RPC message + data)
            // before they reach `error_message_fn`, dispatch logs, or the
            // retained error enum — an upstream must not be able to push a
            // multi-MB error body through the gateway's error plumbing.
            let error = bound_upstream_service_error(error);
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
        RawCallOutcome::Cancelled => {
            // No circuit-breaker failure and no eviction: the caller withdrew,
            // the upstream did nothing wrong, and the connection is still good.
            // Recording this as a failure would let a client that disconnects
            // repeatedly quarantine a perfectly healthy upstream.
            //
            // This is only safe because the HTTP transport backstop is derived
            // to exceed the upstream deadline (`LabConfig::http_request_timeout`),
            // so a genuinely wedged upstream still trips the breaker via
            // `Timeout` before any caller's transport gives up. A fixed
            // transport cap *below* the upstream deadline — the bug this change
            // followed — would make every caller withdraw first and blind the
            // breaker entirely. Pinned by
            // `http_request_timeout_never_undercuts_configured_upstream_deadlines`.
            log_upstream_request_cancelled(event, start.elapsed().as_millis(), "cancelled");
            record_usage_call(
                pool,
                event,
                subject,
                "cancelled",
                start.elapsed().as_millis(),
            );
            Err(CapabilityCallError::Cancelled {
                message: format!("caller cancelled the `{upstream_name}` request"),
            })
        }
    }
}

/// String-error form of [`timed_capability_call`].
///
/// Accepted debt: the non-Code-Mode gateway surfaces (tool/prompt/resource/
/// completion proxying) historically expose `Result<_, String>` and are
/// intentionally string-preserving. This wrapper is the single place that
/// collapses the typed [`CapabilityCallError`] into that string form so call
/// sites do not each repeat `.map_err(|error| error.to_string())`. New
/// consumers that need the failure *class* (Code Mode does) must call
/// [`timed_capability_call`] directly instead of stringifying.
#[allow(clippy::too_many_arguments)]
pub(super) async fn timed_capability_call_str<R, Fut, SizeFn>(
    pool: &UpstreamPool,
    upstream_name: &str,
    capability: UpstreamCapability,
    event: UpstreamRequestLog<'_>,
    start: Instant,
    rpc_future: Fut,
    size_fn: SizeFn,
    subject: Option<&str>,
    error_message_fn: impl Fn(&rmcp::ServiceError) -> String,
    timeout_message: String,
) -> Result<R, String>
where
    Fut: Future<Output = Result<R, rmcp::ServiceError>>,
    SizeFn: Fn(&R) -> usize,
{
    timed_capability_call(
        pool,
        upstream_name,
        capability,
        event,
        start,
        rpc_future,
        size_fn,
        subject,
        error_message_fn,
        timeout_message,
    )
    .await
    .map_err(|error| error.to_string())
}

#[cfg(test)]
// `panic!` is how tests assert; `panic = "warn"` targets production paths.
#[allow(clippy::panic)]
mod tests {
    use labby_runtime::agent_error::SANITIZE_TRUNCATION_MARKER;
    use rmcp::model::{ErrorCode, ErrorData};

    use super::*;

    #[test]
    fn bound_upstream_error_data_caps_multi_mb_message_and_data() {
        let huge = "x".repeat(3 * 1024 * 1024);
        let error = rmcp::ServiceError::McpError(ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            huge.clone(),
            Some(serde_json::json!({ "detail": huge })),
        ));

        let rmcp::ServiceError::McpError(data) = bound_upstream_service_error(error) else {
            panic!("McpError variant must be preserved");
        };

        assert!(
            data.message.chars().count()
                <= UPSTREAM_ERROR_MESSAGE_CAP_CHARS + SANITIZE_TRUNCATION_MARKER.len(),
            "message must be bounded, got {} chars",
            data.message.chars().count()
        );
        assert!(data.message.ends_with(SANITIZE_TRUNCATION_MARKER));
        let serialized_data =
            serde_json::to_vec(&data.data).expect("bounded data payload serializes");
        assert!(
            serialized_data.len() <= UPSTREAM_ERROR_DATA_CAP_BYTES,
            "data payload must be bounded, got {} bytes",
            serialized_data.len()
        );
        // What surfaces/logs actually interpolate — the full `Display` — is
        // bounded too (message + data both flow through `ErrorData::fmt`).
        let display = rmcp::ServiceError::McpError(data).to_string();
        assert!(
            display.len() < 16 * 1024,
            "stringified error must be bounded, got {} bytes",
            display.len()
        );
    }

    #[test]
    fn bound_upstream_error_data_preserves_the_classification_kind() {
        // `redact_trace_value` replaces an over-cap payload wholesale, so the
        // `kind` hint the Code Mode classifier reads must be carried across
        // explicitly — otherwise an over-cap `unknown_action` silently degrades
        // to the ErrorCode-derived kind and the agent gets the wrong recovery.
        let huge_valid_actions: Vec<String> =
            (0..500).map(|i| format!("service.action_{i}")).collect();
        let error = rmcp::ServiceError::McpError(ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            "unknown action `service.typo`",
            Some(serde_json::json!({
                "kind": "unknown_action",
                "valid": huge_valid_actions,
            })),
        ));

        let rmcp::ServiceError::McpError(data) = bound_upstream_service_error(error) else {
            panic!("McpError variant must be preserved");
        };

        let serialized_data =
            serde_json::to_vec(&data.data).expect("bounded data payload serializes");
        assert!(
            serialized_data.len() <= UPSTREAM_ERROR_DATA_CAP_BYTES,
            "payload must still be bounded, got {} bytes",
            serialized_data.len()
        );
        assert_eq!(
            data.data
                .as_ref()
                .and_then(|value| value.get("kind"))
                .and_then(Value::as_str),
            Some("unknown_action"),
            "the classification kind must survive the size bound"
        );
    }

    #[test]
    fn bound_upstream_error_data_leaves_small_payloads_unchanged() {
        let payload = serde_json::json!({
            "kind": "forbidden",
            "required_scopes": ["example:write"],
        });
        let error = rmcp::ServiceError::McpError(ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            "forbidden: requires scope: example:write",
            Some(payload.clone()),
        ));

        let rmcp::ServiceError::McpError(data) = bound_upstream_service_error(error) else {
            panic!("McpError variant must be preserved");
        };

        assert_eq!(data.message, "forbidden: requires scope: example:write");
        assert_eq!(data.data, Some(payload));
    }

    #[test]
    fn bound_upstream_service_error_passes_non_mcp_errors_through() {
        let bounded = bound_upstream_service_error(rmcp::ServiceError::TransportClosed);
        assert!(matches!(bounded, rmcp::ServiceError::TransportClosed));
    }
}
