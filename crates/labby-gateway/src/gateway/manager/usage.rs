//! `GatewayManager` facade over `UsageStore`'s query side, backing the
//! `gateway.usage.metrics` / `gateway.usage.calls` actions. Read-only: this
//! module never writes — writes happen inline in `UpstreamPool` (see
//! `upstream/pool/usage_record.rs`).

use labby_runtime::error::ToolError;

use crate::usage::query::{
    DEFAULT_CALLS_LIMIT, MAX_CALLS_LIMIT, MAX_METRICS_BUCKETS, UsageCallsQuery, UsageCursor,
    UsageMetricsQuery,
};

use super::GatewayManager;
use crate::gateway::params::{
    GatewayEnrichmentScope, GatewayUsageCallsParams, GatewayUsageMetricsParams,
};
use crate::gateway::types::{
    GatewayUsageActorCount, GatewayUsageCallView, GatewayUsageCallsView, GatewayUsageErrorCount,
    GatewayUsageFacets, GatewayUsageHourCount, GatewayUsageLatencyStat, GatewayUsageMetricsView,
    GatewayUsageTimeBucket, GatewayUsageToolCount, GatewayUsageToolFacet,
    GatewayUsageUpstreamCount,
};

impl GatewayManager {
    pub async fn usage_metrics(
        &self,
        params: GatewayUsageMetricsParams,
    ) -> Result<GatewayUsageMetricsView, ToolError> {
        self.usage_metrics_scoped(params, GatewayEnrichmentScope::default())
            .await
    }

    pub(crate) async fn usage_metrics_scoped(
        &self,
        params: GatewayUsageMetricsParams,
        scope: GatewayEnrichmentScope,
    ) -> Result<GatewayUsageMetricsView, ToolError> {
        let Some(store) = &self.usage_store else {
            return Err(ToolError::Sdk {
                sdk_kind: "usage_store_unavailable".to_string(),
                message: "gateway usage telemetry is disabled for this instance".to_string(),
            });
        };
        let allowed_upstreams = scoped_allowed_upstreams(&scope, params.upstream.as_deref())?;
        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: params.since_unix,
                until_unix: params.until_unix,
                upstream: params.upstream,
                tool: params.tool,
                actor: params.actor,
                outcome: params.outcome,
                search: params.search,
                bucket_count: params.bucket_count.unwrap_or(0).min(MAX_METRICS_BUCKETS),
                timezone: params.timezone,
                timezone_offset_minutes: params
                    .timezone_offset_minutes
                    .unwrap_or(0)
                    .clamp(-1440, 1440),
                include_facets: params.include_facets.unwrap_or(false),
                allowed_upstreams,
            })
            .await?;
        let map_tool = |t: crate::usage::query::UsageToolCount| GatewayUsageToolCount {
            upstream: t.upstream,
            tool: t.tool,
            capability: t.capability,
            operation: t.operation,
            subject_scoped: t.subject_scoped,
            calls: t.calls,
            failed: t.failed,
        };
        Ok(GatewayUsageMetricsView {
            window_total_calls: metrics.window_total_calls,
            total_calls: metrics.total_calls,
            error_calls: metrics.error_calls,
            avg_elapsed_ms: metrics.avg_elapsed_ms,
            p50_elapsed_ms: metrics.p50_elapsed_ms,
            p95_elapsed_ms: metrics.p95_elapsed_ms,
            p99_elapsed_ms: metrics.p99_elapsed_ms,
            distinct_tools: metrics.distinct_tools,
            distinct_actors: metrics.distinct_actors,
            peak_per_min: metrics.peak_per_min,
            top_tools: metrics.top_tools.into_iter().map(map_tool).collect(),
            least_tools: metrics.least_tools.into_iter().map(map_tool).collect(),
            top_actors: metrics
                .top_actors
                .into_iter()
                .map(|a| GatewayUsageActorCount {
                    actor: a.actor,
                    calls: a.calls,
                })
                .collect(),
            slowest_tools: metrics
                .slowest_tools
                .into_iter()
                .map(|t| GatewayUsageLatencyStat {
                    upstream: t.upstream,
                    tool: t.tool,
                    avg_elapsed_ms: t.avg_elapsed_ms,
                })
                .collect(),
            errors: metrics
                .errors
                .into_iter()
                .map(|e| GatewayUsageErrorCount {
                    kind: e.kind,
                    calls: e.calls,
                })
                .collect(),
            upstreams: metrics
                .upstreams
                .into_iter()
                .map(|u| GatewayUsageUpstreamCount {
                    upstream: u.upstream,
                    calls: u.calls,
                    failed: u.failed,
                })
                .collect(),
            hourly: metrics
                .hourly
                .into_iter()
                .map(|h| GatewayUsageHourCount {
                    hour: h.hour,
                    calls: h.calls,
                })
                .collect(),
            timeseries: metrics
                .timeseries
                .into_iter()
                .map(|b| GatewayUsageTimeBucket {
                    ts_unix: b.ts_unix,
                    calls: b.calls,
                    failed: b.failed,
                })
                .collect(),
            facets: GatewayUsageFacets {
                tools: metrics
                    .facets
                    .tools
                    .into_iter()
                    .map(|t| GatewayUsageToolFacet {
                        upstream: t.upstream,
                        tool: t.tool,
                    })
                    .collect(),
                actors: metrics.facets.actors,
                upstreams: metrics.facets.upstreams,
                outcomes: metrics.facets.outcomes,
            },
        })
    }

    pub async fn usage_calls(
        &self,
        params: GatewayUsageCallsParams,
    ) -> Result<GatewayUsageCallsView, ToolError> {
        self.usage_calls_scoped(params, GatewayEnrichmentScope::default())
            .await
    }

    pub(crate) async fn usage_calls_scoped(
        &self,
        params: GatewayUsageCallsParams,
        scope: GatewayEnrichmentScope,
    ) -> Result<GatewayUsageCallsView, ToolError> {
        let Some(store) = &self.usage_store else {
            return Err(ToolError::Sdk {
                sdk_kind: "usage_store_unavailable".to_string(),
                message: "gateway usage telemetry is disabled for this instance".to_string(),
            });
        };
        let allowed_upstreams = scoped_allowed_upstreams(&scope, params.upstream.as_deref())?;
        if params.offset.unwrap_or(0) > 0 {
            return Err(ToolError::InvalidParam {
                message: "offset pagination is disabled; pass the previous page's cursor"
                    .to_string(),
                param: "offset".to_string(),
            });
        }
        let cursor = params
            .cursor
            .as_deref()
            .map(parse_usage_cursor)
            .transpose()?;
        let limit = params
            .limit
            .unwrap_or(DEFAULT_CALLS_LIMIT)
            .clamp(1, MAX_CALLS_LIMIT);
        let (rows, total_matching, next_cursor) = store
            .list_calls(UsageCallsQuery {
                since_unix: params.since_unix,
                until_unix: params.until_unix,
                upstream: params.upstream,
                tool: params.tool,
                actor: params.actor,
                outcome: params.outcome,
                search: params.search,
                allowed_upstreams,
                limit,
                cursor,
                include_total: params.include_total.unwrap_or(false),
            })
            .await?;
        Ok(GatewayUsageCallsView {
            calls: rows
                .into_iter()
                .map(|r| GatewayUsageCallView {
                    ts_unix: r.ts_unix,
                    upstream: r.upstream,
                    tool: r.tool,
                    capability: r.capability,
                    operation: r.operation,
                    subject_scoped: r.subject_scoped,
                    actor: r.actor,
                    outcome: r.outcome,
                    elapsed_ms: r.elapsed_ms,
                    response_bytes: r.response_bytes,
                })
                .collect(),
            total_matching,
            next_cursor: next_cursor.map(format_usage_cursor),
        })
    }
}

