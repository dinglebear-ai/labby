//! `GatewayManager` facade over `UsageStore`'s query side, backing the
//! `gateway.usage.metrics` / `gateway.usage.calls` actions. Read-only: this
//! module never writes — writes happen inline in `UpstreamPool` (see
//! `upstream/pool/usage_record.rs`).

use labby_runtime::error::ToolError;

use crate::usage::query::{UsageCallsQuery, UsageMetricsQuery};

use super::GatewayManager;
use crate::gateway::params::{GatewayUsageCallsParams, GatewayUsageMetricsParams};
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
        let Some(store) = &self.usage_store else {
            return Err(ToolError::Sdk {
                sdk_kind: "usage_store_unavailable".to_string(),
                message: "gateway usage telemetry is disabled for this instance".to_string(),
            });
        };
        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: params.since_unix,
                until_unix: params.until_unix,
                upstream: params.upstream,
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
        let Some(store) = &self.usage_store else {
            return Err(ToolError::Sdk {
                sdk_kind: "usage_store_unavailable".to_string(),
                message: "gateway usage telemetry is disabled for this instance".to_string(),
            });
        };
        let limit = params
            .limit
            .unwrap_or(DEFAULT_CALLS_LIMIT)
            .min(MAX_CALLS_LIMIT);
        let (rows, total_matching) = store
            .list_calls(UsageCallsQuery {
                since_unix: params.since_unix,
                until_unix: params.until_unix,
                upstream: params.upstream,
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
