//! Aggregation query parameters and result shapes for `gateway.usage.*`.

#[derive(Debug, Clone, Default)]
pub struct UsageMetricsQuery {
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub upstream: Option<String>,
    /// Route-scope enforcement: when `Some`, results are restricted to these
    /// upstream names regardless of `upstream`. `None` means unscoped (root
    /// caller). See `gateway/manager/usage.rs`.
    pub allowed_upstreams: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageCallsQuery {
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub upstream: Option<String>,
    /// See `UsageMetricsQuery::allowed_upstreams`.
    pub allowed_upstreams: Option<Vec<String>>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageToolCount {
    pub upstream: String,
    pub tool: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageActorCount {
    /// `"unattributed"` for calls with no OAuth subject.
    pub actor: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageMetrics {
    pub total_calls: i64,
    pub error_calls: i64,
    pub avg_elapsed_ms: f64,
    pub top_tools: Vec<UsageToolCount>,
    pub top_actors: Vec<UsageActorCount>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpstreamCallRecordView {
    pub ts_unix: i64,
    pub upstream: String,
    pub tool: String,
    pub actor: String,
    pub outcome: String,
    pub elapsed_ms: i64,
}

pub(super) const TOP_N: usize = 10;
