//! `impl CodeModeHost for GatewayManager`: the gateway's binding of the
//! extracted Code Mode kernel to its upstream MCP proxy pool.
//!
//! This is where gateway/upstream vocabulary is legitimately reintroduced: the
//! crate's neutral `CodeModeHost` methods are implemented in terms of the live
//! `UpstreamPool`, `UpstreamTool`, `UpstreamRuntimeOwner`, OAuth subjects, and
//! the snippet store. The crate never sees any of it.

use labby_codemode::snippet::store::{
    builtin_snippet_dir, code_for_snippet, merge_snippet_input, resolve_snippet,
};
use labby_codemode::{
    CodeModeCallError, CodeModeCaller, CodeModeConfig, CodeModeErrorOrigin, CodeModeHost,
    CodeModeSideEffectRisk, CodeModeSurface, CodeModeToolSafetyHints, ResolvedSnippet, RunnerPool,
    ToolCallOutcome, ToolScope, ToolsRender, UiLink, destructive_permitted,
    discovery_entry_visible, discovery_render_params,
};
use std::sync::Arc;

use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, ErrorData};
use serde_json::{Map, Value};

use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::gateway::manager::GatewayManager;
use crate::upstream::pool::CapabilityCallError;
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};
use labby_runtime::error::ToolError;
use labby_runtime::lab_home;

use super::search;
use super::tool_error::{completed_tool_error, upstream_tool_safety};
use super::validate_code_mode_params_against_schema;

impl CodeModeHost for GatewayManager {
    async fn list_tools(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        include_snippets: bool,
        use_cache: bool,
    ) -> Result<ToolsRender, ToolError> {
        // MCP `codemode` execution must not spend the caller's wall-clock budget
        // cold-connecting every upstream just to render helper metadata; trivial
        // code that never calls a tool should reach the runner immediately.
        // Tool execution remains live because `call_tool` resolves the requested
        // upstream at the actual call boundary.
        let allow_cold_connect = surface == CodeModeSurface::Cli && caller.can_execute();
        let owner = runtime_owner(caller, surface);
        let oauth_subject = oauth_subject(caller);
        let allowed = scope.allowed_namespaces();
        search::build_tools_render(
            self,
            allow_cold_connect,
            &owner,
            oauth_subject,
            allowed,
            include_snippets,
            use_cache,
        )
        .await
    }

