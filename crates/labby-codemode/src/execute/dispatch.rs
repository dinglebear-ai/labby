use super::*;

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    pub(crate) async fn call_tool_id_before_deadline(
        &self,
        id: &str,
        params: Value,
        deadline: tokio::time::Instant,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        ctx: ExecCtx,
        cancellation: &CancellationToken,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        tokio::select! {
            () = cancellation.cancelled() => Err(CodeModeCallError::new(
                "cancelled",
                "Code Mode execution was cancelled",
            )),
            result = tokio::time::timeout_at(
                deadline,
                self.call_tool_id_outcome(id, params, caller, surface, scope, ctx),
            ) => match result {
                Ok(result) => result,
                Err(_) => Err(CodeModeCallError::new(
                    "timeout",
                    "Code Mode execution timed out",
                )),
            },
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn call_tool_id(
        &self,
        id: &str,
        params: Value,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        ctx: ExecCtx,
    ) -> Result<Value, ToolError> {
        self.call_tool_id_outcome(id, params, caller, surface, scope, ctx)
            .await
            .map(|outcome| outcome.value)
            .map_err(CodeModeCallError::into_tool_error)
    }

    pub(crate) async fn call_tool_id_outcome(
        &self,
        id: &str,
        params: Value,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        ctx: ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let parsed = CodeModeToolId::parse(id).map_err(CodeModeCallError::from)?;
        let Some(host) = self.host else {
            return Err(
                CodeModeCallError::new("unknown_tool", "no tool source configured").with_tool(id),
            );
        };
        match parsed.reference {
            CodeModeToolRef::Tool { namespace, tool } => {
                if namespace == LAB_INTERNAL_NAMESPACE {
                    return self
                        .dispatch_internal_call(&tool, params, &caller, surface, scope)
                        .await
                        .map(|value| ToolCallOutcome { value, ui: None })
                        .map_err(CodeModeCallError::from);
                }
                let id = scoped_call_id(scope, &parsed.raw, &namespace, &tool)?;
                // The host applies destructive-tool policy when it resolves the
                // call (read-only callers cannot run a tool the host marks
                // destructive); it surfaces a `forbidden` error which passes
                // straight through.
                let outcome = host
                    .call_tool(&id, params, &caller, surface, scope, ctx)
                    .await?;
                if let Some(ui) = outcome.ui {
                    if let Ok(mut sink) = self.ui_capture.lock() {
                        *sink = Some(ui.clone());
                    } else {
                        tracing::warn!(
                            surface = "dispatch",
                            service = "code_mode",
                            action = "mcp_app.capture",
                            kind = "ui_capture_lock_poisoned",
                            "failed to store captured MCP App widget link"
                        );
                    }
                    return Ok(ToolCallOutcome {
                        value: outcome.value,
                        ui: Some(ui),
                    });
                }
                Ok(outcome)
            }
        }
    }

    /// Dispatch a reserved `__lab_internal::*` pseudo-tool call. These never
    /// reach `host.call_tool` and are never subject to `scope.allows()` —
    /// see the `LAB_INTERNAL_NAMESPACE` doc comment for why that's safe.
    pub(super) async fn dispatch_internal_call(
        &self,
        tool: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        let Some(host) = self.host else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "no tool source configured".to_string(),
            });
        };
        match tool {
            "read_artifact" | "artifact_info" | "list_artifacts" => {
                crate::artifact_access::dispatch(tool, &params, caller, scope).await
            }
            "list_resources" => {
                let upstream = params
                    .get("upstream")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(|| ToolError::MissingParam {
                        message: "list_resources requires a non-empty `upstream`".to_string(),
                        param: "upstream".to_string(),
                    })?;
                if upstream.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
                    return Err(ToolError::Sdk {
                        sdk_kind: "invalid_param".to_string(),
                        message: "resource upstream name is too long".to_string(),
                    });
                }
                host.list_resources(upstream.to_string(), caller, surface, scope)
                    .await
            }
            "read_resource" => {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .filter(|uri| !uri.trim().is_empty())
                    .ok_or_else(|| ToolError::MissingParam {
                        message: "read_resource requires a non-empty `uri`".to_string(),
                        param: "uri".to_string(),
                    })?;
                if uri.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
                    return Err(ToolError::Sdk {
                        sdk_kind: "invalid_param".to_string(),
                        message: format!(
                            "resource URI exceeds max length {MAX_CAPABILITY_IDENTIFIER_BYTES} bytes"
                        ),
                    });
                }
                host.read_resource(uri.to_string(), caller, surface, scope)
                    .await
            }
            "get_prompt" => {
                let prompt = params
                    .get("prompt")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| ToolError::MissingParam {
                        message: "get_prompt requires a non-empty `prompt`".to_string(),
                        param: "prompt".to_string(),
                    })?;
                if prompt.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
                    return Err(ToolError::InvalidParam {
                        message: format!(
                            "prompt identifier exceeds max length {MAX_CAPABILITY_IDENTIFIER_BYTES} bytes"
                        ),
                        param: "prompt".to_string(),
                    });
                }
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Default::default()));
                if !arguments.is_object() {
                    return Err(ToolError::InvalidParam {
                        message: "get_prompt `arguments` must be an object".to_string(),
                        param: "arguments".to_string(),
                    });
                }
                host.get_prompt(prompt.to_string(), arguments, caller, surface, scope)
                    .await
            }
            "list_skills" => host.list_skills(caller, surface, scope).await,
            "get_skill" => {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .filter(|uri| !uri.trim().is_empty())
                    .ok_or_else(|| ToolError::MissingParam {
                        message: "get_skill requires a non-empty `uri`".to_string(),
                        param: "uri".to_string(),
                    })?;
                if uri.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
                    return Err(ToolError::Sdk {
                        sdk_kind: "invalid_param".to_string(),
                        message: format!(
                            "Skill URI exceeds max length {MAX_CAPABILITY_IDENTIFIER_BYTES} bytes"
                        ),
                    });
                }
                host.get_skill(uri.to_string(), caller, surface, scope)
                    .await
            }
            "read_skill" => {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .filter(|uri| !uri.trim().is_empty())
                    .ok_or_else(|| ToolError::MissingParam {
                        message: "read_skill requires a non-empty `uri`".to_string(),
                        param: "uri".to_string(),
                    })?;
                if uri.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
                    return Err(ToolError::Sdk {
                        sdk_kind: "invalid_param".to_string(),
                        message: format!(
                            "Skill URI exceeds max length {MAX_CAPABILITY_IDENTIFIER_BYTES} bytes"
                        ),
                    });
                }
                host.read_skill(uri.to_string(), caller, surface, scope)
                    .await
            }
            "semantic_rank" => {
                let query = clamp_semantic_query(
                    params
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n.clamp(1, 50) as usize)
                    .unwrap_or(50);
                let kinds = params
                    .get("kinds")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .filter_map(CodeModeCatalogKind::parse_filter)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                // Fail-open at this layer too: even though the trait contract
                // says implementations return `Ok(Vec::new())` on degraded
                // paths (never `Err`), an accidental `Err` from a host bug
                // still must not break `codemode.search()` — degrade to an
                // empty ranked list, identical to the "no semantic signal"
                // case.
                let ranked = host
                    .semantic_rank(query, limit, &kinds, caller, surface, scope)
                    .await
                    .unwrap_or_default();
                let ranked_json: Vec<Value> = ranked
                    .into_iter()
                    .map(|(id, score)| serde_json::json!({ "id": id, "score": score }))
                    .collect();
                Ok(serde_json::json!({ "ranked": ranked_json }))
            }
            "artifact_search" => {
                let query = clamp_semantic_query(
                    params
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n.clamp(1, 50) as usize)
                    .unwrap_or(50);
                let kinds = params
                    .get("kinds")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .filter_map(CodeModeCatalogKind::parse_filter)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let search = host
                    .search_artifacts(
                        query,
                        limit.saturating_add(1),
                        &kinds,
                        caller,
                        surface,
                        scope,
                    )
                    .await?;
                let mut entries = search
                    .entries
                    .into_iter()
                    .filter(|entry| discovery_entry_visible(entry, scope))
                    .take(limit.saturating_add(1))
                    .map(|entry| CodeModeDiscoveryEntry::from_catalog(&entry))
                    .collect::<Vec<_>>();
                while serde_json::to_vec(&entries)
                    .map(|json| json.len() > crate::SEARCH_RESPONSE_MAX_BYTES)
                    .unwrap_or(true)
                {
                    if entries.pop().is_none() {
                        break;
                    }
                }
                Ok(
                    serde_json::json!({ "entries": entries, "incompleteSources": search.incomplete_sources }),
                )
            }
            "describe_types" => {
                let id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty());
                let Some(id) = id else {
                    return Err(ToolError::MissingParam {
                        message: "describe_types requires a non-empty `id`".to_string(),
                        param: "id".to_string(),
                    });
                };
                // Prefer the exact render captured while building this execution's
                // discovery proxy. This makes describe() a lookup over the same
                // authorized snapshot that resolved the target instead of a second
                // live catalog enumeration that can be slower, fail independently,
                // or observe a different tool set. Direct internal-call tests and
                // other paths that bypass proxy construction retain a live fallback,
                // but failures there are propagated so callers can surface an
                // explicit incomplete-schema state.
                let lookup_started = std::time::Instant::now();
                let snapshot = self
                    .discovery_render
                    .lock()
                    .map_err(|_| ToolError::Sdk {
                        sdk_kind: "internal_error".to_string(),
                        message: "Code Mode discovery snapshot lock is poisoned".to_string(),
                    })?
                    .clone();
                let (render, catalog_source) = if let Some(render) = snapshot {
                    (render, "execution_snapshot")
                } else {
                    let (include_snippets, use_cache) =
                        discovery_render_params(caller, surface, scope);
                    (
                        host.list_tools(caller, surface, scope, include_snippets, use_cache)
                            .await?,
                        "live_fallback",
                    )
                };
                // `list_tools` only filters on the caller's allowed namespaces;
                // the finer-grained per-tool grant (`scope.allows`) is applied by
                // `discovery_entry_visible` at the point the sandbox's own
                // discovery index is built (`build_code_mode_proxy`). Re-apply it
                // here too — otherwise a tool-scoped caller could fetch type info
                // for a sibling tool in the same namespace it cannot call, since
                // this reserved-namespace call is reachable directly from sandbox
                // JS via `callTool(id, params)`, not only through
                // `codemode.describe()`'s own already-scoped matching.
                let dts = render
                    .entries
                    .iter()
                    .find(|entry| entry.id == id && discovery_entry_visible(entry, scope))
                    .map(|entry| entry.dts.clone())
                    .filter(|dts| !dts.is_empty());
                tracing::debug!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "describe_types",
                    catalog_source,
                    elapsed_ms = lookup_started.elapsed().as_millis(),
                    entry_count = render.entries.len(),
                    schema_found = dts.is_some(),
                    "resolved Code Mode tool parameter declaration"
                );
                Ok(serde_json::json!({ "dts": dts }))
            }
            _ => Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown internal tool `{LAB_INTERNAL_NAMESPACE}::{tool}`"),
            }),
        }
    }
}
