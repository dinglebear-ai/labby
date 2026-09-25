//! Transport-neutral W3C Trace Context and MCP SEP-414 vocabulary.
//!
//! This module deliberately contains no gateway, authorization, routing, or
//! storage policy. It validates and carries observability context only. Product
//! crates decide whether an inbound context may be continued and where it is
//! injected.

use std::fmt;

use serde::Serialize;
use serde_json::{Value, json};

/// Standard MCP SEP-414 `_meta` key for W3C `traceparent`.
pub const MCP_TRACEPARENT_META_KEY: &str = "traceparent";
/// Standard MCP SEP-414 `_meta` key for W3C `tracestate`.
pub const MCP_TRACESTATE_META_KEY: &str = "tracestate";
/// Standard MCP SEP-414 `_meta` key for W3C Baggage.
pub const MCP_BAGGAGE_META_KEY: &str = "baggage";
/// Labby-private `_meta` extension carrying host-owned Code Mode correlation.
pub const MCP_LABBY_TRACE_META_KEY: &str = "ai.dinglebear.labby/trace";

/// W3C Trace Context limit for the serialized `tracestate` value.
pub const TRACESTATE_MAX_BYTES: usize = 512;
/// W3C Trace Context limit for list-members in `tracestate`.
pub const TRACESTATE_MAX_MEMBERS: usize = 32;
/// Defensive upper bound for MCP-carried W3C Baggage.
pub const BAGGAGE_MAX_BYTES: usize = 8 * 1024;
/// Defensive upper bound for members in MCP-carried W3C Baggage.
pub const BAGGAGE_MAX_MEMBERS: usize = 64;
/// Defensive upper bound for a single baggage member.
pub const BAGGAGE_MEMBER_MAX_BYTES: usize = 4096;
/// Bound for Labby's durable Code Mode execution identifier on the wire.
pub const LABBY_EXECUTION_ID_MAX_BYTES: usize = 128;

const TRACE_ID_BYTES: usize = 16;
const SPAN_ID_BYTES: usize = 8;
const TRACEPARENT_V00_LEN: usize = 55;
const VERSION_V00: u8 = 0;
const VERSION_INVALID: u8 = 0xff;
const HEX: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

/// Validation or generation failure for trace metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceContextError {
    InvalidTraceparent,
    UnsupportedTraceparentVersion,
    InvalidTraceId,
    InvalidSpanId,
    InvalidTraceFlags,
    TracestateTooLarge,
    TooManyTracestateMembers,
    InvalidTracestateMember,
    BaggageTooLarge,
    TooManyBaggageMembers,
    InvalidBaggageMember,
    InvalidExecutionId,
    EntropyUnavailable,
}

impl fmt::Display for TraceContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTraceparent => "traceparent must use the canonical W3C version 00 shape",
            Self::UnsupportedTraceparentVersion => {
                "traceparent version is unsupported by this Labby build"
            }
            Self::InvalidTraceId => "trace id must be 32 lowercase hex digits and not all zero",
            Self::InvalidSpanId => "span id must be 16 lowercase hex digits and not all zero",
            Self::InvalidTraceFlags => "trace flags must be two lowercase hex digits",
            Self::TracestateTooLarge => "tracestate exceeds the 512-byte W3C limit",
            Self::TooManyTracestateMembers => "tracestate exceeds the 32-member W3C limit",
            Self::InvalidTracestateMember => "tracestate contains an invalid list-member",
            Self::BaggageTooLarge => "baggage exceeds Labby's 8192-byte propagation limit",
            Self::TooManyBaggageMembers => "baggage exceeds Labby's 64-member propagation limit",
            Self::InvalidBaggageMember => "baggage contains an invalid or oversized member",
            Self::InvalidExecutionId => "execution id is empty, oversized, or non-canonical",
            Self::EntropyUnavailable => "operating-system randomness was unavailable",
        })
    }
}

impl std::error::Error for TraceContextError {}

/// A non-zero 128-bit W3C trace identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TraceId([u8; TRACE_ID_BYTES]);