    async fn call_tool(
        &self,
        id: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        _scope: &ToolScope,
        _ctx: labby_codemode::ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let (upstream, tool) =
            labby_codemode::split_namespaced_id(id).ok_or_else(|| ToolError::Sdk {
                sdk_kind: "invalid_code_mode_id".to_string(),
                message: format!("Code Mode ids must use <namespace>::<tool>: `{id}`"),
            })?;
        let owner = runtime_owner(caller, surface);
        let oauth_subject = oauth_subject(caller);

        let upstream_tool = self
            .resolve_code_mode_upstream_tool(upstream, tool, Some(&owner), oauth_subject)
            .await?;

        // A destructive tool the caller is not otherwise permitted to run is
        // hard-`forbidden`. Code Mode execution is already scope-gated; there is
        // no pause/confirm dance on top of it — `destructive_permitted` is the
        // only gate.
        let requires_approval =
            upstream_tool.destructive && !destructive_permitted(surface, caller);
        if requires_approval {
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
            }
            .into());
        }
        validate_code_mode_params_against_schema(&params, upstream_tool.input_schema.as_ref())?;
        let tool_ui = extract_tool_ui_link(&upstream_tool);
        let safety = upstream_tool_safety(&upstream_tool);
        let mut outcome = self
            .execute_upstream_tool_with_contract(upstream, tool, params, safety)
            .await?;
        if outcome.ui.is_none()
            && let Some(ui) = tool_ui
        {
            let resource_uri = ui_resource_uri(&ui.ui_meta).unwrap_or("<unknown>");
            tracing::info!(
                surface = "dispatch",
                service = "code_mode",
                action = "mcp_app.capture",
                upstream,
                tool,
                resource_uri,
                "captured upstream MCP App widget link from tool metadata"
            );
            outcome.ui = Some(ui);
        }
        Ok(outcome)
    }

    /// Buffer one `codemode.step` boundary for the run's `execution_id`.
    ///
    /// FAIL-OPEN + write-free on the runner drive loop: this only pushes a row
    /// into an in-memory per-execution buffer (nanoseconds, no SQLite I/O). The
    /// single bulk flush happens at the run boundary via `flush_step_journal`.
    /// A `None` `execution_id`/`step_ordinal` or unconfigured journal short-
    /// circuits to `Ok(())`. This method can never fail the run — the buffer
    /// push is infallible barring a poisoned mutex.
    async fn record_step(
        &self,
        ctx: labby_codemode::ExecCtx,
        name: &str,
        value: &Value,
    ) -> Result<(), ToolError> {
        let (Some(execution_id), Some(ordinal), Some(_store)) = (
            ctx.execution_id.as_ref(),
            ctx.step_ordinal,
            self.step_journal.as_ref(),
        ) else {
            return Ok(());
        };
        let row = crate::codemode_journal::StepJournalRow {
            execution_id: execution_id.to_string(),
            step_ordinal: ordinal,
            seq_base: ctx.seq,
            // Redact BOTH name (caller-authored JS) and value at rest. `name` is
            // a short label, so cap it on a char boundary BEFORE redacting so a
            // caller can't write a multi-MB step name into the durable DB (the
            // value path is bounded by `redact_journal_text`'s BoundedWriter).
            name: labby_runtime::agent_error::redact_secret_like_segments(cap_on_char_boundary(
                name,
                JOURNAL_NAME_CAP_BYTES,
            )),
            value: crate::codemode_journal::redact_journal_text(value, JOURNAL_VALUE_CAP_BYTES),
            ok: true,
            // Per-step elapsed isn't threaded in v1; owner identity is stamped
            // at flush from the run context.
            elapsed_ms: 0,
            recorded_at: unix_now(),
            actor_key: None,
            route_scope: String::new(),
            capability_filter_fingerprint: None,
            replayed_from: None,
        };
        self.step_buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(execution_id.to_string())
            .or_default()
            .push(row);
        // name/value are deliberately NOT logged: both are redacted at rest and
        // the identifiers below are sufficient to trace journaling.
        tracing::debug!(
            surface = "dispatch",
            service = labby_codemode::SERVICE,
            action = "step_journal.record",
            execution_id = %execution_id,
            step_ordinal = ordinal,
            "codemode.step journaled"
        );
        Ok(())
    }

    async fn resolve_snippet(
        &self,
        name: &str,
        input: Value,
    ) -> Result<ResolvedSnippet, ToolError> {
        let lab_home = lab_home();
        let builtin_dir = builtin_snippet_dir();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let resolved = resolve_snippet(&lab_home, &builtin_dir, &name)?;
            let input = merge_snippet_input(&resolved, input)?;
            let code = code_for_snippet(&resolved)?;
            Ok::<_, ToolError>(ResolvedSnippet {
                name: resolved.name,
                code,
                input,
            })
        })
        .await
        .map_err(|err| ToolError::internal_message(format!("snippet resolve task failed: {err}")))?
    }

    async fn semantic_rank(
        &self,
        query: String,
        top_k: usize,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        let config = self.code_mode_config().await.semantic_search;
        if !config.is_configured() || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // Recompute the SAME scope-filtered entries `list_tools` +
        // `build_code_mode_proxy` would produce for this exact
        // caller/surface/scope — this is what makes the design race-free: no
        // shared "current fingerprint" state is read here, only this call's
        // own arguments. `include_snippets`/`use_cache` come from
        // labby-codemode's `discovery_render_params` — the SAME function
        // `build_code_mode_proxy` calls — so the fingerprint computed here
        // structurally cannot diverge from the one the warming path in
        // `catalog_from_tools` already embedded for this execution's
        // catalog. `allow_cold_connect` is hardcoded `false`
        // (unlike `list_tools`'s `caller.can_execute()`): semantic ranking
        // must never spend wall-clock cold-connecting upstreams — by the
        // time a sandbox calls search(), the proxy build already connected
        // everything this execution can see.
        let (include_snippets, use_cache) = discovery_render_params(caller, surface, scope);
        let owner = runtime_owner(caller, surface);
        let oauth_subject = oauth_subject(caller);
        let allowed = scope.allowed_namespaces();
        let render = match search::build_tools_render(
            self,
            false,
            &owner,
            oauth_subject,
            allowed,
            include_snippets,
            use_cache,
        )
        .await
        {
            Ok(render) => render,
            // Fail-open: a catalog build failure must not break search().
            Err(_) => return Ok(Vec::new()),
        };
        if !self.semantic_search_available().await {
            return Ok(Vec::new());
        }
        // Embeddings are cached/warmed over the FULL render (same
        // fingerprint + entry set as `catalog_from_tools`' warming path);
        // ranking is then restricted to exactly the entry subset the
        // sandbox's own `__codemodeDiscovery` contains for this scope —
        // labby-codemode's `discovery_entry_visible`, the SAME function
        // `build_code_mode_proxy` filters with. This is the security
        // invariant: `rank_by_similarity` is only ever given scope-allowed
        // ids, so it is structurally impossible to return an id the sandbox
        // cannot see.
        let vectors = self
            .ensure_embeddings_for_fingerprint(&render.fingerprint, &render.entries)
            .await;
        if vectors.is_empty() {
            return Ok(Vec::new());
        }
        let allowed_ids: std::collections::BTreeSet<&str> = render
            .entries
            .iter()
            .filter(|entry| discovery_entry_visible(entry, scope))
            .map(|entry| entry.id.as_str())
            .collect();
        let scoped_vectors: Vec<(String, Vec<f32>)> = vectors
            .into_iter()
            .filter(|(id, _)| allowed_ids.contains(id.as_str()))
            .collect();
        if scoped_vectors.is_empty() {
            return Ok(Vec::new());
        }
        let query_vec = match super::embeddings::embed_via_tei(
            config
                .tei_url
                .as_deref()
                .expect("is_configured() guarantees Some"),
            &[query],
        )
        .await
        {
            Ok(mut v) if !v.is_empty() => v.remove(0),
            Ok(_) => return Ok(Vec::new()),
            Err(err) => {
                self.record_semantic_search_failure(&err.to_string()).await;
                return Ok(Vec::new());
            }
        };
        self.record_semantic_search_recovery().await;
        Ok(super::embeddings::rank_top_k_by_similarity(
            &query_vec,
            &scoped_vectors,
            top_k,
        ))
    }

    async fn config(&self) -> CodeModeConfig {
        self.code_mode_config().await
    }

    fn runner_pool(&self) -> &RunnerPool {
        self.code_mode_runner_pool()
    }

    fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
        self.openapi_registry.clone()
    }

    fn openapi_http_client(&self) -> reqwest::Client {
        self.openapi_http_client.clone()
    }
}

