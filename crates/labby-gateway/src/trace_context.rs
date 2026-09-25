//! Shared rmcp integration for Labby's W3C / MCP SEP-414 trace contract.
//!
//! Both the direct MCP proxy and Code Mode call this module so metadata merge,
//! child-span generation, and private correlation semantics cannot drift.

use std::future::Future;
use std::pin::Pin;

use labby_primitives::trace::{
    LabbyTraceCorrelation, MCP_BAGGAGE_META_KEY, MCP_LABBY_TRACE_META_KEY, MCP_TRACESTATE_META_KEY,
    TraceContext, TraceContextError,
};
use rmcp::model::{CallToolRequestParams, RequestMetaObject};
use tracing::Instrument as _;

/// Inject one outbound child context into an rmcp `tools/call` request.
///
/// Existing non-trace metadata is preserved. Standard SEP-414 fields are
/// replaced with the canonical child values, and absent optional fields are
/// removed so stale caller-supplied values cannot survive a newly minted
/// parent. Labby correlation is host-owned and likewise replaced/removed here.
///
/// The returned context represents the outbound operation and is suitable for
/// structured-log/span correlation. A transport retry of the same logical
/// operation should reuse the already-prepared request rather than call this
/// function again, thereby preserving its child span id.
pub fn inject_outbound_tool_trace(
    params: &mut CallToolRequestParams,
    parent: &TraceContext,
    correlation: Option<&LabbyTraceCorrelation>,
) -> Result<TraceContext, TraceContextError> {
    let child = parent.child()?;
    let meta = params.meta.get_or_insert_with(RequestMetaObject::new);

    meta.set_traceparent(child.traceparent().to_header_value());
    match child.tracestate() {
        Some(tracestate) => meta.set_tracestate(tracestate.as_str()),
        None => {
            meta.remove(MCP_TRACESTATE_META_KEY);
        }
    }
    match child.baggage() {
        Some(baggage) => meta.set_baggage(baggage.as_str()),
        None => {
            meta.remove(MCP_BAGGAGE_META_KEY);
        }
    }
    match correlation {
        Some(correlation) => {
            meta.insert(
                MCP_LABBY_TRACE_META_KEY.to_string(),
                correlation.to_meta_value(),
            );
        }
        None => {
            meta.remove(MCP_LABBY_TRACE_META_KEY);
        }
    }

    Ok(child)
}

/// Type-erase an outbound call's instrumentation so adding trace spans does
/// not inflate already-large gateway async state-machine types.
///
/// This is deliberately shared with the metadata injector: the exact child
/// context sent on the wire is also the context attached to structured logs.
pub fn instrument_outbound_future<'a, F, T>(
    future: F,
    trace: Option<&TraceContext>,
    correlation: Option<&LabbyTraceCorrelation>,
) -> Pin<Box<dyn Future<Output = T> + Send + 'a>>
where
    F: Future<Output = T> + Send + 'a,
    T: 'a,
{
    let Some(trace) = trace else {
        return Box::pin(future);
    };

    let span = if let Some(correlation) = correlation {
        tracing::info_span!(
            "gateway.upstream",
            trace_id = %trace.trace_id().to_hex(),
            span_id = %trace.span_id().to_hex(),
            execution_id = correlation.execution_id(),
            call_ordinal = correlation.call_ordinal(),
        )
    } else {
        tracing::info_span!(
            "gateway.upstream",
            trace_id = %trace.trace_id().to_hex(),
            span_id = %trace.span_id().to_hex(),
        )
    };
    Box::pin(future.instrument(span))
}

#[cfg(test)]
mod tests {
    use labby_primitives::trace::{
        Baggage, LabbyTraceCorrelation, MCP_LABBY_TRACE_META_KEY, TraceContext, TraceParent,
        TraceState,
    };
    use rmcp::model::{CallToolRequestParams, RequestMetaObject};
    use serde_json::json;

    use super::*;

    const PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

    fn continued_context() -> TraceContext {
        TraceContext::continue_remote(
            TraceParent::parse(PARENT).expect("valid parent"),
            Some(TraceState::parse("vendor=value").expect("valid tracestate")),
            Some(Baggage::parse("userId=alice").expect("valid baggage")),
        )
        .expect("continued context")
    }

    #[test]
    fn inject_preserves_unrelated_meta_and_replaces_trace_fields() {
        let mut meta = RequestMetaObject::new();
        meta.insert("custom".to_string(), json!({"kept": true}));
        meta.set_traceparent("00-11111111111111111111111111111111-2222222222222222-00");
        meta.set_tracestate("stale=value");
        meta.set_baggage("stale=value");

        let mut request = CallToolRequestParams::new("echo");
        request.meta = Some(meta);
        let parent = continued_context();

        let child = inject_outbound_tool_trace(&mut request, &parent, None).unwrap();
        let meta = request.meta.as_ref().expect("meta");
        assert_eq!(meta.get("custom"), Some(&json!({"kept": true})));
        assert_eq!(
            meta.get_traceparent(),
            Some(child.traceparent().to_header_value().as_str())
        );
        assert_eq!(meta.get_tracestate(), Some("vendor=value"));
        assert_eq!(meta.get_baggage(), Some("userId=alice"));
        assert!(meta.get(MCP_LABBY_TRACE_META_KEY).is_none());
        assert_eq!(child.trace_id(), parent.trace_id());
        assert_ne!(child.span_id(), parent.span_id());
    }

    #[test]
    fn inject_removes_stale_optional_fields_and_adds_host_correlation() {
        let mut meta = RequestMetaObject::new();
        meta.set_tracestate("stale=value");
        meta.set_baggage("stale=value");
        meta.insert(
            MCP_LABBY_TRACE_META_KEY.to_string(),
            json!({"execution_id": "forged", "call_ordinal": 999}),
        );

        let mut request = CallToolRequestParams::new("echo");
        request.meta = Some(meta);
        let parent = TraceContext::fresh(0).expect("fresh parent");
        let correlation = LabbyTraceCorrelation::new("exec_real", 4).unwrap();

        inject_outbound_tool_trace(&mut request, &parent, Some(&correlation)).unwrap();
        let meta = request.meta.as_ref().expect("meta");
        assert!(meta.get_tracestate().is_none());
        assert!(meta.get_baggage().is_none());
        assert_eq!(
            meta.get(MCP_LABBY_TRACE_META_KEY),
            Some(&json!({"execution_id": "exec_real", "call_ordinal": 4}))
        );
    }

    #[test]
    fn retry_reuses_prepared_child_context_instead_of_minting_again() {
        let parent = continued_context();
        let mut request = CallToolRequestParams::new("echo");
        let child = inject_outbound_tool_trace(&mut request, &parent, None).unwrap();
        let first = request.clone();
        let retry = request;

        assert_eq!(
            first.meta.as_ref().and_then(|meta| meta.get_traceparent()),
            retry.meta.as_ref().and_then(|meta| meta.get_traceparent())
        );
        assert_eq!(
            first.meta.as_ref().and_then(|meta| meta.get_traceparent()),
            Some(child.traceparent().to_header_value().as_str())
        );
    }
}
