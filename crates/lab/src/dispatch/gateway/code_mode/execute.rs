//! `CodeModeBroker::execute` and the upstream tool-call path.

use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Map, Value};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;

use super::CodeModeBroker;
use super::normalize_user_code;
use super::runner_io::code_mode_upstream_error_info;
use super::schema::{unwrap_code_mode_upstream_result, validate_code_mode_params_against_schema};
use super::truncate::{response_within_budget, truncate_execution_response};
use super::types::{
    CodeModeCaller, CodeModeCapabilityFilter, CodeModeDiscoveryEntry, CodeModeExecutionError,
    CodeModeExecutionResponse, CodeModeSurface, CodeModeToolId, CodeModeToolRef, UiLink,
    destructive_permitted,
};

/// Compatibility key a Code Mode snippet can return
/// (`return { __ui: <result> }`) to unwrap the final result payload while using
/// the last-wins captured mcp-ui widget link.
const UI_OPT_IN_KEY: &str = "__ui";

impl CodeModeBroker<'_> {
    /// Execute Code Mode source and (when a recording context is supplied)
    /// record the execution-history entry + source snapshot uniformly for every
    /// surface.
    ///
    /// Recording lives here, in the shared dispatch broker, rather than in the
    /// MCP/CLI adapters: per `docs/dev/DISPATCH.md` the sandbox parent broker
    /// owns Code Mode operation semantics while surfaces only adapt inputs and
    /// outputs. `ctx = None` skips recording — used by the snippet host path
    /// (`codemode.run`), which executes *inside* an already-recorded run, and by
    /// unit tests that don't assert telemetry.
    pub(crate) async fn execute(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: crate::config::CodeModeConfig,
        capability_filter: CodeModeCapabilityFilter,
        ctx: Option<super::types::CodeModeExecuteContext>,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let started = std::time::Instant::now();
        let mut result = self
            .execute_inner(code, caller, surface, config, capability_filter, started)
            .await;
        if let Some(ctx) = ctx {
            // Stamp the execution id onto the success response before recording
            // so the recorded output-token estimate matches the response the
            // surface returns (and surfaces no longer assign it themselves).
            if let Ok(response) = &mut result {
                response.execution_id = Some(ctx.execution_id.clone());
            }
            self.record_execution(code, surface, &result, &ctx, started.elapsed().as_millis())
                .await;
        }
        result
    }

    async fn execute_inner(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: crate::config::CodeModeConfig,
        capability_filter: CodeModeCapabilityFilter,
        started: std::time::Instant,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        // `codemode` is exposed only when the gateway Code Mode surface is
        // enabled (code_mode.enabled -> RootSynthetic), and the MCP handler
        // gates on `exposes_synthetic_tools()` before reaching here.
        if !caller.can_execute() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "codemode requires one of scopes: lab, lab:admin".to_string(),
            }
            .into());
        }
        let mut response = self
            .execute_sandboxed(
                code,
                Duration::from_millis(config.timeout_ms.max(1)),
                caller,
                surface,
                config.max_log_entries,
                config.max_log_bytes,
                config.trace_params,
                capability_filter,
            )
            .await?;
        // Surface any last-wins captured mcp-ui widget link. `{ __ui: <result> }`
        // remains a compatibility form that also unwraps the inner payload.
        // Done before truncation so the (tiny) `ui` field is preserved while
        // `result` may be capped.
        self.apply_ui_opt_in(&mut response);
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
            logs_count = response.logs.len(),
            truncated = was_truncated,
            "code execution complete"
        );
        Ok(response)
    }

    /// Record execution history (always) and source (admin + within size limit)
    /// for one execution, shared by every surface. No-op when the broker has no
    /// `GatewayManager` (standalone/test brokers have nowhere to record).
    async fn record_execution(
        &self,
        code: &str,
        surface: CodeModeSurface,
        result: &Result<CodeModeExecutionResponse, CodeModeExecutionError>,
        ctx: &super::types::CodeModeExecuteContext,
        elapsed_ms: u128,
    ) {
        use super::types::{CodeModeExecutionSource, CodeModeHistoryEntry, CodeModeHistoryKind};

        let Some(manager) = self.gateway_manager else {
            return;
        };

        let (ok, calls, error_kind, output_tokens) = match result {
            Ok(response) => {
                let output = serde_json::to_string(response).unwrap_or_else(|_| "{}".to_string());
                (
                    true,
                    response.calls.clone(),
                    None,
                    crate::dispatch::helpers::estimate_tokens(&output),
                )
            }
            Err(err) => (false, err.calls().to_vec(), Some(err.kind().to_string()), 0),
        };

        manager
            .record_code_mode_history(CodeModeHistoryEntry {
                execution_id: Some(ctx.execution_id.clone()),
                seq: 0,
                route_scope: ctx.route_scope.clone(),
                kind: CodeModeHistoryKind::Execute,
                ok,
                elapsed_ms,
                input_tokens: Some(ctx.input_tokens),
                output_tokens: Some(output_tokens),
                error_kind,
                calls,
                match_count: None,
            })
            .await;

        // Source is recorded only for admins and only up to the shared
        // source-size limit, matching the prior MCP-only behavior.
        if ctx.is_admin && code.len() <= super::config::MAX_SOURCE_BYTES {
            manager
                .record_code_mode_source(CodeModeExecutionSource {
                    execution_id: ctx.execution_id.clone(),
                    created_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis() as i64)
                        .unwrap_or_default(),
                    actor_key: ctx.actor_key.clone(),
                    is_admin: ctx.is_admin,
                    route_scope: ctx.route_scope.clone(),
                    surface,
                    capability_filter_fingerprint: ctx.capability_filter_fingerprint.clone(),
                    code: code.to_string(),
                })
                .await;
        }
    }

    async fn build_code_mode_proxy(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        capability_filter: &CodeModeCapabilityFilter,
    ) -> Result<String, ToolError> {
        let Some(manager) = self.gateway_manager else {
            return Ok(String::new());
        };
        let owner = caller.runtime_owner(surface);
        let oauth_subject = caller.oauth_subject();
        let include_snippets =
            caller.can_use_snippets() && !capability_filter.is_scoped_to_upstreams();
        let allowed_upstreams = capability_filter.allowed_upstreams();
        // The execute proxy only needs a usable `codemode.*` namespace; it does
        // NOT need tool-list growth detection (callTool resolves its target
        // upstream live, so a stale proxy can only mis-shape helper names). So
        // we connect-if-needed and skip already-healthy upstreams rather than
        // re-probing every enabled upstream on every execute. The CLI one-shot
        // path stays on the on-disk catalog cache; the full growth-detecting
        // reprobe stays on the `search`/catalog path. See lab-5h5xl.
        let (catalog, _catalog_json, _serialized_size) =
            if surface == CodeModeSurface::Cli && allowed_upstreams.is_none() {
                self.cached_code_mode_catalog_for_proxy(
                    manager,
                    &owner,
                    oauth_subject,
                    include_snippets,
                )
                .await?
            } else {
                self.ensure_ready_code_mode_catalog_for_proxy(
                    manager,
                    &owner,
                    oauth_subject,
                    allowed_upstreams,
                    include_snippets,
                )
                .await?
            };
        let catalog = catalog
            .into_iter()
            .filter(|entry| {
                entry.kind == super::types::CodeModeCatalogKind::Snippet
                    || capability_filter.allows(&entry.upstream, &entry.name)
            })
            .collect::<Vec<_>>();
        let mut upstreams: Vec<String> = catalog
            .iter()
            .filter(|entry| entry.kind == super::types::CodeModeCatalogKind::Tool)
            .map(|entry| entry.upstream.clone())
            .collect();
        upstreams.sort();
        upstreams.dedup();

        // --- lab-um27z: proxy JS render cache ---
        // The emitted proxy depends only on the (already filtered) catalog
        // shape, the upstream list, snippet visibility, and the capability
        // filter. Key on exactly those so a hit can skip the BTreeMap grouping
        // and per-tool `function(p){…}` emission entirely. The capability filter
        // is folded in because `catalog` above is filtered per-call — a key that
        // ignored it could serve a wrong-scoped proxy.
        let proxy_cache_key =
            proxy_render_cache_key(&catalog, &upstreams, include_snippets, capability_filter);
        if let Some((discovery_js, namespace_js)) =
            manager.cached_code_mode_proxy(&proxy_cache_key).await
        {
            tracing::debug!(
                surface = "dispatch",
                service = "code_mode",
                action = "proxy.build",
                upstream_count = upstreams.len(),
                "Code Mode proxy JS served from render cache"
            );
            return Ok(format!("{discovery_js}\n{namespace_js}"));
        }

        let discovery_entries = catalog
            .iter()
            .map(CodeModeDiscoveryEntry::from_catalog)
            .collect::<Vec<_>>();
        let discovery_js =
            super::preamble::generate_discovery_js(&discovery_entries).map_err(|message| {
                ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message,
                }
            })?;
        let tool_entries = catalog
            .iter()
            .filter(|entry| entry.kind == super::types::CodeModeCatalogKind::Tool)
            .collect::<Vec<_>>();
        let namespace_js =
            super::preamble::generate_js_proxy_from_catalog(&tool_entries, &upstreams).map_err(
                |message| ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message,
                },
            )?;
        manager
            .store_code_mode_proxy(super::ProxyRenderCache {
                key: proxy_cache_key,
                discovery_js: discovery_js.clone(),
                namespace_js: namespace_js.clone(),
            })
            .await;
        Ok(format!("{discovery_js}\n{namespace_js}"))
    }

    #[cfg(test)]
    pub(crate) async fn build_code_mode_proxy_for_tests(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        capability_filter: &CodeModeCapabilityFilter,
    ) -> Result<String, ToolError> {
        self.build_code_mode_proxy(caller, surface, capability_filter)
            .await
    }

    async fn execute_sandboxed(
        &self,
        code: &str,
        timeout: Duration,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        max_log_entries: usize,
        max_log_bytes: usize,
        trace_params: bool,
        capability_filter: CodeModeCapabilityFilter,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        // Cloudflare-parity: no typed TypeScript preamble is injected. The
        // sandbox exposes only `callTool(id, params)`; the agent uses tool ids
        // discovered via `search`. Normalize the user code and run it directly.
        let code_to_run = normalize_user_code(code);

        // Build the runtime `codemode.*` proxy from the live upstream catalog
        // (same source `search` uses) before starting the runner. Proxy failure is
        // an execution failure: otherwise `codemode.search`, `codemode.describe`,
        // and generated helpers silently disappear while raw `callTool` can still
        // make the run look successful.
        let deadline = tokio::time::Instant::now() + timeout;
        let proxy = match tokio::time::timeout_at(
            deadline,
            self.build_code_mode_proxy(&caller, surface, &capability_filter),
        )
        .await
        {
            Ok(Ok(proxy)) => proxy,
            Ok(Err(err)) => {
                tracing::warn!(kind = err.kind(), "code_mode.proxy_generation_failed");
                return Err(err.into());
            }
            Err(_elapsed) => {
                tracing::warn!(
                    timeout_ms = timeout.as_millis(),
                    "code_mode.proxy_generation_timed_out"
                );
                return Err(ToolError::Sdk {
                    sdk_kind: "timeout".to_string(),
                    message: "Code Mode proxy generation timed out".to_string(),
                }
                .into());
            }
        };
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or_default();
        if remaining.is_zero() {
            return Err(ToolError::Sdk {
                sdk_kind: "timeout".to_string(),
                message: "Code Mode execution timed out before sandbox start".to_string(),
            }
            .into());
        }

        self.run_in_runner(super::runner_drive::RunnerConfig {
            code_to_run,
            proxy,
            timeout: remaining,
            caller,
            surface,
            max_log_entries,
            max_log_bytes,
            trace_params,
            capability_filter,
        })
        .await
    }

    pub(crate) async fn call_tool_id_before_deadline(
        &self,
        id: &str,
        params: Value,
        deadline: tokio::time::Instant,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        capability_filter: &CodeModeCapabilityFilter,
    ) -> Result<Value, ToolError> {
        match tokio::time::timeout_at(
            deadline,
            self.call_tool_id(id, params, caller, surface, capability_filter),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ToolError::Sdk {
                sdk_kind: "timeout".to_string(),
                message: "Code Mode execution timed out".to_string(),
            }),
        }
    }

    pub(crate) async fn call_tool_id(
        &self,
        id: &str,
        params: Value,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        capability_filter: &CodeModeCapabilityFilter,
    ) -> Result<Value, ToolError> {
        let parsed = CodeModeToolId::parse(id)?;
        let Some(manager) = self.gateway_manager else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "no gateway manager configured".to_string(),
            });
        };
        match parsed.reference {
            CodeModeToolRef::UpstreamTool { upstream, tool } => {
                if !capability_filter.allows(&upstream, &tool) {
                    return Err(ToolError::Sdk {
                        sdk_kind: "unknown_tool".to_string(),
                        message: format!(
                            "upstream tool `{}` is outside this Code Mode execution capability set",
                            parsed.raw
                        ),
                    });
                }
                let owner = caller.runtime_owner(surface);
                let oauth_subject = caller.oauth_subject();
                self.call_upstream_tool(
                    manager,
                    &upstream,
                    &tool,
                    params,
                    &owner,
                    oauth_subject,
                    surface,
                    &caller,
                )
                .await
            }
        }
    }

    async fn call_upstream_tool(
        &self,
        manager: &GatewayManager,
        upstream: &str,
        tool: &str,
        params: Value,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        surface: CodeModeSurface,
        caller: &CodeModeCaller,
    ) -> Result<Value, ToolError> {
        let upstream_tool = manager
            .resolve_code_mode_upstream_tool(upstream, tool, Some(owner), oauth_subject)
            .await?;

        // Host-side scope check: destructive upstream metadata does not add a
        // second confirmation model, but read-only callers still cannot execute.
        if upstream_tool.destructive && !destructive_permitted(surface, caller) {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                upstream = upstream,
                tool = tool,
                kind = "forbidden",
                "blocked destructive Code Mode tool call for non-execute caller"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!(
                    "Tool `{upstream}::{tool}` requires Code Mode execute permission."
                ),
            });
        }
        validate_code_mode_params_against_schema(&params, upstream_tool.input_schema.as_ref())?;
        let Some(pool) = manager.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: "gateway upstream pool is unavailable".to_string(),
            });
        };
        let mut upstream_params = CallToolRequestParams::new(tool.to_string());
        upstream_params.arguments = Some(match params {
            Value::Object(map) => map,
            _ => Map::new(),
        });
        match pool.call_tool(upstream, upstream_params).await {
            Some(Ok(result)) => {
                if result.is_error == Some(true) {
                    let error_text = result
                        .content
                        .first()
                        .and_then(|content| content.as_text())
                        .map(|content| content.text.as_str());
                    let (kind, message, counts_as_failure) =
                        code_mode_upstream_error_info(error_text);
                    if counts_as_failure {
                        pool.record_failure(upstream, message.clone()).await;
                    } else {
                        pool.record_success(upstream).await;
                    }
                    return Err(ToolError::Sdk {
                        sdk_kind: kind.to_string(),
                        message,
                    });
                }
                pool.record_success(upstream).await;
                // Capture the mcp-ui widget link (if any) before the envelope is
                // unwrapped and `_meta` is discarded. Last-wins across the run;
                // surfaced on the final execute response.
                if let Some(ui) = extract_ui_link(&result) {
                    let resource_uri = ui_resource_uri(&ui.ui_meta).unwrap_or("<unknown>");
                    tracing::info!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "mcp_app.capture",
                        upstream,
                        tool,
                        resource_uri,
                        "captured upstream MCP App widget link"
                    );
                    if let Ok(mut sink) = self.ui_capture.lock() {
                        *sink = Some(ui);
                    } else {
                        tracing::warn!(
                            surface = "dispatch",
                            service = "code_mode",
                            action = "mcp_app.capture",
                            upstream,
                            tool,
                            resource_uri,
                            kind = "ui_capture_lock_poisoned",
                            "failed to store upstream MCP App widget link"
                        );
                    }
                } else {
                    tracing::debug!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "mcp_app.capture",
                        upstream,
                        tool,
                        "upstream result did not include _meta.ui.resourceUri"
                    );
                }
                Ok(unwrap_code_mode_upstream_result(result))
            }
            Some(Err(err)) => {
                pool.record_failure(upstream, err.clone()).await;
                Err(ToolError::Sdk {
                    sdk_kind: "upstream_error".to_string(),
                    message: err,
                })
            }
            None => {
                pool.record_failure(upstream, format!("upstream `{upstream}` is not connected"))
                    .await;
                Err(ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("upstream tool `{upstream}::{tool}` was not found"),
                })
            }
        }
    }

    /// Apply a captured upstream MCP App widget link to a finished response.
    ///
    /// When the user code's return value is an object with a `__ui` key, the
    /// inner value is unwrapped into `result` for compatibility with the older
    /// wrapper convention. Either way, if the run captured a widget-bearing
    /// upstream result, attach the last-wins link to `ui`.
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
                        "Code Mode returned __ui but no upstream MCP App widget was captured"
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