impl TraceId {
    pub fn parse(hex: &str) -> Result<Self, TraceContextError> {
        let bytes =
            decode_lower_hex::<TRACE_ID_BYTES>(hex).ok_or(TraceContextError::InvalidTraceId)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(TraceContextError::InvalidTraceId);
        }
        Ok(Self(bytes))
    }

    pub fn generate() -> Result<Self, TraceContextError> {
        loop {
            let mut bytes = [0_u8; TRACE_ID_BYTES];
            getrandom::fill(&mut bytes).map_err(|_| TraceContextError::EntropyUnavailable)?;
            if bytes.iter().any(|byte| *byte != 0) {
                return Ok(Self(bytes));
            }
        }
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

/// A non-zero 64-bit W3C span identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SpanId([u8; SPAN_ID_BYTES]);

impl SpanId {
    pub fn parse(hex: &str) -> Result<Self, TraceContextError> {
        let bytes =
            decode_lower_hex::<SPAN_ID_BYTES>(hex).ok_or(TraceContextError::InvalidSpanId)?;
        if bytes.iter().all(|byte| *byte == 0) {
            return Err(TraceContextError::InvalidSpanId);
        }
        Ok(Self(bytes))
    }

    pub fn generate() -> Result<Self, TraceContextError> {
        loop {
            let mut bytes = [0_u8; SPAN_ID_BYTES];
            getrandom::fill(&mut bytes).map_err(|_| TraceContextError::EntropyUnavailable)?;
            if bytes.iter().any(|byte| *byte != 0) {
                return Ok(Self(bytes));
            }
        }
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

/// Canonical W3C version-00 `traceparent`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceParent {
    trace_id: TraceId,
    parent_id: SpanId,
    trace_flags: u8,
}

impl TraceParent {
    pub fn parse(value: &str) -> Result<Self, TraceContextError> {
        if value.len() != TRACEPARENT_V00_LEN {
            return Err(TraceContextError::InvalidTraceparent);
        }
        let bytes = value.as_bytes();
        if bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
            return Err(TraceContextError::InvalidTraceparent);
        }

        let version =
            decode_lower_hex_byte(&value[0..2]).ok_or(TraceContextError::InvalidTraceparent)?;
        if version == VERSION_INVALID {
            return Err(TraceContextError::InvalidTraceparent);
        }
        if version != VERSION_V00 {
            return Err(TraceContextError::UnsupportedTraceparentVersion);
        }

        let trace_id = TraceId::parse(&value[3..35])?;
        let parent_id = SpanId::parse(&value[36..52])?;
        let trace_flags =
            decode_lower_hex_byte(&value[53..55]).ok_or(TraceContextError::InvalidTraceFlags)?;

        Ok(Self {
            trace_id,
            parent_id,
            trace_flags,
        })
    }

    #[must_use]
    pub fn new(trace_id: TraceId, parent_id: SpanId, trace_flags: u8) -> Self {
        Self {
            trace_id,
            parent_id,
            trace_flags,
        }
    }

    #[must_use]
    pub fn trace_id(self) -> TraceId {
        self.trace_id
    }

    #[must_use]
    pub fn parent_id(self) -> SpanId {
        self.parent_id
    }

    #[must_use]
    pub fn trace_flags(self) -> u8 {
        self.trace_flags
    }

    #[must_use]
    pub fn to_header_value(self) -> String {
        format!(
            "00-{}-{}-{:02x}",
            self.trace_id.to_hex(),
            self.parent_id.to_hex(),
            self.trace_flags
        )
    }
}

impl fmt::Display for TraceParent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_header_value())
    }
}

/// Validated W3C `tracestate`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceState(String);