fn parse_usage_cursor(cursor: &str) -> Result<UsageCursor, ToolError> {
    let (ts, id) = cursor
        .split_once(':')
        .ok_or_else(|| ToolError::InvalidParam {
            message: "cursor must have the form <timestamp>:<id>".to_string(),
            param: "cursor".to_string(),
        })?;
    Ok(UsageCursor {
        ts_unix: ts.parse().map_err(|_| ToolError::InvalidParam {
            message: "cursor timestamp is invalid".to_string(),
            param: "cursor".to_string(),
        })?,
        id: id.parse().map_err(|_| ToolError::InvalidParam {
            message: "cursor id is invalid".to_string(),
            param: "cursor".to_string(),
        })?,
    })
}

fn format_usage_cursor(cursor: UsageCursor) -> String {
    format!("{}:{}", cursor.ts_unix, cursor.id)
}

/// Enforce route scope for a usage query, delegating to the shared
/// `GatewayEnrichmentScope::ensure_visible`/`allowlist` helpers also used by
/// `manager/enrichment.rs`:
///
/// - If the caller explicitly requested a single `upstream` that is not in
///   the route-visible set, fail with `unknown_upstream`.
/// - Otherwise (aggregate query, no explicit upstream), return the
///   route-visible set so the store can restrict its `WHERE` clause to it.
/// - `None` scope (root/unscoped caller) always returns `None` (no filter).
fn scoped_allowed_upstreams(
    scope: &GatewayEnrichmentScope,
    requested_upstream: Option<&str>,
) -> Result<Option<Vec<String>>, ToolError> {
    if let Some(upstream) = requested_upstream {
        scope.ensure_visible(upstream)?;
    }
    Ok(scope.allowlist())
}
