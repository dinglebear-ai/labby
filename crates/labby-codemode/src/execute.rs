//! `CodeModeBroker::execute` and the host-brokered tool-call path.

use std::sync::Arc;
use std::time::Duration;

use labby_primitives::trace::TraceContext;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::CodeModeCallError;
use crate::error::ToolError;
use crate::host::{CodeModeHost, ExecCtx, ToolCallOutcome};
use labby_runtime::gateway_config::{
    CodeModeSearchConfig, CodeModeSearchKind, CodeModeSearchSource,
};
use labby_runtime::{CodeModeConfig, CodeModeResultShapePolicy};

use super::CodeModeBroker;
use super::config::MAX_SOURCE_BYTES;
use super::normalize_user_code;
use super::shape::shape_final_result;
use super::truncate::{
    response_within_budget, result_would_be_truncated, truncate_execution_response,
};
use super::types::{
    CatalogDescriptor, CodeModeCaller, CodeModeCatalogKind, CodeModeDiscoveryEntry,
    CodeModeExecutionError, CodeModeExecutionOutcome, CodeModeExecutionResponse, CodeModeSurface,
    CodeModeToolId, CodeModeToolRef, ToolScope,
};

/// Compatibility key a Code Mode snippet can return
/// (`return { __ui: <result> }`) to unwrap the final result payload while using
/// the last-wins captured mcp-ui widget link.
const UI_OPT_IN_KEY: &str = "__ui";

/// Reserved namespace for host-internal pseudo-tool calls that are NOT real
/// Code Mode tool calls — they never reach `host.call_tool`, never consume
/// the per-run call budget, and never appear in `response.calls`. The
/// sandbox's generated JS calls these via the ordinary `callTool(id, params)`
/// primitive so no new sandbox protocol surface is needed; `call_tool_id`
/// intercepts ids in this namespace before the normal scope check.
const LAB_INTERNAL_NAMESPACE: &str = "__lab_internal";

/// Maximum accepted semantic query size in bytes for the reserved
/// `__lab_internal::semantic_rank` call. Oversized queries are truncated on a
/// char boundary — never errored — before reaching `host.semantic_rank`, so a
/// hostile sandbox cannot ship arbitrarily large payloads to the embedding
/// service. Mirrors the adjacent `limit.clamp(1, 50)` clamp-don't-reject
/// pattern (FAIL-OPEN invariant).
const MAX_SEMANTIC_QUERY_BYTES: usize = 8 * 1024;

/// Maximum identifier/URI size accepted by progressive-disclosure bridges.
const MAX_CAPABILITY_IDENTIFIER_BYTES: usize = 8 * 1024;

/// Reserve time for the host to serialize and return the final MCP result.
///
/// Downstream MCP clients commonly use a timeout close to Code Mode's
/// configured wall-clock budget. If the sandbox is allowed to consume the
/// entire budget, runner teardown and response assembly can close the bridge
/// before the canonical timeout error reaches the caller. The configured
/// timeout remains the total request budget; this margin is the response
/// delivery portion of that budget.
const CODE_MODE_RESPONSE_RESERVE_MS: u64 = 500;