impl TraceState {
    pub fn parse(value: impl Into<String>) -> Result<Self, TraceContextError> {
        let value = value.into();
        if value.len() > TRACESTATE_MAX_BYTES {
            return Err(TraceContextError::TracestateTooLarge);
        }
        if value.is_empty() {
            return Err(TraceContextError::InvalidTracestateMember);
        }

        let members: Vec<&str> = value.split(',').collect();
        if members.len() > TRACESTATE_MAX_MEMBERS {
            return Err(TraceContextError::TooManyTracestateMembers);
        }
        if members
            .iter()
            .any(|member| !valid_tracestate_member(member.trim_matches([' ', '\t'])))
        {
            return Err(TraceContextError::InvalidTracestateMember);
        }

        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bounded, injection-safe W3C Baggage value.
///
/// Labby treats baggage as opaque observability context. It is never promoted
/// into authorization, routing, tenant, capability, or trusted log fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Baggage(String);

impl Baggage {
    pub fn parse(value: impl Into<String>) -> Result<Self, TraceContextError> {
        let value = value.into();
        if value.len() > BAGGAGE_MAX_BYTES {
            return Err(TraceContextError::BaggageTooLarge);
        }
        if value.is_empty() {
            return Err(TraceContextError::InvalidBaggageMember);
        }

        let members: Vec<&str> = value.split(',').collect();
        if members.len() > BAGGAGE_MAX_MEMBERS {
            return Err(TraceContextError::TooManyBaggageMembers);
        }
        if members.iter().any(|member| !valid_baggage_member(member)) {
            return Err(TraceContextError::InvalidBaggageMember);
        }

        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Active local trace context. Its span id is the parent id sent to children.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceContext {
    trace_id: TraceId,
    span_id: SpanId,
    trace_flags: u8,
    tracestate: Option<TraceState>,
    baggage: Option<Baggage>,
}

impl TraceContext {
    pub fn fresh(trace_flags: u8) -> Result<Self, TraceContextError> {
        Ok(Self {
            trace_id: TraceId::generate()?,
            span_id: SpanId::generate()?,
            trace_flags,
            tracestate: None,
            baggage: None,
        })
    }

    pub fn continue_remote(
        parent: TraceParent,
        tracestate: Option<TraceState>,
        baggage: Option<Baggage>,
    ) -> Result<Self, TraceContextError> {
        Ok(Self {
            trace_id: parent.trace_id(),
            span_id: SpanId::generate()?,
            trace_flags: parent.trace_flags(),
            tracestate,
            baggage,
        })
    }

    pub fn child(&self) -> Result<Self, TraceContextError> {
        Ok(Self {
            trace_id: self.trace_id,
            span_id: SpanId::generate()?,
            trace_flags: self.trace_flags,
            tracestate: self.tracestate.clone(),
            baggage: self.baggage.clone(),
        })
    }

    #[must_use]
    pub fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    #[must_use]
    pub fn span_id(&self) -> SpanId {
        self.span_id
    }

    #[must_use]
    pub fn trace_flags(&self) -> u8 {
        self.trace_flags
    }

    #[must_use]
    pub fn tracestate(&self) -> Option<&TraceState> {
        self.tracestate.as_ref()
    }

    #[must_use]
    pub fn baggage(&self) -> Option<&Baggage> {
        self.baggage.as_ref()
    }

    #[must_use]
    pub fn traceparent(&self) -> TraceParent {
        TraceParent::new(self.trace_id, self.span_id, self.trace_flags)
    }
}

/// Host-owned Code Mode correlation carried separately from standard W3C keys.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LabbyTraceCorrelation {
    execution_id: String,
    call_ordinal: u64,
}

impl LabbyTraceCorrelation {
    pub fn new(
        execution_id: impl Into<String>,
        call_ordinal: u64,
    ) -> Result<Self, TraceContextError> {
        let execution_id = execution_id.into();
        if execution_id.is_empty()
            || execution_id.len() > LABBY_EXECUTION_ID_MAX_BYTES
            || execution_id != execution_id.trim()
            || execution_id.chars().any(char::is_control)
        {
            return Err(TraceContextError::InvalidExecutionId);
        }
        Ok(Self {
            execution_id,
            call_ordinal,
        })
    }

    #[must_use]
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }

    #[must_use]
    pub fn call_ordinal(&self) -> u64 {
        self.call_ordinal
    }

    #[must_use]
    pub fn to_meta_value(&self) -> Value {
        json!({
            "execution_id": self.execution_id,
            "call_ordinal": self.call_ordinal,
        })
    }
}

fn valid_tracestate_member(member: &str) -> bool {
    let Some((key, value)) = member.split_once('=') else {
        return false;
    };
    if key.is_empty() || key.len() > 256 || value.is_empty() || value.len() > 256 {
        return false;
    }
    if !valid_tracestate_key(key) {
        return false;
    }
    value.bytes().all(valid_tracestate_value_byte) && !value.ends_with(' ')
}

fn valid_tracestate_key(key: &str) -> bool {
    if let Some((tenant, system)) = key.split_once('@') {
        if tenant.is_empty()
            || tenant.len() > 241
            || system.is_empty()
            || system.len() > 14
            || system.contains('@')
        {
            return false;
        }
        let mut tenant_bytes = tenant.bytes();
        let Some(first) = tenant_bytes.next() else {
            return false;
        };
        if !(first.is_ascii_lowercase() || first.is_ascii_digit())
            || !tenant_bytes.all(valid_tracestate_key_char)
        {
            return false;
        }
        let mut system_bytes = system.bytes();
        let Some(first) = system_bytes.next() else {
            return false;
        };
        first.is_ascii_lowercase() && system_bytes.all(valid_tracestate_key_char)
    } else {
        let mut bytes = key.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        first.is_ascii_lowercase() && bytes.all(valid_tracestate_key_char)
    }
}

fn valid_tracestate_key_char(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'*' | b'/')
}

fn valid_tracestate_value_byte(byte: u8) -> bool {
    matches!(byte, 0x20..=0x2b | 0x2d..=0x3c | 0x3e..=0x7e)
}

fn valid_baggage_member(member: &str) -> bool {
    let member = member.trim_matches([' ', '\t']);
    if member.is_empty()
        || member.len() > BAGGAGE_MEMBER_MAX_BYTES
        || member.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
    {
        return false;
    }
    let core = member.split_once(';').map_or(member, |(core, _)| core);
    let Some((name, _value)) = core.split_once('=') else {
        return false;
    };
    !name.is_empty() && name.bytes().all(valid_baggage_key_byte)
}

fn valid_baggage_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | 0x27
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | 0x60
                | b'|'
                | b'~'
        )
}