/// Extract a UI link from an upstream `CallToolResult`'s `_meta.ui` object.
///
/// Returns `None` unless `_meta.ui.resourceUri` is present. The whole `ui`
/// object is captured verbatim so the final `execute` `CallToolResult` mirrors
/// the upstream's `_meta.ui` identically.
fn extract_ui_link(result: &CallToolResult) -> Option<UiLink> {
    let meta = result.meta.as_ref()?;
    let ui = meta.get("ui")?;
    // Require a string `resourceUri` to treat this as a renderable widget link;
    // capture the whole `ui` object verbatim for identical mirroring.
    ui.get("resourceUri")?.as_str()?;
    Some(UiLink {
        ui_meta: ui.clone(),
    })
}

fn ui_resource_uri(ui_meta: &Value) -> Option<&str> {
    ui_meta.get("resourceUri").and_then(Value::as_str)
}

/// Build the proxy JS render-cache key (lab-um27z).
///
/// Captures everything the emitted `codemode.*` proxy depends on:
/// - the sorted set of catalog entry ids (tools as `upstream::name`, snippets
///   by their id) — the same identity the catalog render cache keys on, so the
///   proxy cache shares the catalog cache's staleness boundary;
/// - the sorted upstream list (the proxy emits per-upstream namespaces);
/// - the snippet-visibility flag;
/// - the per-call capability filter fingerprint (the `catalog` is filtered
///   per-call, so an unfiltered key could serve a wrong-scoped proxy).
fn proxy_render_cache_key(
    catalog: &[super::types::CodeModeCatalogEntry],
    upstreams: &[String],
    include_snippets: bool,
    capability_filter: &CodeModeCapabilityFilter,
) -> String {
    let mut ids: Vec<&str> = catalog.iter().map(|entry| entry.id.as_str()).collect();
    ids.sort_unstable();
    format!(
        "snippets:{include_snippets}\nfilter:{}\nupstreams:{}\nids:\n{}",
        capability_filter.fingerprint(),
        upstreams.join(","),
        ids.join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{Content, Meta};
    use serde_json::json;

    /// Broker-level recording parity (lab-xvmti): `record_execution` is the
    /// single recording path every surface (MCP, CLI, future HTTP) drives, so a
    /// success and a failure each record one history entry, and an admin caller
    /// records one source snapshot per execution regardless of outcome. Driving
    /// it directly proves the broker — not the surface — owns recording.
    #[tokio::test]
    async fn record_execution_records_history_and_admin_source_uniformly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = super::super::super::runtime::GatewayRuntimeHandle::default();
        let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
        let broker = CodeModeBroker::new(Some(&manager));

        let ctx = super::super::types::CodeModeExecuteContext {
            execution_id: "01XVMTI_OK".to_string(),
            route_scope: "root".to_string(),
            actor_key: Some("actor-1".to_string()),
            is_admin: true,
            capability_filter_fingerprint: "fp".to_string(),
            input_tokens: 4,
        };

        // Success path: one history entry (ok=true) + one source snapshot.
        let ok_response: Result<CodeModeExecutionResponse, CodeModeExecutionError> =
            Ok(response_with_result(json!(1)));
        broker
            .record_execution("async () => 1", CodeModeSurface::Cli, &ok_response, &ctx, 7)
            .await;

        // Failure path with a fresh execution id: one more history entry
        // (ok=false) + one more source snapshot (admin records on failure too).
        let err_ctx = super::super::types::CodeModeExecuteContext {
            execution_id: "01XVMTI_ERR".to_string(),
            ..ctx.clone()
        };
        let err_response: Result<CodeModeExecutionResponse, CodeModeExecutionError> =
            Err(ToolError::Sdk {
                sdk_kind: "timeout".to_string(),
                message: "boom".to_string(),
            }
            .into());
        broker
            .record_execution(
                "async () => 2",
                CodeModeSurface::Mcp,
                &err_response,
                &err_ctx,
                9,
            )
            .await;

        let history = manager.code_mode_history_snapshot().await;
        assert_eq!(history.len(), 2, "both executions recorded a history entry");
        assert!(history.iter().any(|h| h.ok), "success entry present");
        assert!(history.iter().any(|h| !h.ok), "failure entry present");
        assert!(
            history
                .iter()
                .any(|h| h.error_kind.as_deref() == Some("timeout")),
            "failure entry carries the error kind"
        );

        // Both source snapshots are resolvable for the admin actor.
        let lookup = super::super::types::CodeModeSourceLookup {
            actor_key: Some("actor-1".to_string()),
            is_admin: true,
            route_scope: "root".to_string(),
            capability_filter_fingerprint: "fp".to_string(),
        };
        for id in ["01XVMTI_OK", "01XVMTI_ERR"] {
            assert!(
                manager.resolve_code_mode_source(id, &lookup).await.is_ok(),
                "admin source recorded for {id}"
            );
        }
    }

    /// A non-admin caller records history but never a source snapshot.
    #[tokio::test]
    async fn record_execution_skips_source_for_non_admin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = super::super::super::runtime::GatewayRuntimeHandle::default();
        let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
        let broker = CodeModeBroker::new(Some(&manager));

        let ctx = super::super::types::CodeModeExecuteContext {
            execution_id: "01XVMTI_NOADMIN".to_string(),
            route_scope: "root".to_string(),
            actor_key: None,
            is_admin: false,
            capability_filter_fingerprint: String::new(),
            input_tokens: 1,
        };
        let response: Result<CodeModeExecutionResponse, CodeModeExecutionError> =
            Ok(response_with_result(json!(1)));
        broker
            .record_execution("async () => 1", CodeModeSurface::Cli, &response, &ctx, 3)
            .await;

        assert_eq!(
            manager.code_mode_history_snapshot().await.len(),
            1,
            "history is recorded for non-admin callers"
        );
        // Look up as admin so an Err here means "no source recorded"
        // (unknown_execution), not merely "reader lacks admin".
        let lookup = super::super::types::CodeModeSourceLookup {
            actor_key: None,
            is_admin: true,
            route_scope: "root".to_string(),
            capability_filter_fingerprint: String::new(),
        };
        assert!(
            manager
                .resolve_code_mode_source("01XVMTI_NOADMIN", &lookup)
                .await
                .is_err(),
            "no source snapshot recorded for a non-admin caller"
        );
    }

    fn result_with_meta_ui(ui: Value) -> CallToolResult {
        let mut meta = Map::new();
        meta.insert("ui".to_string(), ui);
        let mut result = CallToolResult::success(vec![Content::text("{}")]);
        result.meta = Some(Meta(meta));
        result
    }

    #[test]
    fn extract_ui_link_reads_meta_ui_resource_uri() {
        let result = result_with_meta_ui(json!({
            "resourceUri": "ui://axon/status-dashboard",
            "mimeTypes": ["text/html;profile=mcp-app"],
        }));
        let link = extract_ui_link(&result).expect("ui link present");
        // The whole `ui` object is captured verbatim for identical mirroring.
        assert_eq!(link.ui_meta["resourceUri"], "ui://axon/status-dashboard");
        assert_eq!(link.ui_meta["mimeTypes"][0], "text/html;profile=mcp-app");
    }

    #[test]
    fn extract_ui_link_none_without_meta_ui() {
        // No `_meta` at all.
        assert!(extract_ui_link(&CallToolResult::success(vec![Content::text("{}")])).is_none());
        // `_meta` present but no `ui` key.
        let mut meta = Map::new();
        meta.insert("other".to_string(), json!(1));
        let mut result = CallToolResult::success(vec![Content::text("{}")]);
        result.meta = Some(Meta(meta));
        assert!(extract_ui_link(&result).is_none());
    }

    fn response_with_result(result: Value) -> CodeModeExecutionResponse {
        CodeModeExecutionResponse {
            execution_id: None,
            result: Some(result),
            ui: None,
            calls: Vec::new(),
            logs: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn apply_ui_opt_in_unwraps_and_attaches_captured_link() {
        let broker = CodeModeBroker::new(None);
        *broker.ui_capture.lock().unwrap() = Some(UiLink {
            ui_meta: json!({ "resourceUri": "ui://axon/status-dashboard" }),
        });
        let mut response = response_with_result(json!({ "__ui": { "degraded": false } }));
        broker.apply_ui_opt_in(&mut response);
        // Inner payload is surfaced as `result`, wrapper removed.
        assert_eq!(response.result, Some(json!({ "degraded": false })));
        assert_eq!(
            response.ui.as_ref().expect("widget attached").ui_meta["resourceUri"],
            "ui://axon/status-dashboard"
        );
    }

    #[test]
    fn apply_ui_opt_in_without_optin_is_noop() {
        let broker = CodeModeBroker::new(None);
        let mut response = response_with_result(json!({ "degraded": false }));
        broker.apply_ui_opt_in(&mut response);
        assert_eq!(response.result, Some(json!({ "degraded": false })));
        assert!(
            response.ui.is_none(),
            "no captured widget → no widget attached"
        );
    }

    #[test]
    fn apply_ui_opt_in_surfaces_direct_ui_tool_result() {
        let broker = CodeModeBroker::new(None);
        *broker.ui_capture.lock().unwrap() = Some(UiLink {
            ui_meta: json!({ "resourceUri": "ui://ytdl-mcp/youtube-search.html" }),
        });

        let mut response = response_with_result(json!({
            "query": "phish",
            "limit": 1,
            "results": []
        }));

        broker.apply_ui_opt_in(&mut response);

        assert_eq!(
            response.ui.as_ref().expect("widget attached").ui_meta["resourceUri"],
            "ui://ytdl-mcp/youtube-search.html"
        );
    }
}