/// Per-run caller identity stamped onto journal rows at flush time (captured
/// once at the run boundary rather than per `record_step`). Persisted for the
/// v2 replay-auth path (epic lab-5dtw9); v1 never reads it back.
#[derive(Debug, Clone, Default)]
pub struct JournalOwner {
    pub actor_key: Option<String>,
    pub route_scope: String,
    pub capability_filter_fingerprint: Option<String>,
}

/// Byte cap for a journaled step value's serialized JSON (mirrors the history
/// byte-cap spirit). Oversize values become a small truncation sentinel.
const JOURNAL_VALUE_CAP_BYTES: usize = 64 * 1024;

/// Byte cap for a journaled step `name`. A step name is a short label, so this
/// bounds a hostile caller's per-row name growth at rest.
const JOURNAL_NAME_CAP_BYTES: usize = 4096;

/// Truncate `s` to at most `cap` bytes on a UTF-8 char boundary.
fn cap_on_char_boundary(s: &str, cap: usize) -> &str {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Current wall-clock time as unix seconds (0 on a pre-epoch clock).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Gateway-side Code Mode dispatch helpers (not trait methods).
impl GatewayManager {
    /// Drain the `execution_id` step buffer and persist it in ONE bulk insert
    /// at the run boundary.
    ///
    /// FAIL-OPEN: journaling is orthogonal to dispatch. A flush failure logs a
    /// warning and returns — a lost journal only costs future replay
    /// completeness, never the run's success. The buffer is drained
    /// unconditionally (even on flush error) so a failed run can't leak buffered
    /// rows across executions.
    pub async fn flush_step_journal(&self, execution_id: &str, owner: &JournalOwner) {
        let Some(store) = self.step_journal.as_ref() else {
            return;
        };
        let mut rows = {
            let mut buffers = self
                .step_buffers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            buffers.remove(execution_id).unwrap_or_default()
        };
        if rows.is_empty() {
            return;
        }
        for r in &mut rows {
            r.actor_key = owner.actor_key.clone();
            r.route_scope = owner.route_scope.clone();
            r.capability_filter_fingerprint = owner.capability_filter_fingerprint.clone();
        }
        let row_count = rows.len();
        if let Err(err) = store.flush(rows).await {
            // `err.kind()` is always the generic `journal_store_error`; log the
            // full `err` Display for the real cause (disk full / no such table /
            // locked). rusqlite's Display references SQL text and constraints,
            // never bound parameter values, so this leaks no journaled content.
            tracing::warn!(
                surface = "dispatch",
                service = labby_codemode::SERVICE,
                action = "step_journal.flush",
                execution_id,
                rows = row_count,
                error = %err,
                "step journal flush failed (fail-open)"
            );
        }
    }

    /// Drop-safe cancellation cleanup for a run that never reached its async
    /// journal flush boundary. This is synchronous by design so an execution
    /// future's `Drop` can remove buffered request state immediately.
    pub fn discard_step_buffer(&self, execution_id: &str) {
        self.step_buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(execution_id);
    }

    /// Read-only accessor to the step journal store (used by tests and future
    /// read surfaces). `None` when journaling is unconfigured.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn step_journal(&self) -> Option<&Arc<crate::codemode_journal::StepJournalStore>> {
        self.step_journal.as_ref()
    }

    /// True when no execution has any buffered (un-flushed) journal rows.
    #[cfg(test)]
    pub(crate) fn step_buffer_is_empty(&self) -> bool {
        self.step_buffers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .all(Vec::is_empty)
    }

    /// Dispatch a resolved Code Mode call to the upstream MCP pool and unwrap
    /// the result. Shared by the durable and write-free `call_tool` paths
    /// (mcp-ui capture, error classification, success/failure recording).
    pub(crate) async fn execute_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        params: Value,
    ) -> Result<ToolCallOutcome, ToolError> {
        self.execute_upstream_tool_with_contract(
            upstream,
            tool,
            params,
            CodeModeToolSafetyHints::default(),
        )
        .await
        .map_err(CodeModeCallError::into_tool_error)
    }

    async fn execute_upstream_tool_with_contract(
        &self,
        upstream: &str,
        tool: &str,
        params: Value,
        safety: CodeModeToolSafetyHints,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let id = format!("{upstream}::{tool}");
        let arguments =
            upstream_arguments(upstream, tool, params).map_err(CodeModeCallError::from)?;
        let Some(pool) = self.current_pool().await else {
            return Err(CodeModeCallError::new(
                "upstream_error",
                "gateway upstream pool is unavailable",
            )
            .with_tool(id)
            .with_origin(CodeModeErrorOrigin::UpstreamTransport)
            .with_side_effects(CodeModeSideEffectRisk::NoneExpected));
        };
        let mut upstream_params = CallToolRequestParams::new(tool.to_string());
        upstream_params.arguments = Some(arguments);
        match pool.call_tool_classified(upstream, upstream_params).await {
            Some(Ok(result)) => {
                // `is_error=true` is an MCP tool-level failure carried inside
                // a successful protocol response. Reaching this branch proves
                // the upstream connection is healthy regardless of the tool's
                // outcome or payload representation.
                pool.record_success(upstream).await;
                if result.is_error == Some(true) {
                    return Err(completed_tool_error(&id, &result, safety));
                }
                let ui = extract_ui_link(&result);
                if let Some(ui) = ui.as_ref() {
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
                }
                Ok(ToolCallOutcome {
                    value: unwrap_code_mode_upstream_result(result),
                    ui,
                })
            }
            Some(Err(err)) => {
                // The pool owns transport-vs-MCP health accounting. A JSON-RPC
                // `ErrorData` is an application-level rejection carried over a
                // healthy connection — preserve its classified kind and
                // redacted payload so caller mistakes do not masquerade as
                // broken upstream infrastructure. Everything else is a real
                // transport-class failure, distinct from
                // `CallToolResult(isError=true)` above.
                let (kind, message) = code_mode_capability_error_info(&err);
                Err(match err {
                    CapabilityCallError::Mcp { .. } => CodeModeCallError::new(
                        kind,
                        labby_runtime::agent_error::sanitize_error_text(
                            &message,
                            MAX_UPSTREAM_MCP_MESSAGE_CHARS,
                        ),
                    )
                    .with_tool(id),
                    _ => {
                        CodeModeCallError::upstream_transport_classified(id, kind, message, safety)
                    }
                })
            }
            None => {
                pool.record_failure(upstream, format!("upstream `{upstream}` is not connected"))
                    .await;
                Err(CodeModeCallError::new(
                    "not_found",
                    format!("upstream tool `{upstream}::{tool}` was not found"),
                )
                .with_tool(id))
            }
        }
    }
}

