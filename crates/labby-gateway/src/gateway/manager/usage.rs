//! `GatewayManager` facade over `UsageStore`'s query side, backing the
//! `gateway.usage.metrics` / `gateway.usage.calls` actions. Read-only: this
//! module never writes — writes happen inline in `UpstreamPool` (see
//! `upstream/pool/usage_record.rs`).

use labby_runtime::error::ToolError;

use crate::usage::query::{
    DEFAULT_CALLS_LIMIT, MAX_CALLS_LIMIT, UsageCallsQuery, UsageCursor, UsageMetricsQuery,
};

use super::GatewayManager;
use crate::gateway::params::{
    GatewayEnrichmentScope, GatewayUsageCallsParams, GatewayUsageMetricsParams,
};
use crate::gateway::types::{
    GatewayUsageActorCount, GatewayUsageCallView, GatewayUsageCallsView, GatewayUsageMetricsView,
    GatewayUsageToolCount,
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
                allowed_upstreams,
            })
            .await?;
        Ok(GatewayUsageMetricsView {
            total_calls: metrics.total_calls,
            error_calls: metrics.error_calls,
            avg_elapsed_ms: metrics.avg_elapsed_ms,
            top_tools: metrics
                .top_tools
                .into_iter()
                .map(|t| GatewayUsageToolCount {
                    upstream: t.upstream,
                    tool: t.tool,
                    calls: t.calls,
                })
                .collect(),
            top_actors: metrics
                .top_actors
                .into_iter()
                .map(|a| GatewayUsageActorCount {
                    actor: a.actor,
                    calls: a.calls,
                })
                .collect(),
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
                    actor: r.actor,
                    outcome: r.outcome,
                    elapsed_ms: r.elapsed_ms,
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