fn decode_lower_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let bytes = value.as_bytes();
    let mut decoded = [0_u8; N];
    for index in 0..N {
        let high = lower_hex_nibble(bytes[index * 2])?;
        let low = lower_hex_nibble(bytes[index * 2 + 1])?;
        decoded[index] = (high << 4) | low;
    }
    Some(decoded)
}

fn decode_lower_hex_byte(value: &str) -> Option<u8> {
    decode_lower_hex::<1>(value).map(|bytes| bytes[0])
}

fn lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)]);
        encoded.push(HEX[usize::from(byte & 0x0f)]);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

    #[test]
    fn traceparent_valid_vector_round_trips() {
        let parent = TraceParent::parse(VALID_TRACEPARENT).unwrap();
        assert_eq!(
            parent.trace_id().to_hex(),
            "0af7651916cd43dd8448eb211c80319c"
        );
        assert_eq!(parent.parent_id().to_hex(), "b7ad6b7169203331");
        assert_eq!(parent.trace_flags(), 1);
        assert_eq!(parent.to_header_value(), VALID_TRACEPARENT);
    }

    #[test]
    fn traceparent_rejects_invalid_w3c_boundaries() {
        for invalid in [
            "",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331",
            "00-00000000000000000000000000000000-b7ad6b7169203331-01",
            "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01",
            "00-0AF7651916CD43DD8448EB211C80319C-b7ad6b7169203331-01",
            "ff-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-0g",
        ] {
            assert!(TraceParent::parse(invalid).is_err(), "{invalid}");
        }
        assert_eq!(
            TraceParent::parse("01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
                .unwrap_err(),
            TraceContextError::UnsupportedTraceparentVersion
        );
    }

    #[test]
    fn tracestate_enforces_w3c_size_count_and_member_shape() {
        let state = TraceState::parse("vendor=value,tenant@system=opaque").unwrap();
        assert_eq!(state.as_str(), "vendor=value,tenant@system=opaque");
        assert_eq!(
            TraceState::parse("a=".to_string() + &"x".repeat(TRACESTATE_MAX_BYTES)).unwrap_err(),
            TraceContextError::TracestateTooLarge
        );
        assert_eq!(
            TraceState::parse(
                (0..=TRACESTATE_MAX_MEMBERS)
                    .map(|index| format!("v{index}=x"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .unwrap_err(),
            TraceContextError::TooManyTracestateMembers
        );
        for invalid in ["Upper=value", "vendor=", "=value", "a=v,", "a==v"] {
            assert_eq!(
                TraceState::parse(invalid).unwrap_err(),
                TraceContextError::InvalidTracestateMember,
                "{invalid}"
            );
        }
        assert!(TraceState::parse("a=v ").is_ok());
    }

    #[test]
    fn baggage_is_bounded_and_rejects_control_injection() {
        let baggage = Baggage::parse("userId=alice,serverNode=DF%2028;ttl=60").unwrap();
        assert_eq!(baggage.as_str(), "userId=alice,serverNode=DF%2028;ttl=60");
        assert_eq!(
            Baggage::parse("a=".to_string() + &"x".repeat(BAGGAGE_MAX_BYTES)).unwrap_err(),
            TraceContextError::BaggageTooLarge
        );
        assert_eq!(
            Baggage::parse(
                "safe=value
trusted=true"
            )
            .unwrap_err(),
            TraceContextError::InvalidBaggageMember
        );
        assert_eq!(
            Baggage::parse(",").unwrap_err(),
            TraceContextError::InvalidBaggageMember
        );
    }

    #[test]
    fn continued_context_preserves_trace_and_mints_child_spans() {
        let parent = TraceParent::parse(VALID_TRACEPARENT).unwrap();
        let context = TraceContext::continue_remote(
            parent,
            Some(TraceState::parse("vendor=value").unwrap()),
            Some(Baggage::parse("userId=alice").unwrap()),
        )
        .unwrap();
        assert_eq!(context.trace_id(), parent.trace_id());
        assert_ne!(context.span_id(), parent.parent_id());
        assert_eq!(context.trace_flags(), parent.trace_flags());
        assert_eq!(context.tracestate().unwrap().as_str(), "vendor=value");
        assert_eq!(context.baggage().unwrap().as_str(), "userId=alice");

        let child = context.child().unwrap();
        assert_eq!(child.trace_id(), context.trace_id());
        assert_ne!(child.span_id(), context.span_id());
        assert_eq!(child.trace_flags(), context.trace_flags());
        assert_eq!(child.tracestate(), context.tracestate());
        assert_eq!(child.baggage(), context.baggage());
    }

    #[test]
    fn fresh_context_has_nonzero_canonical_identifiers() {
        let context = TraceContext::fresh(1).unwrap();
        assert_ne!(context.trace_id().to_hex(), "0".repeat(32));
        assert_ne!(context.span_id().to_hex(), "0".repeat(16));
        assert_eq!(
            context.traceparent().to_header_value().len(),
            TRACEPARENT_V00_LEN
        );
    }

    #[test]
    fn labby_correlation_is_bounded_and_serializes_separately() {
        let correlation = LabbyTraceCorrelation::new("exec_abc", 7).unwrap();
        assert_eq!(correlation.execution_id(), "exec_abc");
        assert_eq!(correlation.call_ordinal(), 7);
        assert_eq!(
            correlation.to_meta_value(),
            json!({"execution_id": "exec_abc", "call_ordinal": 7})
        );
        assert_eq!(
            LabbyTraceCorrelation::new("", 0).unwrap_err(),
            TraceContextError::InvalidExecutionId
        );
        assert_eq!(
            LabbyTraceCorrelation::new("x".repeat(LABBY_EXECUTION_ID_MAX_BYTES + 1), 0)
                .unwrap_err(),
            TraceContextError::InvalidExecutionId
        );
    }
}
