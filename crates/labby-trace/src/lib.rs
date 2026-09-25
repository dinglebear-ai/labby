//! Bounded W3C Trace Context helpers for MCP SEP-414.
//!
//! The standard MCP keys remain the unprefixed `traceparent`,
//! `tracestate`, and `baggage` fields. Labby-only correlation data is
//! carried separately under a namespaced extension and is observability-only.

use rmcp::model::RequestMetaObject;
use serde::{Deserialize, Serialize};

pub const LABBY_CORRELATION_META_KEY: &str = "ai.dinglebear.labby/trace";
pub const MAX_TRACESTATE_BYTES: usize = 512;
pub const MAX_TRACESTATE_MEMBERS: usize = 32;
pub const MAX_BAGGAGE_BYTES: usize = 8192;
pub const MAX_BAGGAGE_MEMBERS: usize = 64;
pub const MAX_EXECUTION_ID_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub flags: u8,
    pub tracestate: Option<String>,
    pub baggage: Option<String>,
    pub correlation: Option<LabbyCorrelation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabbyCorrelation {
    pub execution_id: String,
    pub call_ordinal: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TraceContextError {
    #[error("invalid traceparent")]
    InvalidTraceparent,
    #[error("invalid tracestate")]
    InvalidTracestate,
    #[error("invalid baggage")]
    InvalidBaggage,
    #[error("invalid Labby trace correlation")]
    InvalidCorrelation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundTraceDisposition {
    Continued,
    Created,
    ReplacedInvalid,
}

impl TraceContext {
    #[must_use]
    pub fn continue_or_new(
        meta: Option<&RequestMetaObject>,
    ) -> (Self, InboundTraceDisposition) {
        let Some(meta) = meta else {
            return (Self::new_root(), InboundTraceDisposition::Created);
        };
        match Self::from_meta(meta) {
            Ok(Some(context)) => (context, InboundTraceDisposition::Continued),
            Ok(None) => (Self::new_root(), InboundTraceDisposition::Created),
            Err(_) => (Self::new_root(), InboundTraceDisposition::ReplacedInvalid),
        }
    }

    #[must_use]
    pub fn without_correlation(mut self) -> Self {
        self.correlation = None;
        self
    }
    #[must_use]
    pub fn new_root() -> Self {
        Self {
            trace_id: random_nonzero_u128().to_be_bytes(),
            span_id: random_nonzero_u64().to_be_bytes(),
            // Propagation and sampling are intentionally independent. A newly
            // minted context is unsampled unless a later policy explicitly
            // opts it into sampling/export.
            flags: 0,
            tracestate: None,
            baggage: None,
            correlation: None,
        }
    }

    pub fn from_meta(meta: &RequestMetaObject) -> Result<Option<Self>, TraceContextError> {
        let Some(traceparent) = meta.get_traceparent() else {
            // tracestate/baggage have no meaning without a valid parent and
            // must never be continued independently.
            return Ok(None);
        };
        let (trace_id, span_id, flags) = parse_traceparent(traceparent)?;

        let tracestate = meta.get_tracestate().map(ToOwned::to_owned);
        if let Some(value) = tracestate.as_deref() {
            validate_list(
                value,
                MAX_TRACESTATE_BYTES,
                MAX_TRACESTATE_MEMBERS,
                TraceContextError::InvalidTracestate,
            )?;
        }

        let baggage = meta.get_baggage().map(ToOwned::to_owned);
        if let Some(value) = baggage.as_deref() {
            validate_list(
                value,
                MAX_BAGGAGE_BYTES,
                MAX_BAGGAGE_MEMBERS,
                TraceContextError::InvalidBaggage,
            )?;
        }

        let correlation = match meta.0.0.get(LABBY_CORRELATION_META_KEY) {
            Some(value) => {
                let correlation: LabbyCorrelation = serde_json::from_value(value.clone())
                    .map_err(|_| TraceContextError::InvalidCorrelation)?;
                validate_correlation(&correlation)?;
                Some(correlation)
            }
            None => None,
        };

        Ok(Some(Self {
            trace_id,
            span_id,
            flags,
            tracestate,
            baggage,
            correlation,
        }))
    }

    pub fn inject(&self, meta: &mut RequestMetaObject) -> Result<(), TraceContextError> {
        if self.trace_id == [0; 16] || self.span_id == [0; 8] {
            return Err(TraceContextError::InvalidTraceparent);
        }
        if let Some(value) = self.tracestate.as_deref() {
            validate_list(
                value,
                MAX_TRACESTATE_BYTES,
                MAX_TRACESTATE_MEMBERS,
                TraceContextError::InvalidTracestate,
            )?;
        }
        if let Some(value) = self.baggage.as_deref() {
            validate_list(
                value,
                MAX_BAGGAGE_BYTES,
                MAX_BAGGAGE_MEMBERS,
                TraceContextError::InvalidBaggage,
            )?;
        }
        if let Some(correlation) = self.correlation.as_ref() {
            validate_correlation(correlation)?;
        }

        meta.set_traceparent(self.traceparent());
        match self.tracestate.as_ref() {
            Some(value) => meta.set_tracestate(value.clone()),
            None => {
                meta.0.0.remove("tracestate");
            }
        }
        match self.baggage.as_ref() {
            Some(value) => meta.set_baggage(value.clone()),
            None => {
                meta.0.0.remove("baggage");
            }
        }
        match self.correlation.as_ref() {
            Some(correlation) => {
                let value = serde_json::to_value(correlation)
                    .map_err(|_| TraceContextError::InvalidCorrelation)?;
                meta.0.0
                    .insert(LABBY_CORRELATION_META_KEY.to_string(), value);
            }
            None => {
                meta.0.0.remove(LABBY_CORRELATION_META_KEY);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn child(&self) -> Self {
        let mut child = self.clone();
        child.span_id = random_nonzero_u64().to_be_bytes();
        child
    }

    pub fn with_correlation(mut self, execution_id: impl Into<String>, call_ordinal: u64) -> Self {
        self.correlation = Some(LabbyCorrelation {
            execution_id: execution_id.into(),
            call_ordinal,
        });
        self
    }

    #[must_use]
    pub fn traceparent(&self) -> String {
        format!(
            "00-{}-{}-{:02x}",
            encode_hex(&self.trace_id),
            encode_hex(&self.span_id),
            self.flags
        )
    }
}

fn random_nonzero_u128() -> u128 {
    loop {
        let value = rand::random::<u128>();
        if value != 0 {
            return value;
        }
    }
}

fn random_nonzero_u64() -> u64 {
    loop {
        let value = rand::random::<u64>();
        if value != 0 {
            return value;
        }
    }
}

fn parse_traceparent(value: &str) -> Result<([u8; 16], [u8; 8], u8), TraceContextError> {
    if value.len() != 55
        || value.as_bytes().get(2) != Some(&b'-')
        || value.as_bytes().get(35) != Some(&b'-')
        || value.as_bytes().get(52) != Some(&b'-')
        || &value[0..2] != "00"
    {
        return Err(TraceContextError::InvalidTraceparent);
    }

    let trace_id =
        parse_hex_array::<16>(&value[3..35]).ok_or(TraceContextError::InvalidTraceparent)?;
    let span_id =
        parse_hex_array::<8>(&value[36..52]).ok_or(TraceContextError::InvalidTraceparent)?;
    let flags =
        u8::from_str_radix(&value[53..55], 16).map_err(|_| TraceContextError::InvalidTraceparent)?;

    if trace_id == [0; 16] || span_id == [0; 8] {
        return Err(TraceContextError::InvalidTraceparent);
    }
    Ok((trace_id, span_id, flags))
}

fn parse_hex_array<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 || !value.is_ascii() {
        return None;
    }
    let mut out = [0_u8; N];
    for (index, byte) in out.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16).ok()?;
    }
    Some(out)
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn validate_list(
    value: &str,
    max_bytes: usize,
    max_members: usize,
    error: TraceContextError,
) -> Result<(), TraceContextError> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value.is_ascii()
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(error);
    }

    let mut count = 0_usize;
    for member in value.split(',') {
        count += 1;
        let member = member.trim();
        let Some((key, _value)) = member.split_once('=') else {
            return Err(error);
        };
        if key.is_empty() || count > max_members {
            return Err(error);
        }
    }
    Ok(())
}

fn validate_correlation(correlation: &LabbyCorrelation) -> Result<(), TraceContextError> {
    if correlation.execution_id.is_empty()
        || correlation.execution_id.len() > MAX_EXECUTION_ID_BYTES
        || !correlation.execution_id.is_ascii()
        || correlation
            .execution_id
            .bytes()
            .any(|byte| byte.is_ascii_control())
    {
        return Err(TraceContextError::InvalidCorrelation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rmcp::model::RequestMetaObject;

    use super::*;

    const VALID: &str = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";

    #[test]
    fn parses_and_formats_w3c_traceparent() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent(VALID);

        let context = TraceContext::from_meta(&meta)
            .expect("valid context")
            .expect("present context");

        assert_eq!(context.traceparent(), VALID);
    }

    #[test]
    fn rejects_zero_ids_and_malformed_traceparents() {
        for value in [
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01",
            "ff-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01",
            "00-nothex-00f067aa0ba902b7-01",
        ] {
            let mut meta = RequestMetaObject::new();
            meta.set_traceparent(value);
            assert_eq!(
                TraceContext::from_meta(&meta),
                Err(TraceContextError::InvalidTraceparent),
                "{value}"
            );
        }
    }

    #[test]
    fn invalid_traceparent_does_not_continue_tracestate() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent("garbage");
        meta.set_tracestate("vendor=value");

        assert_eq!(
            TraceContext::from_meta(&meta),
            Err(TraceContextError::InvalidTraceparent)
        );
        let (replacement, disposition) = TraceContext::continue_or_new(Some(&meta));
        assert_eq!(disposition, InboundTraceDisposition::ReplacedInvalid);
        assert!(replacement.tracestate.is_none());
        assert!(replacement.baggage.is_none());
    }

    #[test]
    fn absent_trace_context_mints_fresh_unsampled_root() {
        let (context, disposition) = TraceContext::continue_or_new(None);
        assert_eq!(disposition, InboundTraceDisposition::Created);
        assert_eq!(context.flags, 0);
        assert_ne!(context.trace_id, [0; 16]);
        assert_ne!(context.span_id, [0; 8]);
    }

    #[test]
    fn untrusted_correlation_can_be_stripped_before_forwarding() {
        let mut meta = RequestMetaObject::new();
        meta.set_traceparent(VALID);
        meta.0.0.insert(
            LABBY_CORRELATION_META_KEY.to_string(),
            serde_json::json!({"execution_id":"forged","call_ordinal":99}),
        );

        let (context, disposition) = TraceContext::continue_or_new(Some(&meta));
        assert_eq!(disposition, InboundTraceDisposition::Continued);
        assert!(context.correlation.is_some());
        assert!(context.without_correlation().correlation.is_none());
    }

    #[test]
    fn enforces_tracestate_and_baggage_bounds() {
        let mut state = RequestMetaObject::new();
        state.set_traceparent(VALID);
        state.set_tracestate("a=v,".repeat(MAX_TRACESTATE_MEMBERS) + "z=v");
        assert_eq!(
            TraceContext::from_meta(&state),
            Err(TraceContextError::InvalidTracestate)
        );

        let mut baggage = RequestMetaObject::new();
        baggage.set_traceparent(VALID);
        baggage.set_baggage("a=x,".repeat(MAX_BAGGAGE_MEMBERS) + "z=x");
        assert_eq!(
            TraceContext::from_meta(&baggage),
            Err(TraceContextError::InvalidBaggage)
        );
    }

    #[test]
    fn inject_extract_round_trip_preserves_standard_and_labby_fields() {
        let mut context = TraceContext::new_root()
            .with_correlation("exec_123", 7);
        context.tracestate = Some("vendor=value".to_string());
        context.baggage = Some("region=us-east-1".to_string());

        let mut meta = RequestMetaObject::new();
        meta.0.0.insert(
            "other-extension".into(),
            serde_json::json!({"keep": true}),
        );

        context.inject(&mut meta).expect("inject");

        assert_eq!(meta.get_traceparent(), Some(context.traceparent().as_str()));
        assert_eq!(meta.get_tracestate(), Some("vendor=value"));
        assert_eq!(meta.get_baggage(), Some("region=us-east-1"));
        assert_eq!(
            meta.0.0.get("other-extension"),
            Some(&serde_json::json!({"keep": true}))
        );
        assert_eq!(
            TraceContext::from_meta(&meta).expect("extract"),
            Some(context)
        );
    }

    #[test]
    fn child_preserves_trace_id_and_mints_distinct_span_id() {
        let parent = TraceContext::new_root();
        let child = parent.child();

        assert_eq!(child.trace_id, parent.trace_id);
        assert_ne!(child.span_id, parent.span_id);
        assert_eq!(child.flags, parent.flags);
    }

    #[test]
    fn rejects_oversized_execution_id() {
        let context = TraceContext::new_root()
            .with_correlation("x".repeat(MAX_EXECUTION_ID_BYTES + 1), 1);
        let mut meta = RequestMetaObject::new();

        assert_eq!(
            context.inject(&mut meta),
            Err(TraceContextError::InvalidCorrelation)
        );
    }
}
