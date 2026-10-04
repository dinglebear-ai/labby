use super::*;

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    pub(super) async fn build_code_mode_proxy(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<String, ToolError> {
        let Some(host) = self.host else {
            return Ok(if local_providers_allowed(caller, scope) {
                format!(
                    "{}\n{}",
                    crate::preamble::generate_local_provider_js(),
                    crate::sandbox::javascript()
                )
            } else {
                String::new()
            });
        };
        let (include_snippets, use_cache) = discovery_render_params(caller, surface, scope);
        let render_started = std::time::Instant::now();
        let render = host
            .list_tools(caller, surface, scope, include_snippets, use_cache)
            .await?;
        tracing::debug!(
            surface = "dispatch",
            service = "code_mode",
            action = "catalog.snapshot",
            elapsed_ms = render_started.elapsed().as_millis(),
            entry_count = render.entries.len(),
            "captured Code Mode discovery catalog snapshot"
        );
        *self.discovery_render.lock().map_err(|_| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode discovery snapshot lock is poisoned".to_string(),
        })? = Some(render.clone());
        let code_mode_config = host.config().await;
        let catalog = render
            .entries
            .iter()
            .filter(|entry| personal_catalog_entry_enabled(entry, scope, &code_mode_config.search))
            .cloned()
            .collect::<Vec<_>>();

        let discovery_entries = catalog
            .iter()
            .map(CodeModeDiscoveryEntry::from_catalog)
            .collect::<Vec<_>>();
        let blend_weight = code_mode_config.semantic_search.blend_weight;
        let discovery_js = crate::preamble::generate_discovery_js(
            &discovery_entries,
            blend_weight,
            &render.withheld,
        )
        .map_err(|message| ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message,
        })?;
        let tool_entries = catalog
            .iter()
            .filter(|entry| entry.kind == CodeModeCatalogKind::Tool)
            .collect::<Vec<_>>();
        let namespace_js =
            crate::preamble::generate_js_proxy_from_catalog(&tool_entries).map_err(|message| {
                ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message,
                }
            })?;
        let local_provider_js = if local_providers_allowed(caller, scope) {
            format!(
                "{}\n{}",
                crate::preamble::generate_local_provider_js(),
                crate::sandbox::javascript()
            )
        } else {
            String::new()
        };
        // The `openapi` shim is emitted ONLY on the host path (this fn) and ONLY
        // when the gate passes AND the host has ≥1 loaded spec. The host-less
        // early-return path has no registry, so the shim would only ever error
        // there (M11).
        let openapi_registry = host.openapi_registry();
        let openapi_provider_js = if openapi_provider_allowed(caller, scope, &openapi_registry)
            && !openapi_registry.is_empty()
        {
            crate::preamble::generate_openapi_provider_js()
        } else {
            ""
        };
        Ok(format!(
            "{local_provider_js}\n{openapi_provider_js}\n{discovery_js}\n{namespace_js}"
        ))
    }

    pub(super) async fn execute_sandboxed(
        &self,
        code: &str,
        timeout: Duration,
        snippet_max_bytes: usize,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        max_log_entries: usize,
        max_log_bytes: usize,
        trace_params: bool,
        scope: ToolScope,
        execution_id: Option<Arc<str>>,
        trace_context: Option<Arc<TraceContext>>,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        // Cloudflare-parity: no typed TypeScript preamble is injected. The
        // sandbox exposes only `callTool(id, params)`; the agent uses tool ids
        // discovered via `search`. Normalize the user code and run it directly.
        let code_to_run = normalize_user_code(code);

        // Build the runtime `codemode.*` proxy from the live catalog (same
        // source `search` uses) before starting the runner. Proxy failure is an
        // execution failure: otherwise `codemode.search`, `codemode.describe`,
        // and generated helpers silently disappear while raw `callTool` can
        // still make the run look successful. A host may degrade to a partial
        // catalog under its own, shorter cold-connect budget, but only loudly
        // (it logs which upstreams it omitted), and a pass that would leave the
        // catalog empty is an error rather than an empty proxy; this deadline
        // stays the fail-closed backstop.
        let deadline = tokio::time::Instant::now() + timeout;
        let proxy = match tokio::time::timeout_at(
            deadline,
            self.build_code_mode_proxy(&caller, surface, &scope),
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

        self.run_in_runner(
            code_to_run,
            proxy,
            remaining,
            caller,
            surface,
            max_log_entries,
            max_log_bytes,
            trace_params,
            scope,
            snippet_max_bytes,
            execution_id,
            trace_context,
        )
        .await
    }
}
