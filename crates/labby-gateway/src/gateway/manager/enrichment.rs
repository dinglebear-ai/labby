use labby_runtime::catalog_notify::SOURCE_GATEWAY_ENRICH_HINT;
use labby_runtime::error::ToolError;

use crate::gateway::enrichment::collector::{
    EnrichmentInputStats, MAX_MANUAL_UPSTREAMS, MAX_PROMPTS_PER_UPSTREAM, MAX_PROVIDER_INPUT_BYTES,
    MAX_RESOURCES_PER_UPSTREAM, MAX_TOOLS_PER_UPSTREAM, MAX_TOTAL_TOOLS, SelectedUpstream,
    UpstreamEnrichmentInput, collect_enrichment_inputs, select_upstreams_for_preview,
};
use crate::gateway::enrichment::provider::{
    DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_TIMEOUT_MS, MAX_TIMEOUT_MS, MIN_TIMEOUT_MS,
    PROVIDER_CONCURRENCY, PROVIDER_RATE_LIMIT_PER_MINUTE, ProviderRunner,
    in_flight_provider_runs, provider_metrics, run_provider_preview, waiting_provider_runs,
};
use crate::gateway::params::{
    GatewayEnrichApplyParams, GatewayEnrichPreviewParams, GatewayEnrichmentScope,
};
use crate::gateway::types::{
    GatewayCatalogDiff, GatewayEnrichmentHintView, GatewayEnrichmentLimitsView,
    GatewayEnrichmentPreviewStatsView, GatewayEnrichmentPreviewView, GatewayEnrichmentProvider,
    GatewayEnrichmentStatusView, GatewayHintApplyView, GatewayHintProposalStatus,
    GatewayHintProposalView,
};

use super::GatewayManager;

impl From<EnrichmentInputStats> for GatewayEnrichmentPreviewStatsView {
    fn from(stats: EnrichmentInputStats) -> Self {
        Self {
            bytes: stats.bytes,
            upstream_count: stats.upstream_count,
            tool_count: stats.tool_count,
            truncated: stats.truncated,
        }
    }
}

impl GatewayManager {
    pub async fn preview_enrichment(
        &self,
        params: GatewayEnrichPreviewParams,
    ) -> Result<GatewayEnrichmentPreviewView, ToolError> {
        self.preview_enrichment_scoped(params, GatewayEnrichmentScope::default())
            .await
    }

    pub(crate) async fn preview_enrichment_scoped(
        &self,
        mut params: GatewayEnrichPreviewParams,
        scope: GatewayEnrichmentScope,
    ) -> Result<GatewayEnrichmentPreviewView, ToolError> {
        let cfg = self.current_config().await;
        let selection = select_upstreams_for_preview(&cfg, &params, &scope)?;
        let pool = self.current_pool().await;
        let mut collected =
            collect_enrichment_inputs(pool.as_deref(), &cfg, &selection.upstreams).await?;
        if selection.truncated {
            collected.stats.truncated = true;
        }
        let mut runner = ProviderRunner::default();
        if let Some(timeout_ms) = params.timeout_ms.take() {
            runner.timeout_ms = timeout_ms;
        }
        let mut proposals =
            run_provider_preview(params.provider, &collected.inputs, &runner).await?;
        proposals.extend(
            collected
                .omitted_inputs
                .iter()
                .map(|input| omitted_input_proposal(input, params.provider)),
        );
        Ok(GatewayEnrichmentPreviewView {
            provider: params.provider,
            stats: collected.stats.into(),
            proposals,
        })
    }

    pub(crate) async fn enrichment_status_scoped(
        &self,
        scope: GatewayEnrichmentScope,
    ) -> GatewayEnrichmentStatusView {
        let cfg = self.current_config().await;
        let mut hints = cfg
            .upstream
            .into_iter()
            .filter(|upstream| {
                scope
                    .route_visible_upstreams
                    .as_ref()
                    .is_none_or(|visible| visible.contains(&upstream.name))
            })
            .map(|upstream| GatewayEnrichmentHintView {
                upstream: upstream.name,
                enabled: upstream.enabled,
                hint: upstream
                    .code_mode_hint
                    .as_deref()
                    .and_then(labby_runtime::gateway_config::normalize_code_mode_hint),
            })
            .collect::<Vec<_>>();
        hints.sort_by(|left, right| left.upstream.cmp(&right.upstream));
        let hinted_upstream_count = hints.iter().filter(|entry| entry.hint.is_some()).count();
        let visible_upstream_count = hints.len();

        GatewayEnrichmentStatusView {
            hints,
            hinted_upstream_count,
            visible_upstream_count,
            providers: provider_metrics(),
            waiting_provider_runs: waiting_provider_runs(),
            in_flight_provider_runs: in_flight_provider_runs(),
            limits: GatewayEnrichmentLimitsView {
                max_manual_upstreams: MAX_MANUAL_UPSTREAMS,
                max_tools_per_upstream: MAX_TOOLS_PER_UPSTREAM,
                max_total_tools: MAX_TOTAL_TOOLS,
                max_resources_per_upstream: MAX_RESOURCES_PER_UPSTREAM,
                max_prompts_per_upstream: MAX_PROMPTS_PER_UPSTREAM,
                max_provider_input_bytes: MAX_PROVIDER_INPUT_BYTES,
                provider_concurrency: PROVIDER_CONCURRENCY,
                default_timeout_ms: DEFAULT_TIMEOUT_MS,
                min_timeout_ms: MIN_TIMEOUT_MS,
                max_timeout_ms: MAX_TIMEOUT_MS,
                max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
                provider_rate_limit_per_minute: PROVIDER_RATE_LIMIT_PER_MINUTE as u64,
                automatic_provider: GatewayEnrichmentProvider::Deterministic,
                automatic_timeout_ms: 2_000,
                automatic_max_upstreams: 1,
            },
        }
    }

