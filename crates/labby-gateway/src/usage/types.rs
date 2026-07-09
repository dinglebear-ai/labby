//! Types shared between the usage-telemetry writer (`UpstreamPool`) and the
//! query/aggregation side (`gateway.usage.*` actions).

/// One recorded call proxied through the gateway's `UpstreamPool`.
///
/// `capability`/`operation` mirror `UpstreamRequestLog` (`upstream/pool/logging.rs`)
/// deliberately, so this schema is not hardcoded to "external upstream only" —
/// a future in-process source (Labby's own tools reachable from Code Mode)
/// can populate the same shape without a migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamCallRecord {
    /// Unix seconds when the call finished (success or failure).
    pub ts_unix: i64,
    pub upstream_name: String,
    pub tool_name: String,
    /// `"tools"` | `"resources"` | `"prompts"`.
    pub capability: String,
    /// `"tool.call"` | `"resource.read"` | `"prompt.get"`.
    pub operation: String,
    pub subject_scoped: bool,
    /// OAuth subject for subject-scoped calls; `None` for the non-OAuth pool
    /// path (bearer-auth callers are not yet individually attributed).
    pub actor: Option<String>,
    /// `"ok"` | `"upstream_error"` | `"timeout"` | `"response_too_large"` | `"upstream_connect_error"`.
    pub outcome: String,
    pub error_kind: Option<String>,
    pub elapsed_ms: i64,
    pub response_bytes: Option<i64>,
}
