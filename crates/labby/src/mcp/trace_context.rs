//! Product policy for inbound MCP SEP-414 trace metadata.
//!
//! Parsing/generation lives in `labby-primitives`; this module owns Labby's
//! availability-oriented inbound policy. Trace metadata is untrusted
//! observability input and never participates in authorization.

use std::sync::Arc;

use labby_primitives::trace::{Baggage, TraceContext, TraceContextError, TraceParent, TraceState};
use rmcp::model::RequestMetaObject;

/// How Labby established the local request trace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InboundTraceSource {
    /// A valid inbound W3C parent was continued.
    Continued,
    /// No valid parent was supplied, so Labby minted a new trace.
    Minted,
}

/// Result of applying Labby's bounded inbound trace policy.
#[derive(Clone, Debug)]
pub(crate) struct InboundTrace {
    pub(crate) context: Arc<TraceContext>,
    pub(crate) source: InboundTraceSource,
    pub(crate) invalid_parent: bool,
    pub(crate) dropped_tracestate: bool,
    pub(crate) dropped_baggage: bool,
}

/// Server-owned request trace stored in rmcp request extensions.
///
/// The wrapper prevents arbitrary transport metadata from being mistaken for
/// trusted trace state. Only the call boundary inserts this type.
#[derive(Clone, Debug)]
pub(crate) struct RequestTraceContext(pub(crate) Arc<TraceContext>);

/// Read the validated host-owned request trace from rmcp extensions.
pub(crate) fn request_trace_context(
    extensions: &rmcp::model::Extensions,
) -> Option<Arc<TraceContext>> {
    extensions
        .get::<RequestTraceContext>()
        .map(|trace| Arc::clone(&trace.0))
}

/// Validate/continue an inbound context or mint a fresh request trace.
///
/// Policy:
/// - a valid `traceparent` is continued and preserves its trace flags;
/// - an absent parent mints an unsampled local trace;
/// - a malformed/unsupported parent mints a new trace and its associated
///   `tracestate`/`baggage` are not propagated;
/// - malformed/oversized optional fields are dropped independently while a
///   valid parent continues.
///
/// This intentionally fails open on untrusted optional observability metadata:
/// malformed baggage must never deny an otherwise authorized tool call.
pub(crate) fn resolve_inbound_trace(
    meta: Option<&RequestMetaObject>,
) -> Result<InboundTrace, TraceContextError> {
    let Some(meta) = meta else {
        return mint(false);
    };
    let Some(raw_parent) = meta.get_traceparent() else {
        return mint(false);
    };

    let parent = match TraceParent::parse(raw_parent) {
        Ok(parent) => parent,
        Err(_) => return mint(true),
    };

    let (tracestate, dropped_tracestate) = match meta.get_tracestate() {
        None => (None, false),
        Some(value) => match TraceState::parse(value) {
            Ok(value) => (Some(value), false),
            Err(_) => (None, true),
        },
    };
    let (baggage, dropped_baggage) = match meta.get_baggage() {
        None => (None, false),
        Some(value) => match Baggage::parse(value) {
            Ok(value) => (Some(value), false),
            Err(_) => (None, true),
        },
    };

    let context = TraceContext::continue_remote(parent, tracestate, baggage)?;
    let result = InboundTrace {
        context: Arc::new(context),
        source: InboundTraceSource::Continued,
        invalid_parent: false,
        dropped_tracestate,
        dropped_baggage,
    };
    log_policy_result(&result);
    Ok(result)
}

fn mint(invalid_parent: bool) -> Result<InboundTrace, TraceContextError> {
    let result = InboundTrace {
        context: Arc::new(TraceContext::fresh(0)?),
        source: InboundTraceSource::Minted,
        invalid_parent,
        // Optional fields without a valid parent are intentionally not
        // propagated, but absence/stranding is not itself a parse failure.
        dropped_tracestate: false,
        dropped_baggage: false,
    };
    log_policy_result(&result);
    Ok(result)
}

fn log_policy_result(result: &InboundTrace) {
    if result.invalid_parent || result.dropped_tracestate || result.dropped_baggage {
        tracing::debug!(
            surface = "mcp",
            service = "trace_context",
            action = "inbound.validate",
            continued = matches!(result.source, InboundTraceSource::Continued),
            invalid_parent = result.invalid_parent,
            dropped_tracestate = result.dropped_tracestate,
            dropped_baggage = result.dropped_baggage,
            "sanitized untrusted inbound MCP trace metadata"
        );
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::RequestMetaObject;

    use super::*;

    const VALID_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

    #[test]
    fn missing_parent_mints_new_trace_and_ignores_stray_optional_fields() {
        let mut meta = RequestMetaObject::new();
        meta.set_tracestate("vendor=value");
        meta.set_baggage("userId=alice");

        let resolved = resolve_inbound_trace(Some(&meta)).unwrap();
        assert_eq!(resolved.source, InboundTraceSource::Minted);
        assert!(!resolved.invalid_parent);
        assert!(resolved.context.tracestate().is_none());
        assert!(resolved.context.baggage().is_none());
    }

    #[test]
    fn valid_parent_continues_trace_and_valid_optional_fields() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent(VALID_PARENT);
        meta.set_tracestate("vendor=value");
        meta.set_baggage("userId=alice");

        let resolved = resolve_inbound_trace(Some(&meta)).unwrap();
        let parent = TraceParent::parse(VALID_PARENT).unwrap();
        assert_eq!(resolved.source, InboundTraceSource::Continued);
        assert_eq!(resolved.context.trace_id(), parent.trace_id());
        assert_eq!(resolved.context.trace_flags(), parent.trace_flags());
        assert_eq!(
            resolved.context.tracestate().map(TraceState::as_str),
            Some("vendor=value")
        );
        assert_eq!(
            resolved.context.baggage().map(Baggage::as_str),
            Some("userId=alice")
        );
        assert!(!resolved.dropped_tracestate);
        assert!(!resolved.dropped_baggage);
    }

    #[test]
    fn invalid_parent_mints_and_drops_associated_optional_fields() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent("00-00000000000000000000000000000000-b7ad6b7169203331-01");
        meta.set_tracestate("vendor=value");
        meta.set_baggage("userId=alice");

        let resolved = resolve_inbound_trace(Some(&meta)).unwrap();
        assert_eq!(resolved.source, InboundTraceSource::Minted);
        assert!(resolved.invalid_parent);
        assert!(resolved.context.tracestate().is_none());
        assert!(resolved.context.baggage().is_none());
    }

    #[test]
    fn invalid_optional_fields_are_dropped_without_rejecting_valid_parent() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent(VALID_PARENT);
        meta.set_tracestate("Upper=value");
        meta.set_baggage(
            "safe=value
trusted=true",
        );

        let resolved = resolve_inbound_trace(Some(&meta)).unwrap();
        assert_eq!(resolved.source, InboundTraceSource::Continued);
        assert!(!resolved.invalid_parent);
        assert!(resolved.dropped_tracestate);
        assert!(resolved.dropped_baggage);
        assert!(resolved.context.tracestate().is_none());
        assert!(resolved.context.baggage().is_none());
    }

    #[test]
    fn oversized_optional_fields_are_dropped_not_forwarded() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent(VALID_PARENT);
        meta.set_tracestate(format!("a={}", "x".repeat(600)));
        meta.set_baggage(format!("a={}", "x".repeat(9000)));

        let resolved = resolve_inbound_trace(Some(&meta)).unwrap();
        assert_eq!(resolved.source, InboundTraceSource::Continued);
        assert!(resolved.dropped_tracestate);
        assert!(resolved.dropped_baggage);
    }
}
