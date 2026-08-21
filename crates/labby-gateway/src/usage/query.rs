//! Aggregation query parameters and result shapes for gateway.usage.*.

/// Default page size for gateway.usage.calls when the caller omits limit.
pub const DEFAULT_CALLS_LIMIT: usize = 100;
/// Hard per-page cap. This is a pagination safety bound, never an analytics
/// sampling bound: filters/counts always run against the complete retained
/// window before the page is selected.
pub const MAX_CALLS_LIMIT: usize = 1000;
pub const MAX_BUCKET_SECONDS: i64 = 24 * 60 * 60;
pub const MIN_BUCKET_SECONDS: i64 = 60;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageFilters {
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub upstream: Option<String>,
    pub tool: Option<String>,
    pub capability: Option<String>,
    pub operation: Option<String>,
    pub subject_scoped: Option<bool>,
    pub actor: Option<String>,
    /// ok, failed (all non-ok outcomes), or an exact persisted failure kind.
    pub outcome: Option<String>,
    /// Case-insensitive substring match across persisted textual dimensions.
    pub search: Option<String>,
    /// Route-scope enforcement. None means unscoped.
    pub allowed_upstreams: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageMetricsQuery {
    pub filters: UsageFilters,
    pub bucket_seconds: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageFacetsQuery {
    pub filters: UsageFilters,
}

#[derive(Debug, Clone, Default)]
pub struct UsageCallsQuery {
    pub filters: UsageFilters,
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
pub struct UsageTargetLatency {
    pub upstream: String,
    pub tool: String,
    pub capability: String,
    pub operation: String,
    pub subject_scoped: bool,
    pub avg_elapsed_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageActorCount {
    pub actor: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageUpstreamCount {
    pub upstream: String,
    pub calls: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageNamedCount {
    pub name: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageTimeBucket {
    pub ts_unix: i64,
    pub calls: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageMetrics {
    pub total_calls: i64,
    pub error_calls: i64,
    pub avg_elapsed_ms: f64,
    pub p50_elapsed_ms: i64,
    pub p95_elapsed_ms: i64,
    pub p99_elapsed_ms: i64,
    pub top_tools: Vec<UsageToolCount>,
    pub least_tools: Vec<UsageToolCount>,
    pub distinct_tools: i64,
    pub top_actors: Vec<UsageActorCount>,
    pub upstreams: Vec<UsageUpstreamCount>,
    pub errors: Vec<UsageNamedCount>,
    pub slowest_tools: Vec<UsageTargetLatency>,
    pub peak_per_min: i64,
    pub bucket_seconds: Option<i64>,
    pub timeseries: Vec<UsageTimeBucket>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageFacets {
    pub targets: Vec<UsageToolCount>,
    pub actors: Vec<UsageActorCount>,
    pub upstreams: Vec<UsageUpstreamCount>,
    pub capabilities: Vec<UsageNamedCount>,
    pub operations: Vec<UsageNamedCount>,
    pub outcomes: Vec<UsageNamedCount>,
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
pub(super) const SLOWEST_N: usize = 5;
