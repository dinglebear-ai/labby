//! Types shared between the usage-telemetry writer (`UpstreamPool`) and the
//! query/aggregation side (`gateway.usage.*` actions).

/// One recorded call proxied through the gateway's `UpstreamPool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamCallRecord {
    /// Unix seconds when the call finished (success or failure).
    pub ts_unix: i64,
    pub upstream_name: String,
    pub tool_name: String,
    /// OAuth subject for subject-scoped calls; `"unattributed"` for the
    /// non-OAuth pool path (bearer-auth callers are not yet individually
    /// attributed).
    pub actor: String,
    /// `"ok"` | `"upstream_error"` | `"timeout"` | `"response_too_large"` | `"upstream_connect_error"`.
    pub outcome: String,
    pub elapsed_ms: i64,
}