    pub async fn apply_enrichment(
        &self,
        params: GatewayEnrichApplyParams,
    ) -> Result<GatewayHintApplyView, ToolError> {
        self.apply_enrichment_scoped(params, GatewayEnrichmentScope::default())
            .await
    }

    pub(crate) async fn apply_enrichment_scoped(
        &self,
        params: GatewayEnrichApplyParams,
        scope: GatewayEnrichmentScope,
    ) -> Result<GatewayHintApplyView, ToolError> {
        scope.ensure_visible(&params.upstream)?;
        let hint = validate_hint(&params.hint)?;
        let _mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let pool = self.current_pool().await;
        let selected = [SelectedUpstream {
            name: params.upstream.clone(),
            explicit: true,
        }];
        let collected = collect_enrichment_inputs(pool.as_deref(), &cfg, &selected).await?;
        let current_hash = collected
            .inputs
            .first()
            .map(|input| input.metadata_hash.as_str())
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown gateway upstream `{}`", params.upstream),
            })?;
        if current_hash != params.metadata_hash {
            return Err(ToolError::Sdk {
                sdk_kind: "stale_suggestion".to_string(),
                message:
                    "gateway enrichment suggestion no longer matches current upstream metadata"
                        .to_string(),
            });
        }

        let upstream = cfg
            .upstream
            .iter_mut()
            .find(|upstream| upstream.name == params.upstream)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "unknown_upstream".to_string(),
                message: format!("unknown gateway upstream `{}`", params.upstream),
            })?;
        let previous_hint = upstream
            .code_mode_hint
            .as_deref()
            .and_then(labby_runtime::gateway_config::normalize_code_mode_hint);
        upstream.code_mode_hint = Some(hint.clone());
        self.persist_config_owned(_mutation_guard, cfg).await?;
        // A `code_mode_hint` is rendered into the visible `codemode` tool
        // description (the "## Upstreams" section built in
        // `mcp/call_tool_codemode/description.rs`), so applying a
        // hint genuinely changes the externally visible tool contract and must
        // notify. Only the tool descriptor changes — resources and prompts do
        // not — so this is a tools-only change.
        //
        // `hint_unchanged` marks the one case where this notification is known
        // to be spurious: re-applying a hint that normalizes to the value
        // already stored. The notification is still sent (suppressing it is a
        // behavior change owned by the notification-coalescing work), but it is
        // labelled so it can be counted rather than mistaken for real churn.
        let hint_unchanged = previous_hint.as_deref()
            == labby_runtime::gateway_config::normalize_code_mode_hint(&hint).as_deref();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.enrich.hint.apply",
            event = "catalog.notify.intent",
            source = SOURCE_GATEWAY_ENRICH_HINT,
            upstream = %params.upstream,
            hint_unchanged,
            "gateway enrichment hint applied; notifying catalog change"
        );
        self.notify_catalog_changes(
            &GatewayCatalogDiff {
                tools_changed: true,
                resources_changed: false,
                prompts_changed: false,
            },
            SOURCE_GATEWAY_ENRICH_HINT,
        );

        Ok(GatewayHintApplyView {
            upstream: params.upstream,
            hint,
            applied: true,
            previous_hint,
        })
    }

    pub(crate) async fn preview_enrichment_for_new_upstream(
        &self,
        upstream: &str,
        scope: GatewayEnrichmentScope,
    ) -> (Option<GatewayHintProposalView>, Option<String>) {
        let preview_result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.preview_enrichment_scoped(
                GatewayEnrichPreviewParams {
                    upstreams: vec![upstream.to_string()],
                    all: false,
                    provider: GatewayEnrichmentProvider::Deterministic,
                    max_upstreams: Some(1),
                    timeout_ms: Some(2_000),
                },
                scope,
            ),
        )
        .await;
        let preview = match preview_result {
            Ok(Ok(preview)) => preview,
            Ok(Err(err)) => {
                let message = err.to_string();
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.enrich.preview",
                    upstream,
                    kind = %err.kind(),
                    error = %message,
                    "gateway enrichment suggestion skipped"
                );
                return (None, Some(message));
            }
            Err(_) => {
                let message = "gateway enrichment suggestion timed out".to_string();
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "gateway.enrich.preview",
                    upstream,
                    kind = "timeout",
                    error = %message,
                    "gateway enrichment suggestion skipped"
                );
                return (None, Some(message));
            }
        };
        (preview.proposals.into_iter().next(), None)
    }
}

fn validate_hint(hint: &str) -> Result<String, ToolError> {
    labby_runtime::gateway_config::normalize_code_mode_hint(hint).ok_or_else(|| ToolError::Sdk {
        sdk_kind: "invalid_hint".to_string(),
        message:
            "code mode hint must be plain, non-instructional text from 1-240 characters on one line"
                .to_string(),
    })
}

fn omitted_input_proposal(
    input: &UpstreamEnrichmentInput,
    provider: GatewayEnrichmentProvider,
) -> GatewayHintProposalView {
    let existing_hint = input.existing_hint.clone();
    let status = if existing_hint.is_some() {
        GatewayHintProposalStatus::Existing
    } else {
        GatewayHintProposalStatus::MetadataInsufficient
    };
    GatewayHintProposalView {
        upstream: input.name.clone(),
        hint: existing_hint.clone(),
        status,
        metadata_hash: input.metadata_hash.clone(),
        provider,
        tool_count: input.tool_names.len(),
        resource_count: input.resource_count,
        prompt_count: input.prompt_count,
        existing_hint,
    }
}