/// Map a Code Mode caller + surface onto an `UpstreamRuntimeOwner`. Lifted out
/// of the (now neutral) `CodeModeCaller` so the kernel carries no gateway type.
fn runtime_owner(caller: &CodeModeCaller, surface: CodeModeSurface) -> UpstreamRuntimeOwner {
    let surface = surface.tag();
    let subject = caller.subject().map(ToOwned::to_owned);
    let raw = subject
        .as_ref()
        .map(|subject| format!("{surface}:{subject}"))
        .unwrap_or_else(|| format!("{surface}:trusted-local"));
    UpstreamRuntimeOwner {
        surface: surface.to_string(),
        subject,
        request_id: None,
        session_id: None,
        client_name: None,
        raw: Some(raw),
    }
}

/// The upstream OAuth subject for a Code Mode caller.
///
/// Admin/operator callers share the single gateway-owned upstream credential
/// (`SHARED_GATEWAY_OAUTH_SUBJECT`); non-admin callers keep their own `sub` so a
/// personal upstream grant is used; a `sub`-less caller falls back to the shared
/// subject. Mirrors `oauth_upstream_subject_for_request`.
fn oauth_subject(caller: &CodeModeCaller) -> Option<&str> {
    if caller.is_admin() {
        return Some(SHARED_GATEWAY_OAUTH_SUBJECT);
    }
    Some(caller.subject().unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT))
}

