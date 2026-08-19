//! Aggregation query parameters and result shapes for `gateway.usage.*`.

/// Default page size for `gateway.usage.calls` when the caller omits `limit`.
/// Applied at the params→query mapping layer (`gateway/manager/usage.rs`)
/// since `UsageCallsQuery.limit` is non-optional `usize`.
pub const DEFAULT_CALLS_LIMIT: usize = 100;
/// Hard cap on `UsageCallsQuery.limit`. Enforced both where the query is
/// constructed (`gateway/manager/usage.rs`) and, defense-in-depth, directly in
/// `UsageStore::list_calls` so the store never trusts an unbounded limit
/// regardless of what constructs the query.
pub const MAX_CALLS_LIMIT: usize = 1000;
/// Max buckets returned by one aggregate metrics query.
pub const MAX_METRICS_BUCKETS: usize = 168;

#[derive(Debug, Clone, Default)]
pub struct UsageMetricsQuery {
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub upstream: Option<String>,
    pub tool: Option<String>,
    pub actor: Option<String>,
    pub outcome: Option<String>,
    pub search: Option<String>,
    pub bucket_count: usize,
    pub timezone_offset_minutes: i32,
    pub include_facets: bool,
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
    pub tool: Option<String>,
    pub actor: Option<String>,
    pub outcome: Option<String>,
    pub search: Option<String>,
    /// See `UsageMetricsQuery::allowed_upstreams`.
    pub allowed_upstreams: Option<Vec<String>>,
    pub limit: usize,
    pub cursor: Option<UsageCursor>,
    pub include_total: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageCursor {
    pub ts_unix: i64,
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageToolCount {
    pub upstream: String,
    pub tool: String,
    pub capability: String,
    pub operation: String,
    pub subject_scoped: bool,
    pub calls: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageActorCount {
    /// `"unattributed"` for calls with no OAuth subject.
    pub actor: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageErrorCount {
    pub kind: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageUpstreamCount {
    pub upstream: String,
    pub calls: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageLatencyStat {
    pub upstream: String,
    pub tool: String,
    pub avg_elapsed_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageTimeBucket {
    pub ts_unix: i64,
    pub calls: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageHourCount {
    pub hour: u8,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageToolFacet {
    pub upstream: String,
    pub tool: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageFacets {
    pub tools: Vec<UsageToolFacet>,
    pub actors: Vec<String>,
    pub upstreams: Vec<String>,
    pub outcomes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageMetrics {
    pub window_total_calls: i64,
    pub total_calls: i64,
    pub error_calls: i64,
    pub avg_elapsed_ms: f64,
    pub p50_elapsed_ms: i64,
    pub p95_elapsed_ms: i64,
    pub p99_elapsed_ms: i64,
    pub distinct_tools: i64,
    pub distinct_actors: i64,
    pub peak_per_min: i64,
    pub top_tools: Vec<UsageToolCount>,
    pub least_tools: Vec<UsageToolCount>,
    pub top_actors: Vec<UsageActorCount>,
    pub slowest_tools: Vec<UsageLatencyStat>,
    pub errors: Vec<UsageErrorCount>,
    pub upstreams: Vec<UsageUpstreamCount>,
    pub hourly: Vec<UsageHourCount>,
    pub timeseries: Vec<UsageTimeBucket>,
    pub facets: UsageFacets,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpstreamCallRecordView {
    pub id: i64,
    pub ts_unix: i64,
    pub upstream: String,
    pub tool: String,
    pub capability: String,
    pub operation: String,
    pub subject_scoped: bool,
    pub actor: String,
    pub outcome: String,
    pub elapsed_ms: i64,
    pub response_bytes: Option<i64>,
}

pub(super) const TOP_N: usize = 10;