fn execution_timeout(timeout_ms: u64) -> Duration {
    let timeout_ms = timeout_ms.max(1);
    let reserve = if timeout_ms >= CODE_MODE_RESPONSE_RESERVE_MS.saturating_mul(2) {
        CODE_MODE_RESPONSE_RESERVE_MS
    } else {
        0
    };
    Duration::from_millis(timeout_ms - reserve)
}

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    /// Execute Code Mode and return the configured model-facing response.
    pub async fn execute(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
        execution_id: Option<Arc<str>>,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        self.execute_with_trace_context(code, caller, surface, config, scope, execution_id, None)
            .await
    }

    /// Execute Code Mode while carrying host-owned request trace context.
    pub async fn execute_with_trace_context(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
        execution_id: Option<Arc<str>>,
        trace_context: Option<Arc<TraceContext>>,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        Ok(self
            .execute_with_raw_response_and_trace_context(
                code,
                caller,
                surface,
                config,
                scope,
                execution_id,
                trace_context,
                MAX_SOURCE_BYTES,
            )
            .await?
            .display_response)
    }

    /// Execute Code Mode and retain both raw and model-shaped responses.
    pub async fn execute_with_raw_response(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
        execution_id: Option<Arc<str>>,
    ) -> Result<CodeModeExecutionOutcome, CodeModeExecutionError> {
        self.execute_with_raw_response_and_trace_context(
            code,
            caller,
            surface,
            config,
            scope,
            execution_id,
            None,
            MAX_SOURCE_BYTES,
        )
        .await
    }

    /// The fixture harness embeds bounded snippet and fixture data in its
    /// isolated wrapper. Keep this larger source allowance local to that path.
    pub(crate) async fn execute_fixture_with_raw_response(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
    ) -> Result<CodeModeExecutionOutcome, CodeModeExecutionError> {
        self.execute_with_raw_response_and_trace_context(
            code,
            caller,
            surface,
            config,
            scope,
            None,
            None,
            10 * MAX_SOURCE_BYTES,
        )
        .await
    }

    async fn execute_with_raw_response_and_trace_context(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
        execution_id: Option<Arc<str>>,
        trace_context: Option<Arc<TraceContext>>,
        source_ceiling: usize,
    ) -> Result<CodeModeExecutionOutcome, CodeModeExecutionError> {
        // `codemode` is exposed only when the host's Code Mode surface is
        // enabled; the surface handler gates on that before reaching here.
        if !execution_allowed(&caller, &scope) {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "codemode requires one of scopes: lab, lab:admin".to_string(),
            }
            .into());
        }
        let max_source_bytes = config.max_source_bytes.min(source_ceiling);
        if code.len() > max_source_bytes {
            return Err(ToolError::InvalidParam {
                message: format!("code exceeds max length {max_source_bytes} bytes"),
                param: "code".to_string(),
            }
            .into());
        }
        let started = std::time::Instant::now();
        let artifact_run_id = ulid::Ulid::new().to_string();
        let _active_artifact_run = crate::artifacts::ActiveArtifactRun::register(&artifact_run_id);
        let mut response = self
            .execute_sandboxed(
                code,
                execution_timeout(config.timeout_ms),
                max_source_bytes,
                caller.clone(),
                surface,
                config.max_log_entries,
                config.max_log_bytes,
                config.trace_params,
                scope.clone(),
                execution_id,
                trace_context,
                artifact_run_id.clone(),
            )
            .await?;
        // Surface any last-wins captured mcp-ui widget link. `{ __ui: <result> }`
        // remains a compatibility form that also unwraps the inner payload.
        // Done before truncation so the (tiny) `ui` field is preserved while
        // `result` may be capped.
        self.apply_ui_opt_in(&mut response);
        // Preserve only outputs that the configured shaping/truncation path would change.
        if result_would_be_truncated(
            &mut response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        ) {
            if let Some(value) = response.result.as_ref() {
                let root = crate::artifacts::code_mode_artifact_root(&artifact_run_id);
                if let Some(receipt) = crate::response_artifacts::preserve(
                    &root,
                    "automatic/final-result.json".into(),
                    value,
                    &caller,
                    &scope,
                )
                .await
                {
                    response.artifacts.push(receipt);
                }
            }
        }
        let raw_response = response.clone();
        let shaped = shape_final_result(
            response.result.take(),
            config.result_shape_policy,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        let result_shape_changed = shaped.metadata.changed;
        let result_shape_truncated = shaped.metadata.truncated;
        let result_shape_original_size_bytes = shaped.metadata.original_size_bytes;
        let result_shape_shaped_size_bytes = shaped.metadata.shaped_size_bytes;
        let result_shape_warning_requested = shaped.metadata.warning.is_some();
        response.result = shaped.result;
        if config.result_shape_policy != CodeModeResultShapePolicy::Off
            || result_shape_warning_requested
        {
            response.result_shaping = Some(shaped.metadata);
        }
        remove_soft_warning_if_it_breaks_budget(&mut response, &config);
        let result_shape_warning = response
            .result_shaping
            .as_ref()
            .and_then(|metadata| metadata.warning.as_ref())
            .is_some();
        let was_truncated = !response_within_budget(
            &response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        let response = truncate_execution_response(
            response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        tracing::info!(
            surface = "dispatch",
            service = "code_mode",
            action = "codemode",
            tool_calls = response.calls.len(),
            elapsed_ms = started.elapsed().as_millis(),
            result_bytes = response
                .result
                .as_ref()
                .map(|v| v.to_string().len())
                .unwrap_or(0),
            result_shape_policy = ?config.result_shape_policy,
            result_shape_changed,
            result_shape_truncated,
            result_shape_original_size_bytes,
            result_shape_shaped_size_bytes,
            result_shape_warning,
            logs_count = response.logs.len(),
            truncated = was_truncated,
            "code execution complete"
        );
        Ok(CodeModeExecutionOutcome {
            raw_response,
            display_response: response,
        })
    }

    /// Apply a captured MCP App widget link to a finished response.
    ///
    /// When the user code's return value is an object with a `__ui` key, the
    /// inner value is unwrapped into `result` for compatibility with the older
    /// wrapper convention. Either way, if the run captured a widget-bearing
    /// result, attach the last-wins link to `ui`.
    fn apply_ui_opt_in(&self, response: &mut CodeModeExecutionResponse) {
        // Clone the inner value out (ending the borrow of `response.result`)
        // before reassigning. No `__ui` key → keep the result as-is.
        let inner = match response.result.as_ref() {
            Some(Value::Object(map)) => map.get(UI_OPT_IN_KEY).cloned(),
            _ => None,
        };
        let had_ui_opt_in = inner.is_some();
        if let Some(inner) = inner {
            response.result = Some(inner);
        }
        if let Ok(mut sink) = self.ui_capture.lock() {
            response.ui = sink.take();
            match response.ui.as_ref() {
                Some(ui) => tracing::info!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "mcp_app.opt_in",
                    resource_uri = ui_resource_uri(&ui.ui_meta).unwrap_or("<unknown>"),
                    "attached captured MCP App widget to execute response"
                ),
                None if had_ui_opt_in => {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "mcp_app.opt_in",
                        kind = "ui_capture_missing",
                        "Code Mode returned __ui but no MCP App widget was captured"
                    );
                }
                None => {}
            }
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "mcp_app.opt_in",
                kind = "ui_capture_lock_poisoned",
                "Code Mode returned __ui but captured MCP App widget could not be read"
            );
        }
    }
}

mod dispatch;
mod policy;
mod proxy;
pub(crate) use policy::openapi_provider_allowed;
use policy::*;
pub use policy::{discovery_entry_visible, discovery_render_params, local_providers_allowed};
#[cfg(test)]
mod scoped_call_id_tests;
#[cfg(test)]
mod tests;