fn extract_ui_link(result: &CallToolResult) -> Option<UiLink> {
    let meta = result.meta.as_ref()?;
    let ui = meta.get("ui")?;
    ui.get("resourceUri")?.as_str()?;
    Some(UiLink {
        ui_meta: ui.clone(),
    })
}

fn extract_tool_ui_link(tool: &UpstreamTool) -> Option<UiLink> {
    let meta = tool.tool.meta.as_ref()?;
    let ui = meta.0.get("ui")?;
    ui.get("resourceUri")?.as_str()?;
    Some(UiLink {
        ui_meta: ui.clone(),
    })
}

fn ui_resource_uri(ui_meta: &Value) -> Option<&str> {
    ui_meta.get("resourceUri").and_then(Value::as_str)
}

/// Unwrap an upstream `CallToolResult` into the value Code Mode returns.
fn unwrap_code_mode_upstream_result(result: CallToolResult) -> Value {
    if let Some(value) = result.structured_content {
        return value;
    }
    let all_text = !result.content.is_empty()
        && result
            .content
            .iter()
            .all(|content| content.as_text().is_some());
    if all_text {
        let text = result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(|content| content.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text));
    }
    if result.content.is_empty() {
        Value::Null
    } else {
        serde_json::json!(result)
    }
}

fn upstream_arguments(
    upstream: &str,
    tool: &str,
    params: Value,
) -> Result<Map<String, Value>, ToolError> {
    match params {
        Value::Object(arguments) => Ok(arguments),
        _ => Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("Code Mode tool `{upstream}::{tool}` params must be an object"),
        }),
    }
}

/// Byte cap for the redacted, serialized upstream `ErrorData.data` payload
/// appended to a Code Mode error message.
const UPSTREAM_ERROR_DATA_CAP_BYTES: usize = 2048;

/// Char cap for the upstream-authored JSON-RPC error message embedded in a
/// Code Mode rejection (the payload flows into the runner sandbox and the
/// outer MCP envelope).
const MAX_UPSTREAM_MCP_MESSAGE_CHARS: usize = 4096;

fn code_mode_capability_error_info(error: &CapabilityCallError) -> (&'static str, String) {
    match error {
        CapabilityCallError::Mcp { data, .. } => {
            let kind = code_mode_mcp_error_kind(data);
            let mut message = data.message.to_string();
            // Preserve the structured payload (param/valid/hint/retry_after_ms/
            // required_scopes/…) the old stringified Display used to carry.
            // Redacted (secret keys + secret-shaped strings) and bounded.
            if let Some(payload) = data.data.as_ref() {
                let redacted =
                    labby_codemode::redact_trace_value(payload, UPSTREAM_ERROR_DATA_CAP_BYTES);
                if let Ok(serialized) = serde_json::to_string(&redacted) {
                    let serialized =
                        labby_runtime::agent_error::redact_secret_like_segments(&serialized);
                    message = format!("{message} (upstream error data: {serialized})");
                }
            }
            // When the kind collapses to the generic `upstream_error`, keep the
            // numeric JSON-RPC code visible so the original class survives.
            if kind == "upstream_error" {
                message = format!("{message} (JSON-RPC code {})", data.code.0);
            }
            (kind, message)
        }
        CapabilityCallError::Timeout { message } => ("timeout", message.clone()),
        // Local gateway concurrency gate, not an upstream rate limit — matches
        // the `queue_saturated` outcome the pool already logs/records.
        CapabilityCallError::QueueSaturated { message } => ("queue_saturated", message.clone()),
        CapabilityCallError::ResponseTooLarge { message } => {
            ("response_too_large", message.clone())
        }
        CapabilityCallError::Transport { message } => ("network_error", message.clone()),
        CapabilityCallError::Protocol { message } => ("decode_error", message.clone()),
        CapabilityCallError::Cancelled { message } => ("cancelled", message.clone()),
        CapabilityCallError::InputRequiredRoundsExceeded { message } => {
            ("confirmation_required", message.clone())
        }
        CapabilityCallError::Other { message } => ("upstream_error", message.clone()),
    }
}

