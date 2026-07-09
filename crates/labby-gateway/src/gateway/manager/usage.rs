//! `GatewayManager` facade over `UsageStore`'s query side, backing the
//! `gateway.usage.metrics` / `gateway.usage.calls` actions. Read-only: this
//! module never writes — writes happen inline in `UpstreamPool` (see
//! `upstream/pool/usage_record.rs`).

use labby_runtime::error::ToolError;

use crate::usage::query::{UsageCallsQuery, UsageMetricsQuery};

use super::GatewayManager;
use crate::gateway::params::{
    GatewayEnrichmentScope, GatewayUsageCallsParams, GatewayUsageMetricsParams,
};
use crate::gateway::types::{
    GatewayUsageActorCount, GatewayUsageCallView, GatewayUsageCallsView, GatewayUsageMetricsView,
    GatewayUsageToolCount,
};

const DEFAULT_CALLS_LIMIT: usize = 100;
const MAX_CALLS_LIMIT: usize = 1000;

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
        let limit = params
            .limit
            .unwrap_or(DEFAULT_CALLS_LIMIT)
            .min(MAX_CALLS_LIMIT);
        let (rows, total_matching) = store
            .list_calls(UsageCallsQuery {
                since_unix: params.since_unix,
                until_unix: params.until_unix,
                upstream: params.upstream,
                allowed_upstreams,
                limit,
                offset: params.offset.unwrap_or(0),
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
        })
    }
}

/// Enforce route scope for a usage query. Mirrors the enrichment scope check
/// in `manager/enrichment.rs::apply_enrichment_scoped`:
///
/// - If the caller explicitly requested a single `upstream` that is not in
///   the route-visible set, fail with `unknown_upstream` (matching the
///   enrichment out-of-scope-upstream error shape).
/// - Otherwise (aggregate query, no explicit upstream), return the
///   route-visible set so the store can restrict its `WHERE` clause to it.
/// - `None` scope (root/unscoped caller) always returns `None` (no filter).
fn scoped_allowed_upstreams(
    scope: &GatewayEnrichmentScope,
    requested_upstream: Option<&str>,
) -> Result<Option<Vec<String>>, ToolError> {
    let Some(visible) = &scope.route_visible_upstreams else {
        return Ok(None);
    };
    if let Some(upstream) = requested_upstream
        && !visible.contains(upstream)
    {
        return Err(ToolError::Sdk {
            sdk_kind: "unknown_upstream".to_string(),
            message: format!("unknown gateway upstream `{upstream}`"),
        });
    }
    Ok(Some(visible.iter().cloned().collect()))
}