/// The shared allowlist of stable caller-facing error kinds an upstream may
/// assert about its own tool dispatch via JSON-RPC `ErrorData.data.kind`
/// ([`code_mode_mcp_error_kind`]); anything not listed falls back to the
/// `ErrorCode`-derived classification instead of passing verbatim. The
/// tool-level `isError` payload path applies the equivalent clamp in
/// `crate::upstream::tool_error::canonicalize_untrusted_upstream_kind`.
fn known_stable_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "unknown_action" => "unknown_action",
        "unknown_subaction" => "unknown_subaction",
        "missing_param" => "missing_param",
        "invalid_param" | "invalid_params" => "invalid_param",
        "unknown_instance" => "unknown_instance",
        "confirmation_required" => "confirmation_required",
        "conflict" => "conflict",
        "forbidden" => "forbidden",
        "unknown_tool" => "unknown_tool",
        "route_scope_denied" => "route_scope_denied",
        "path_traversal" => "path_traversal",
        "permission_denied" => "permission_denied",
        "auth_failed" => "auth_failed",
        "not_found" => "not_found",
        "rate_limited" => "rate_limited",
        "validation_failed" => "validation_failed",
        _ => return None,
    })
}

fn code_mode_mcp_error_kind(error: &ErrorData) -> &'static str {
    // An upstream-supplied `data.kind` is only trusted when it is in the shared
    // allowlist; unknown or invented kinds fall through to the ErrorCode-derived
    // classification below instead of passing verbatim.
    if let Some(kind) = error
        .data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|data| data.get("kind"))
        .and_then(Value::as_str)
        .and_then(known_stable_kind)
    {
        return kind;
    }

    if error.code == ErrorCode::INVALID_PARAMS {
        "invalid_param"
    } else if error.code == ErrorCode::METHOD_NOT_FOUND {
        "unknown_tool"
    } else if error.code == ErrorCode::RESOURCE_NOT_FOUND {
        "not_found"
    } else if error.code == ErrorCode::PARSE_ERROR {
        "decode_error"
    } else if error.code == ErrorCode::INTERNAL_ERROR {
        "server_error"
    } else if error.code == ErrorCode::UNSUPPORTED_PROTOCOL_VERSION
        || error.code == ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY
        || error.code == ErrorCode::HEADER_MISMATCH
    {
        "validation_failed"
    } else {
        "upstream_error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::runtime::GatewayRuntimeHandle;
    use labby_codemode::ExecCtx;
    use rmcp::model::MetaObject;

    /// Build a `GatewayManager` wired to a fresh temp `StepJournalStore`. The
    /// tempdir is intentionally leaked so the DB file outlives the store's open
    /// connections for the test's duration.
    async fn manager_with_store(
        store: crate::codemode_journal::StepJournalStore,
    ) -> (GatewayManager, tempfile::TempDir) {
        let cfg_dir = tempfile::tempdir().unwrap();
        let manager = GatewayManager::new(
            cfg_dir.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        )
        .with_step_journal(Arc::new(store));
        (manager, cfg_dir)
    }

    async fn test_manager_with_journal() -> (GatewayManager, tempfile::TempDir, tempfile::TempDir) {
        let db_dir = tempfile::tempdir().unwrap();
        let store =
            crate::codemode_journal::StepJournalStore::open(db_dir.path().join("journal.db"))
                .await
                .unwrap();
        let (manager, cfg_dir) = manager_with_store(store).await;
        (manager, cfg_dir, db_dir)
    }

    async fn test_manager_with_failing_journal()
    -> (GatewayManager, tempfile::TempDir, tempfile::TempDir) {
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("journal.db");
        let store = crate::codemode_journal::StepJournalStore::open(db_path.clone())
            .await
            .unwrap();
        // Drop the table out from under the store via a side connection so a
        // subsequent flush INSERT fails deterministically.
        {
            let side = rusqlite::Connection::open(&db_path).unwrap();
            side.execute_batch("DROP TABLE step_journal").unwrap();
        }
        let (manager, cfg_dir) = manager_with_store(store).await;
        (manager, cfg_dir, db_dir)
    }

    #[test]
    fn extract_tool_ui_link_preserves_tool_metadata_resource() {
        let mut tool = rmcp::model::Tool::new(
            "open_quick_shell".to_string(),
            "Open quick shell",
            Arc::new(Map::new()),
        );
        tool.meta = Some(MetaObject(Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({
                "resourceUri": "ui://quick-shell/component.html",
                "preferredSize": { "height": 520 }
            }),
        )])));
        let upstream_tool = UpstreamTool {
            tool,
            input_schema: None,
            output_schema: None,
            upstream_name: Arc::from("quick-shell"),
            destructive: false,
        };

        let ui = extract_tool_ui_link(&upstream_tool).expect("tool UI metadata");

        assert_eq!(
            ui.ui_meta["resourceUri"],
            serde_json::json!("ui://quick-shell/component.html")
        );
        assert_eq!(
            ui.ui_meta["preferredSize"]["height"],
            serde_json::json!(520)
        );
    }

    #[tokio::test]
    async fn record_step_buffers_then_flush_persists() {
        let (mgr, _cfg, _db) = test_manager_with_journal().await;
        let exec = Arc::<str>::from("exec_t1");
        let ctx = ExecCtx {
            seq: 3,
            execution_id: Some(exec.clone()),
            step_ordinal: Some(0),
        };
        mgr.record_step(ctx, "fetch", &serde_json::json!({"id": 7}))
            .await
            .unwrap();
        // Buffered only — nothing on disk yet (proves no I/O on the record path).
        assert!(
            mgr.step_journal()
                .unwrap()
                .load("exec_t1")
                .await
                .unwrap()
                .is_empty()
        );
        // Flush at the run boundary stamps owner identity and persists.
        mgr.flush_step_journal(
            "exec_t1",
            &JournalOwner {
                actor_key: Some("a".into()),
                route_scope: "default".into(),
                capability_filter_fingerprint: None,
            },
        )
        .await;
        let rows = mgr.step_journal().unwrap().load("exec_t1").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "fetch");
        assert_eq!(rows[0].step_ordinal, 0);
        assert_eq!(rows[0].seq_base, 3);
        assert_eq!(rows[0].actor_key.as_deref(), Some("a"));
        assert_eq!(rows[0].route_scope, "default");
        assert!(mgr.step_buffer_is_empty());
    }

    #[tokio::test]
    async fn record_step_none_execution_id_is_noop() {
        let (mgr, _cfg, _db) = test_manager_with_journal().await;
        let ctx = ExecCtx {
            seq: 1,
            execution_id: None,
            step_ordinal: Some(0),
        };
        mgr.record_step(ctx, "x", &serde_json::json!(1))
            .await
            .unwrap();
        assert!(mgr.step_buffer_is_empty());
    }

    #[tokio::test]
    async fn record_step_redacts_secret_name_and_value() {
        let (mgr, _cfg, _db) = test_manager_with_journal().await;
        let ctx = ExecCtx {
            seq: 1,
            execution_id: Some(Arc::<str>::from("exec_secret")),
            step_ordinal: Some(0),
        };
        mgr.record_step(
            ctx,
            "token sk-abcdefghij0123456789extra",
            &serde_json::json!({"authorization": "Bearer sk-abcdefghij0123456789extra"}),
        )
        .await
        .unwrap();
        mgr.flush_step_journal("exec_secret", &JournalOwner::default())
            .await;
        let rows = mgr
            .step_journal()
            .unwrap()
            .load("exec_secret")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            !rows[0].name.contains("sk-abcdefghij0123456789extra"),
            "step name must be redacted at rest: {}",
            rows[0].name
        );
        assert!(
            !rows[0].value.contains("sk-abcdefghij0123456789extra"),
            "step value must be redacted at rest: {}",
            rows[0].value
        );
    }

    #[tokio::test]
    async fn record_step_caps_oversized_name() {
        let (mgr, _cfg, _db) = test_manager_with_journal().await;
        // An all-ASCII name that is not secret-shaped, so redaction leaves it
        // intact and only the byte cap can shorten it.
        let huge = "n".repeat(100_000);
        let ctx = ExecCtx {
            seq: 1,
            execution_id: Some(Arc::<str>::from("exec_cap")),
            step_ordinal: Some(0),
        };
        mgr.record_step(ctx, &huge, &serde_json::json!(1))
            .await
            .unwrap();
        mgr.flush_step_journal("exec_cap", &JournalOwner::default())
            .await;
        let rows = mgr.step_journal().unwrap().load("exec_cap").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].name.len() <= JOURNAL_NAME_CAP_BYTES,
            "step name must be capped at rest, got {} bytes",
            rows[0].name.len()
        );
    }

    #[tokio::test]
    async fn flush_without_journal_configured_is_noop() {
        let cfg_dir = tempfile::tempdir().unwrap();
        let mgr = GatewayManager::new(
            cfg_dir.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        );
        // No journal store configured: record_step + flush are pure no-ops.
        let ctx = ExecCtx {
            seq: 1,
            execution_id: Some(Arc::<str>::from("e")),
            step_ordinal: Some(0),
        };
        mgr.record_step(ctx, "s", &serde_json::json!(1))
            .await
            .unwrap();
        assert!(mgr.step_journal().is_none());
        mgr.flush_step_journal("e", &JournalOwner::default()).await;
        assert!(mgr.step_buffer_is_empty());
    }

    #[tokio::test]
    async fn flush_failure_is_fail_open() {
        let (mgr, _cfg, _db) = test_manager_with_failing_journal().await;
        let ctx = ExecCtx {
            seq: 1,
            execution_id: Some(Arc::<str>::from("e")),
            step_ordinal: Some(0),
        };
        mgr.record_step(ctx, "s", &serde_json::json!(1))
            .await
            .unwrap();
        // Must not panic/propagate, and must drain the buffer even on error.
        mgr.flush_step_journal("e", &JournalOwner::default()).await;
        assert!(mgr.step_buffer_is_empty());
    }

    #[tokio::test]
    async fn cancelled_execution_can_drop_buffer_without_async_cleanup() {
        let (mgr, _cfg, _db) = test_manager_with_journal().await;
        mgr.record_step(
            ExecCtx {
                seq: 1,
                execution_id: Some(Arc::<str>::from("exec_cancelled")),
                step_ordinal: Some(0),
            },
            "first",
            &serde_json::json!({"value": 1}),
        )
        .await
        .unwrap();
        assert!(!mgr.step_buffer_is_empty());
        mgr.discard_step_buffer("exec_cancelled");
        assert!(mgr.step_buffer_is_empty());
    }

    #[test]
    fn maps_standard_mcp_error_codes_to_code_mode_kinds() {
        let cases = [
            (
                ErrorData::invalid_params("bad params", None),
                "invalid_param",
            ),
            (
                ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "missing method", None),
                "unknown_tool",
            ),
            (
                ErrorData::internal_error("server failed", None),
                "server_error",
            ),
            (ErrorData::parse_error("invalid JSON", None), "decode_error"),
        ];

        for (error, expected) in cases {
            assert_eq!(code_mode_mcp_error_kind(&error), expected);
        }
    }

    #[test]
    fn structured_mcp_error_kind_takes_precedence_over_generic_code() {
        let error = ErrorData::invalid_request(
            "scope denied",
            Some(serde_json::json!({"kind": "forbidden"})),
        );

        assert_eq!(code_mode_mcp_error_kind(&error), "forbidden");
    }

    #[test]
    fn unknown_structured_data_kind_falls_back_to_code_derived_kind() {
        // A fabricated upstream `data.kind` outside the shared allowlist must
        // not pass verbatim — the ErrorCode-derived classification wins.
        let error = ErrorData::internal_error(
            "server exploded",
            Some(serde_json::json!({"kind": "totally_made_up"})),
        );

        assert_eq!(code_mode_mcp_error_kind(&error), "server_error");
    }

    #[test]
    fn mcp_error_data_payload_is_preserved_and_redacted() {
        let err = CapabilityCallError::Mcp {
            data: ErrorData::invalid_params(
                "invalid arguments",
                Some(serde_json::json!({
                    "param": "query",
                    "valid": ["movie.search", "movie.get"],
                    "token": "sk-abcdefghij0123456789extra"
                })),
            ),
            message: "upstream call failed: invalid arguments".to_string(),
        };

        let (kind, message) = code_mode_capability_error_info(&err);

        assert_eq!(kind, "invalid_param");
        assert!(
            message.contains("upstream error data"),
            "payload must be appended: {message}"
        );
        assert!(
            message.contains("query") && message.contains("movie.search"),
            "structured param info must survive: {message}"
        );
        assert!(
            !message.contains("sk-abcdefghij0123456789extra"),
            "secret-like values must be redacted: {message}"
        );
    }

    #[test]
    fn unmapped_mcp_error_kind_includes_numeric_json_rpc_code() {
        let err = CapabilityCallError::Mcp {
            data: ErrorData::new(ErrorCode(-32000), "implementation defined failure", None),
            message: "upstream call failed".to_string(),
        };

        let (kind, message) = code_mode_capability_error_info(&err);

        assert_eq!(kind, "upstream_error");
        assert!(
            message.contains("-32000"),
            "numeric JSON-RPC code must survive for generic kinds: {message}"
        );
    }

    #[test]
    fn queue_saturated_maps_to_queue_saturated_not_rate_limited() {
        let err = CapabilityCallError::QueueSaturated {
            message: "upstream `alpha` concurrency queue timed out".to_string(),
        };

        let (kind, _message) = code_mode_capability_error_info(&err);

        assert_eq!(kind, "queue_saturated");
    }

    #[test]
    fn cancelled_and_input_required_rounds_map_to_explicit_kinds() {
        let (kind, _) = code_mode_capability_error_info(&CapabilityCallError::Cancelled {
            message: "task cancelled".to_string(),
        });
        assert_eq!(kind, "cancelled");

        let (kind, _) =
            code_mode_capability_error_info(&CapabilityCallError::InputRequiredRoundsExceeded {
                message: "input_required did not complete within 4 MRTR rounds".to_string(),
            });
        assert_eq!(kind, "confirmation_required");
    }

    #[test]
    fn non_object_upstream_params_reject_before_pool_lookup() {
        for value in [
            Value::Null,
            Value::Bool(true),
            Value::String("oops".to_string()),
            Value::Array(vec![]),
        ] {
            let err = upstream_arguments("demo", "tool", value).expect_err("must reject");

            assert_eq!(err.kind(), "invalid_param");
        }
    }
}
