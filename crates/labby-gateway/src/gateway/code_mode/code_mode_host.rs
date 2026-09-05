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

use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Map, Value};

use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::gateway::manager::GatewayManager;
use crate::gateway::palette::CapabilityContract;
use crate::upstream::pool::{CapabilityCallError, CheckedToolCallError};
use crate::upstream::tool_error::mcp_error_data_kind;
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};
use labby_runtime::caller_auth::{
    CALLER_AUTH_META_KEY, CALLER_UPSTREAM_SCOPE_META_KEY, PropagatedCallerAuth,
    PropagatedCallerUpstreamScope,
};
use labby_runtime::error::ToolError;
use labby_runtime::lab_home;

use super::search;
use super::tool_error::{completed_tool_error, upstream_tool_safety};
use super::validate_code_mode_params_against_schema;

pub(crate) struct CheckedToolCallOutcome {
    pub(crate) outcome: ToolCallOutcome,
    pub(crate) contract_hash: String,
    pub(crate) catalog_revision: String,
}

struct CheckedDispatch {
    safety: CodeModeToolSafetyHints,
    contract_hash: String,
}

struct CoreProviderCancelOnDrop {
    provider: crate::core_provider::CoreProviderClient,
    assertion: String,
    request_id: String,
    armed: bool,
}

impl CoreProviderCancelOnDrop {
    fn new(
        provider: crate::core_provider::CoreProviderClient,
        assertion: String,
        request_id: String,
    ) -> Self {
        Self {
            provider,
            assertion,
            request_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CoreProviderCancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let provider = self.provider.clone();
        let assertion = self.assertion.clone();
        let request_id = self.request_id.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "core_provider.cancel",
                "could not schedule Core provider cancellation without a Tokio runtime"
            );
            return;
        };
        runtime.spawn(async move {
            match provider.cancel(&assertion, &request_id).await {
                Ok(_) => tracing::info!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "core_provider.cancel",
                    "cancelled an abandoned Core provider call"
                ),
                Err(error) => tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "core_provider.cancel",
                    error = %error,
                    "could not cancel an abandoned Core provider call"
                ),
            }
        });
    }
}

fn stable_request_tag(parent_request_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(parent_request_id.as_bytes());
    hex::encode(&digest[..6])
}

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
        let render = search::build_tools_render(
            self,
            allow_cold_connect,
            &owner,
            oauth_subject,
            allowed,
            scope,
            include_snippets,
            use_cache,
        )
        .await?;

        let Some(provider) = self.core_provider_client.as_ref() else {
            return Ok(render);
        };
        let Some(assertion) = caller.host_provider_token() else {
            return Ok(render);
        };
        if scope
            .allowed_namespaces()
            .is_some_and(|allowed| !allowed.contains("unraid"))
        {
            return Ok(render);
        }

        match provider.code_mode_catalog(assertion).await {
            Ok(core_tools) => crate::core_provider::merge_tools_render(render, core_tools, scope)
                .map_err(|_| ToolError::Sdk {
                    sdk_kind: "provider_incompatible".to_string(),
                    message: "Unraid Core provider returned an incompatible catalog".to_string(),
                }),
            Err(error) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "core_provider.catalog",
                    error = %error,
                    "Unraid Core provider catalog is unavailable"
                );
                Ok(render)
            }
        }
    }

    async fn call_tool(
        &self,
        id: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        ctx: labby_codemode::ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let (upstream, tool) =
            labby_codemode::split_namespaced_id(id).ok_or_else(|| ToolError::Sdk {
                sdk_kind: "invalid_code_mode_id".to_string(),
                message: format!("Code Mode ids must use <namespace>::<tool>: `{id}`"),
            })?;

        if upstream == "unraid" {
            return self
                .call_core_provider(tool, params, caller, surface, scope, ctx)
                .await;
        }
        let owner = runtime_owner(caller, surface);
        let oauth_subject = oauth_subject(caller);

        let upstream_tool = self
            .resolve_code_mode_upstream_tool(upstream, tool, Some(&owner), oauth_subject)
            .await?;

        // Discovery is advisory; authorization is checked again against the
        // live descriptor immediately before invocation so an annotation
        // change or stale render cannot turn a read-only run into a write.
        if scope.is_read_only() && !tool_is_explicitly_read_only(&upstream_tool) {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode.read",
                upstream,
                tool,
                kind = "forbidden",
                "blocked tool without an explicit read-only annotation"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!("Tool `{upstream}::{tool}` is not explicitly read-only."),
            }
            .into());
        }

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
        let checked_contract_hash =
            CapabilityContract::execution_hash_from_upstream_tool(&upstream_tool)?;
        let mut outcome = self
            .execute_upstream_tool_checked(
                upstream,
                tool,
                params,
                &owner,
                oauth_subject,
                Some(propagated_caller_auth(caller)),
                Some(PropagatedCallerUpstreamScope::new(
                    scope.allowed_namespaces().cloned(),
                )),
                &checked_contract_hash,
                destructive_permitted(surface, caller),
                "forbidden",
            )
            .await?
            .outcome;
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

    async fn read_resource(
        &self,
        uri: String,
        caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        let Some(pool) = self.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "provider_unavailable".to_string(),
                message: "gateway upstream pool is unavailable".to_string(),
            });
        };

        let allowed = scope.allowed_namespaces();
        let result = if uri.starts_with("lab://upstream/") {
            let upstream = uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())
                .filter(|name| !name.is_empty())
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: "resource URI must include an upstream name".to_string(),
                })?;
            if allowed.is_some_and(|allowed| !allowed.contains(upstream)) {
                return Err(ToolError::Sdk {
                    sdk_kind: "forbidden".to_string(),
                    message: format!(
                        "resource upstream `{upstream}` is outside this Code Mode scope"
                    ),
                });
            }

            // OAuth upstreams require the caller's subject-scoped connection,
            // matching the native MCP resource path. Plain upstreams use the
            // shared pool path.
            if let Some(config) = self.upstream_config(upstream).await
                && config.oauth.is_some()
            {
                Some(
                    pool.subject_scoped_read_resource(
                        &config,
                        oauth_subject(caller).unwrap_or(""),
                        &uri,
                    )
                    .await,
                )
            } else {
                pool.read_upstream_resource_allowed(&uri, allowed).await
            }
        } else if uri.starts_with("ui://") {
            pool.read_upstream_ui_resource_allowed(&uri, allowed).await
        } else {
            None
        };

        let result = result.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("resource `{uri}` was not found or is not exposed"),
        })?;
        let result = result.map_err(|message| ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message,
        })?;
        serde_json::to_value(result).map_err(|err| {
            ToolError::internal_message(format!("failed to serialize resource: {err}"))
        })
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
            scope,
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
            .ensure_embeddings_for_fingerprint(&render.embedding_fingerprint, &render.entries)
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

pub(super) fn tool_is_explicitly_read_only(tool: &UpstreamTool) -> bool {
    rmcp_tool_is_explicitly_read_only(&tool.tool)
}

fn rmcp_tool_is_explicitly_read_only(tool: &rmcp::model::Tool) -> bool {
    tool.annotations.as_ref().is_some_and(|annotations| {
        annotations.read_only_hint == Some(true) && annotations.destructive_hint == Some(false)
    })
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
    async fn call_core_provider(
        &self,
        tool: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        ctx: labby_codemode::ExecCtx,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let provider = self
            .core_provider_client
            .as_ref()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: "Unraid Core provider is not configured".to_string(),
            })?;
        let assertion = caller.host_provider_token().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "Unraid Core provider requires delegated actor context".to_string(),
        })?;
        let parent_request_id =
            caller
                .host_provider_request_id()
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "forbidden".to_string(),
                    message: "Unraid Core provider requires request correlation".to_string(),
                })?;
        let tools = provider
            .code_mode_catalog(assertion)
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "provider_unavailable".to_string(),
                message: error.to_string(),
            })?;
        let core_tool = tools
            .into_iter()
            .find(|candidate| candidate.descriptor.name == tool)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("Unraid Core operation `unraid::{tool}` was not found"),
            })?;

        if !discovery_entry_visible(&core_tool.descriptor, scope) {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!("Unraid Core operation `unraid::{tool}` is outside this scope"),
            }
            .into());
        }
        let safety = core_tool.descriptor.safety.unwrap_or_default();
        if scope.is_read_only() && safety.read_only != Some(true) {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!("Unraid Core operation `unraid::{tool}` is not read-only"),
            }
            .into());
        }
        if safety.destructive == Some(true) && !destructive_permitted(surface, caller) {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!("Unraid Core operation `unraid::{tool}` requires execute scope"),
            }
            .into());
        }
        if !params.is_object() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: format!("Unraid Core operation `unraid::{tool}` params must be an object"),
            }
            .into());
        }

        let request_id = format!(
            "core-tool-{}-{}-{}",
            stable_request_tag(parent_request_id),
            ctx.seq,
            uuid::Uuid::new_v4()
        );
        let mut cancel_on_drop = CoreProviderCancelOnDrop::new(
            provider.clone(),
            assertion.to_string(),
            request_id.clone(),
        );
        let result = provider
            .execute(
                assertion,
                &request_id,
                &core_tool.operation_id,
                &params,
                &core_tool.schema_version,
            )
            .await;
        cancel_on_drop.disarm();
        let value = result.map_err(|error| ToolError::Sdk {
            sdk_kind: "provider_error".to_string(),
            message: error.to_string(),
        })?;
        Ok(ToolCallOutcome { value, ui: None })
    }

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
    #[cfg(test)]
    pub(crate) async fn execute_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        params: Value,
    ) -> Result<ToolCallOutcome, ToolError> {
        let arguments = upstream_arguments(upstream, tool, params)?;
        let (config, pool) = self.published_config_and_pool().await;
        let pool = pool.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message: "gateway upstream pool is unavailable".to_string(),
        })?;
        let oauth_config = config.upstream.into_iter().find(|candidate| {
            candidate.enabled && candidate.name == upstream && candidate.oauth.is_some()
        });
        self.dispatch_upstream_tool(
            pool,
            upstream,
            tool,
            arguments,
            CodeModeToolSafetyHints::default(),
            oauth_config,
            Some(SHARED_GATEWAY_OAUTH_SUBJECT),
            None,
            None,
        )
        .await
        .map_err(CodeModeCallError::into_tool_error)
    }

    pub(crate) async fn execute_upstream_tool_checked(
        &self,
        upstream: &str,
        tool: &str,
        params: Value,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        caller_auth: Option<PropagatedCallerAuth>,
        caller_scope: Option<PropagatedCallerUpstreamScope>,
        expected_contract_hash: &str,
        destructive_allowed: bool,
        destructive_denial_kind: &'static str,
    ) -> Result<CheckedToolCallOutcome, ToolError> {
        self.execute_upstream_tool_checked_inner(
            upstream,
            tool,
            params,
            owner,
            oauth_subject,
            caller_auth,
            caller_scope,
            expected_contract_hash,
            destructive_allowed,
            destructive_denial_kind,
        )
        .await
        .map_err(CodeModeCallError::into_tool_error)
    }

    async fn execute_upstream_tool_checked_inner(
        &self,
        upstream: &str,
        tool: &str,
        params: Value,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        caller_auth: Option<PropagatedCallerAuth>,
        caller_scope: Option<PropagatedCallerUpstreamScope>,
        expected_contract_hash: &str,
        destructive_allowed: bool,
        destructive_denial_kind: &'static str,
    ) -> Result<CheckedToolCallOutcome, CodeModeCallError> {
        let id = format!("{upstream}::{tool}");
        let arguments =
            upstream_arguments(upstream, tool, params).map_err(CodeModeCallError::from)?;
        if caller_scope
            .as_ref()
            .and_then(|scope| scope.allowed_upstreams.as_ref())
            .is_some_and(|allowed| !allowed.contains(upstream))
        {
            return Err(CodeModeCallError::new(
                "forbidden",
                format!("upstream `{upstream}` is outside the caller scope"),
            )
            .with_tool(id)
            .with_origin(CodeModeErrorOrigin::Policy)
            .with_side_effects(CodeModeSideEffectRisk::NoneExpected));
        }
        let previewed_tool = self
            .resolve_code_mode_upstream_tool(upstream, tool, Some(owner), oauth_subject)
            .await
            .map_err(|error| CodeModeCallError::from(error))?;
        let previewed_contract_hash =
            CapabilityContract::execution_hash_from_upstream_tool(&previewed_tool)
                .map_err(CodeModeCallError::from)?;
        if previewed_contract_hash != expected_contract_hash {
            return Err(contract_changed_call_error(&id));
        }
        if previewed_tool.destructive && !destructive_allowed {
            return Err(CodeModeCallError::new(
                destructive_denial_kind,
                format!("Tool `{upstream}::{tool}` is destructive and not permitted."),
            )
            .with_tool(id.clone())
            .with_origin(CodeModeErrorOrigin::Policy)
            .with_side_effects(CodeModeSideEffectRisk::NoneExpected));
        }
        self.ensure_upstream_tool_runtime_ready(upstream, Some(owner), oauth_subject)
            .await
            .map_err(CodeModeCallError::from)?;
        let (config, pool) = self.published_config_and_pool().await;
        let Some(pool) = pool else {
            return Err(CodeModeCallError::new(
                "upstream_error",
                "gateway upstream pool is unavailable",
            )
            .with_tool(id)
            .with_origin(CodeModeErrorOrigin::UpstreamTransport)
            .with_side_effects(CodeModeSideEffectRisk::NoneExpected));
        };
        if !config.code_mode.enabled {
            return Err(CodeModeCallError::new(
                "contract_changed",
                "the gateway Code Mode catalog changed before dispatch",
            )
            .with_tool(id)
            .with_origin(CodeModeErrorOrigin::Discovery)
            .with_side_effects(CodeModeSideEffectRisk::NoneExpected));
        }
        let upstream_config = config
            .upstream
            .iter()
            .find(|candidate| {
                candidate.enabled && candidate.priority > 0.0 && candidate.name == upstream
            })
            .cloned()
            .ok_or_else(|| {
                CodeModeCallError::new("not_found", format!("upstream tool `{id}` was not found"))
                    .with_tool(id.clone())
            })?;
        let mut upstream_params = CallToolRequestParams::new(tool.to_string());
        upstream_params.arguments = Some(arguments.clone());
        if is_in_process_upstream(upstream)
            && let Some(auth) = caller_auth.as_ref()
        {
            upstream_params.meta = Some(caller_meta(auth, caller_scope.as_ref()));
        }
        let caller_is_read_only = caller_auth.as_ref().is_some_and(|auth| {
            !auth.trusted_local
                && !auth
                    .scopes
                    .iter()
                    .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin" | "mcp:write"))
        });
        let checked = pool
            .checked_call_tool(
                &upstream_config,
                oauth_subject,
                upstream_params,
                |current_tool| {
                    let current_contract_hash =
                        CapabilityContract::execution_hash_from_upstream_tool(current_tool)
                            .map_err(CodeModeCallError::from)?;
                    if current_contract_hash != expected_contract_hash {
                        return Err(contract_changed_call_error(&id).into());
                    }
                    if current_tool.destructive && !destructive_allowed {
                        return Err(CodeModeCallError::new(
                            destructive_denial_kind,
                            format!("Tool `{upstream}::{tool}` is destructive and not permitted."),
                        )
                        .with_tool(id.clone())
                        .with_origin(CodeModeErrorOrigin::Policy)
                        .with_side_effects(CodeModeSideEffectRisk::NoneExpected)
                        .into());
                    }
                    if caller_is_read_only && !tool_is_explicitly_read_only(current_tool) {
                        return Err(CodeModeCallError::new(
                            "forbidden",
                            format!(
                                "Tool `{upstream}::{tool}` is not explicitly annotated as read-only."
                            ),
                        )
                        .with_tool(id.clone())
                        .with_origin(CodeModeErrorOrigin::Policy)
                        .with_side_effects(CodeModeSideEffectRisk::NoneExpected)
                        .into());
                    }
                    validate_code_mode_params_against_schema(
                        &Value::Object(arguments.clone()),
                        current_tool.input_schema.as_ref(),
                    )
                    .map_err(CodeModeCallError::from)
                    .map_err(Box::new)?;
                    Ok(CheckedDispatch {
                        safety: upstream_tool_safety(current_tool),
                        contract_hash: current_contract_hash,
                    })
                },
            )
            .await
            .map_err(|error| map_checked_call_error(error, &id))?;
        let outcome = self
            .finish_dispatched_tool(
                Arc::clone(&pool),
                upstream,
                tool,
                checked.checked.safety,
                Some(Ok(checked.result)),
            )
            .await?;
        Ok(CheckedToolCallOutcome {
            outcome,
            contract_hash: checked.checked.contract_hash,
            catalog_revision: checked.catalog_revision,
        })
    }

    #[allow(dead_code, clippy::too_many_arguments)]
    async fn dispatch_upstream_tool(
        &self,
        pool: Arc<crate::upstream::pool::UpstreamPool>,
        upstream: &str,
        tool: &str,
        arguments: Map<String, Value>,
        safety: CodeModeToolSafetyHints,
        oauth_config: Option<labby_runtime::gateway_config::UpstreamConfig>,
        oauth_subject: Option<&str>,
        caller_auth: Option<PropagatedCallerAuth>,
        caller_scope: Option<PropagatedCallerUpstreamScope>,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let id = format!("{upstream}::{tool}");
        let mut upstream_params = CallToolRequestParams::new(tool.to_string());
        upstream_params.arguments = Some(arguments);
        // Carry the caller's authorization across the in-process hop, and only
        // there. Local rmcp extensions (where `AuthContext` normally lives) do
        // not survive that pipe's serialization boundary, so without this the
        // mini server sees `None` for everyone — which previously meant it
        // trusted everyone, and now means it trusts no one. `_meta` does
        // serialize.
        //
        // Scoped to in-process upstreams deliberately: a real upstream is a
        // third party and has no business receiving Labby's caller identity.
        if is_in_process_upstream(upstream)
            && let Some(auth) = caller_auth.as_ref()
        {
            upstream_params.meta = Some(caller_meta(auth, caller_scope.as_ref()));
        }
        let call_result = if let Some(config) = oauth_config {
            let subject = oauth_subject.ok_or_else(|| {
                CodeModeCallError::new(
                    "auth_failed",
                    format!(
                        "upstream `{upstream}` requires an authenticated subject for tool execution"
                    ),
                )
                .with_tool(id.clone())
            })?;
            Some(
                pool.subject_scoped_call_tool_classified(&config, subject, upstream_params)
                    .await,
            )
        } else {
            pool.call_tool_classified(upstream, upstream_params).await
        };
        self.finish_dispatched_tool(pool, upstream, tool, safety, call_result)
            .await
    }

    async fn finish_dispatched_tool(
        &self,
        pool: Arc<crate::upstream::pool::UpstreamPool>,
        upstream: &str,
        tool: &str,
        safety: CodeModeToolSafetyHints,
        call_result: Option<Result<CallToolResult, CapabilityCallError>>,
    ) -> Result<ToolCallOutcome, CodeModeCallError> {
        let id = format!("{upstream}::{tool}");
        match call_result {
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

fn contract_changed_call_error(id: &str) -> CodeModeCallError {
    CodeModeCallError::new(
        "contract_changed",
        format!("Tool `{id}` changed before dispatch; rediscover it and retry."),
    )
    .with_tool(id.to_string())
    .with_origin(CodeModeErrorOrigin::Discovery)
    .with_side_effects(CodeModeSideEffectRisk::NoneExpected)
}

fn map_checked_call_error(error: CheckedToolCallError, id: &str) -> CodeModeCallError {
    match error {
        CheckedToolCallError::Check(error) => *error,
        CheckedToolCallError::MissingTool => contract_changed_call_error(id),
        CheckedToolCallError::Unavailable => {
            CodeModeCallError::new("not_found", format!("upstream tool `{id}` was not found"))
                .with_tool(id.to_string())
        }
        CheckedToolCallError::Connect(message) => CodeModeCallError::new(
            "auth_failed",
            labby_runtime::agent_error::sanitize_error_text(
                &message,
                MAX_UPSTREAM_MCP_MESSAGE_CHARS,
            ),
        )
        .with_tool(id.to_string()),
        CheckedToolCallError::Catalog { kind, message } => CodeModeCallError::new(
            kind,
            labby_runtime::agent_error::sanitize_error_text(
                &message,
                MAX_UPSTREAM_MCP_MESSAGE_CHARS,
            ),
        )
        .with_tool(id.to_string())
        .with_origin(CodeModeErrorOrigin::Discovery)
        .with_side_effects(CodeModeSideEffectRisk::NoneExpected),
        CheckedToolCallError::Capability(error) => {
            let (kind, message) = code_mode_capability_error_info(&error);
            match error {
                CapabilityCallError::Mcp { .. } => CodeModeCallError::new(
                    kind,
                    labby_runtime::agent_error::sanitize_error_text(
                        &message,
                        MAX_UPSTREAM_MCP_MESSAGE_CHARS,
                    ),
                )
                .with_tool(id.to_string()),
                _ => CodeModeCallError::upstream_transport_classified(
                    id.to_string(),
                    kind,
                    message,
                    CodeModeToolSafetyHints::default(),
                ),
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
///
/// This is a locked contract (docs/contracts/mcp-tool-output.md
/// §C6), byte-identical since `977cb2166` (2026-05-31). Do not change the
/// behavior without updating that contract and the edge-case matrix tests
/// below. Precedence — first match wins; rule 0 (`is_error == Some(true)`)
/// is handled by the caller *before* this function and never reaches it:
///
/// | # | Condition | Result |
/// |---|---|---|
/// | 1 | `structured_content` is `Some(v)` | `v` as-is — including falsy JSON (`false`, `0`, `null`, `""`) |
/// | 2 | `content` non-empty, every block text | joined with `"\n"`, then ONE `serde_json` parse; on failure the joined string |
/// | 3 | `content` empty | `Value::Null` |
/// | 4 | otherwise (mixed / binary) | the entire `CallToolResult` as JSON, including upstream `_meta` |
///
/// Invariants: rule 1 precedes any inspection of `content` (when both are
/// present the structured value wins and content blocks are discarded;
/// mcp-ui links are unaffected because they are read from `_meta` via
/// `extract_ui_link`, not `content`). `if let Some(..)` tests presence, not
/// truthiness. A structured value is never stringified. Divergences from
/// Cloudflare's `unwrapMcpResult`: no legacy `toolResult` field (rmcp has
/// none), and empty content yields `Null` rather than the raw result.
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
            let kind = mcp_error_data_kind(data);
            let mut message = data.message.to_string();
            // Preserve the structured payload (param/valid/hint/retry_after_ms/
            // required_scopes/…) the old stringified Display used to carry.
            // Redacted (secret keys + secret-shaped strings) and bounded.
            if let Some(payload) = data.data.as_ref() {
                let redacted = labby_runtime::redact::redact_trace_value(
                    payload,
                    UPSTREAM_ERROR_DATA_CAP_BYTES,
                );
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

// The JSON-RPC `ErrorData` → stable-kind classification (allowlisted
// `data.kind`, else `ErrorCode`-derived) lives in
// `crate::upstream::tool_error::mcp_error_data_kind` — shared with the MCP
// upstream proxy so both surfaces emit the same model-facing kind.

/// True when `upstream` names one of Labby's own in-process service peers.
fn is_in_process_upstream(upstream: &str) -> bool {
    // Same constant the name is minted from, and the one `UpstreamConfig`
    // reserves — a literal here is how a third-party upstream ends up silently
    // treated as in-process and handed the caller's identity.
    upstream.starts_with(labby_runtime::gateway_config::IN_PROCESS_UPSTREAM_PREFIX)
}

/// A Code Mode caller's authorization facts, in propagatable form.
///
/// Scopes travel, not a decision: the receiving gate applies its own rules, so
/// an action whose requirements differ from Code Mode's is still evaluated
/// correctly rather than against a stale yes/no.
pub(crate) fn propagated_caller_auth(caller: &CodeModeCaller) -> PropagatedCallerAuth {
    match caller {
        CodeModeCaller::TrustedLocal => PropagatedCallerAuth::trusted_local(),
        CodeModeCaller::Scoped { capabilities, sub } => {
            // The kernel deliberately keeps Lab's scope vocabulary out of its
            // own types, so this adapter boundary is where the names come back.
            let mut scopes = Vec::new();
            if capabilities.is_admin {
                scopes.push("lab:admin".to_string());
            }
            if capabilities.can_execute {
                scopes.push("lab".to_string());
            }
            if capabilities.can_read {
                scopes.push("lab:read".to_string());
            }
            PropagatedCallerAuth::scoped(scopes, sub.clone())
        }
        CodeModeCaller::ScopedPrivate {
            capabilities,
            sub,
            context_token,
        } => {
            let mut scopes = Vec::new();
            if capabilities.is_admin {
                scopes.push("lab:admin".to_string());
            }
            if capabilities.can_execute {
                scopes.push("lab".to_string());
            }
            if capabilities.can_read {
                scopes.push("lab:read".to_string());
            }
            PropagatedCallerAuth::scoped(scopes, sub.clone())
                .with_private_context_token(context_token.clone())
        }
        CodeModeCaller::ScopedHostProvider {
            capabilities, sub, ..
        } => {
            let mut scopes = Vec::new();
            if capabilities.is_admin {
                scopes.push("lab:admin".to_string());
            }
            if capabilities.can_execute {
                scopes.push("lab".to_string());
            }
            if capabilities.can_read {
                scopes.push("lab:read".to_string());
            }
            PropagatedCallerAuth::scoped(scopes, sub.clone())
        }
    }
}

fn caller_meta(
    auth: &PropagatedCallerAuth,
    scope: Option<&PropagatedCallerUpstreamScope>,
) -> rmcp::model::RequestMetaObject {
    let mut meta = rmcp::model::RequestMetaObject::default();
    if let Ok(value) = serde_json::to_value(auth) {
        meta.insert(CALLER_AUTH_META_KEY.to_string(), value);
    }
    if let Some(scope) = scope
        && let Ok(value) = serde_json::to_value(scope)
    {
        meta.insert(CALLER_UPSTREAM_SCOPE_META_KEY.to_string(), value);
    }
    meta
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use super::*;
    use crate::gateway::runtime::GatewayRuntimeHandle;
    use labby_codemode::ExecCtx;
    use rmcp::model::{ContentBlock, ErrorCode, ErrorData, MetaObject};
    #[cfg(unix)]
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    #[cfg(unix)]
    #[tokio::test]
    async fn abandoned_core_provider_call_sends_correlated_cancel() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("provider.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let expected_length = loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap();
                    break header_end + 4 + content_length;
                }
            };
            while request.len() < expected_length {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
            }
            let body = br#"{"outcome":"cancelled_before_attempt","request_id":"provider-call-1"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
            String::from_utf8(request).unwrap()
        });

        let provider = crate::core_provider::CoreProviderClient::new(&socket_path).unwrap();
        let guard = CoreProviderCancelOnDrop::new(
            provider,
            "delegated-assertion".to_string(),
            "provider-call-1".to_string(),
        );
        drop(guard);

        let request = tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("cancel request was sent")
            .unwrap();
        assert!(request.contains("authorization: Bearer delegated-assertion"));
        assert!(request.contains("\"op\":\"cancel\""));
        assert!(request.contains("\"request_id\":\"provider-call-1\""));
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
    fn caller_meta_carries_auth_and_upstream_scope_together() {
        let auth =
            PropagatedCallerAuth::scoped(vec!["lab:read".to_string()], Some("alice".to_string()));
        let scope = PropagatedCallerUpstreamScope::new(Some(std::collections::BTreeSet::from([
            "github".to_string(),
            "docs".to_string(),
        ])));
        let meta = caller_meta(&auth, Some(&scope));

        let decoded_auth: PropagatedCallerAuth = serde_json::from_value(
            meta.get(CALLER_AUTH_META_KEY)
                .expect("caller auth metadata")
                .clone(),
        )
        .expect("auth decodes");
        let decoded_scope: PropagatedCallerUpstreamScope = serde_json::from_value(
            meta.get(CALLER_UPSTREAM_SCOPE_META_KEY)
                .expect("caller scope metadata")
                .clone(),
        )
        .expect("scope decodes");
        assert_eq!(decoded_auth, auth);
        assert_eq!(decoded_scope, scope);
    }

    #[test]
    fn caller_meta_without_scope_never_invents_one() {
        let auth = PropagatedCallerAuth::trusted_local();
        let meta = caller_meta(&auth, None);
        assert!(meta.get(CALLER_AUTH_META_KEY).is_some());
        assert!(meta.get(CALLER_UPSTREAM_SCOPE_META_KEY).is_none());
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

    #[test]
    fn read_only_access_requires_an_explicit_read_only_hint() {
        fn upstream_with_annotations(
            annotations: Option<rmcp::model::ToolAnnotations>,
        ) -> UpstreamTool {
            let mut tool =
                rmcp::model::Tool::new("query".to_string(), "Query data", Arc::new(Map::new()));
            tool.annotations = annotations;
            UpstreamTool {
                tool,
                input_schema: None,
                output_schema: None,
                upstream_name: Arc::from("fixture"),
                destructive: false,
            }
        }

        assert!(!tool_is_explicitly_read_only(&upstream_with_annotations(
            None
        )));
        assert!(!tool_is_explicitly_read_only(&upstream_with_annotations(
            Some(rmcp::model::ToolAnnotations::new().destructive(false),)
        )));
        assert!(!tool_is_explicitly_read_only(&upstream_with_annotations(
            Some(rmcp::model::ToolAnnotations::new().read_only(true),)
        )));
        assert!(tool_is_explicitly_read_only(&upstream_with_annotations(
            Some(
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            )
        )));
        assert!(!tool_is_explicitly_read_only(&upstream_with_annotations(
            Some(
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(true),
            )
        )));

        let hinted = upstream_with_annotations(Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(false),
        ));
        assert!(tool_is_explicitly_read_only(&hinted));
    }

    #[test]
    fn code_mode_contract_digest_detects_security_relevant_descriptor_drift() {
        let make = |description: &str, read_only: bool| {
            let mut tool = rmcp::model::Tool::new(
                "query".to_string(),
                description.to_string(),
                Arc::new(Map::from_iter([(
                    "type".to_string(),
                    serde_json::json!("object"),
                )])),
            );
            tool.annotations = Some(
                rmcp::model::ToolAnnotations::new()
                    .read_only(read_only)
                    .destructive(!read_only),
            );
            UpstreamTool {
                tool,
                input_schema: None,
                output_schema: None,
                upstream_name: Arc::from("fixture"),
                destructive: !read_only,
            }
        };

        let checked = make("Query data", true);
        let checked_hash = CapabilityContract::from_upstream_tool(&checked)
            .expect("checked contract")
            .contract_hash;
        assert_eq!(
            checked_hash,
            CapabilityContract::from_upstream_tool(&checked.clone())
                .expect("cloned contract")
                .contract_hash
        );
        assert_eq!(
            checked_hash,
            CapabilityContract::from_upstream_tool(&make("Changed contract", true))
                .expect("description-only change")
                .contract_hash
        );
        assert_ne!(
            checked_hash,
            CapabilityContract::from_upstream_tool(&make("Query data", false))
                .expect("safety change")
                .contract_hash
        );
        let mut oversized = checked.clone();
        oversized.input_schema = Some(serde_json::json!({
            "type": "object",
            "description": "x".repeat(70 * 1024)
        }));
        assert!(CapabilityContract::from_upstream_tool(&oversized).is_err());
        let oversized_hash = CapabilityContract::execution_hash_from_upstream_tool(&oversized)
            .expect("Code Mode execution hash must accept large schemas");
        assert_eq!(oversized_hash.len(), 64);
        assert_ne!(checked_hash, oversized_hash);
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
            assert_eq!(mcp_error_data_kind(&error), expected);
        }
    }

    #[test]
    fn structured_mcp_error_kind_takes_precedence_over_generic_code() {
        let error = ErrorData::invalid_request(
            "scope denied",
            Some(serde_json::json!({"kind": "forbidden"})),
        );

        assert_eq!(mcp_error_data_kind(&error), "forbidden");
    }

    #[test]
    fn unknown_structured_data_kind_falls_back_to_code_derived_kind() {
        // A fabricated upstream `data.kind` outside the shared allowlist must
        // not pass verbatim — the ErrorCode-derived classification wins.
        let error = ErrorData::internal_error(
            "server exploded",
            Some(serde_json::json!({"kind": "totally_made_up"})),
        );

        assert_eq!(mcp_error_data_kind(&error), "server_error");
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

    // ── Issue #210 (lab-41e7m.2): the C6 unwrap precedence matrix ───────────
    //
    // `unwrap_code_mode_upstream_result` is a locked contract
    // (docs/contracts/mcp-tool-output.md §C6). These tests pin the
    // behavior; they do not define it. Rule 0 (`is_error`) is handled by the
    // caller before the unwrap — its conversion to `CodeModeCallError` is
    // pinned by `gateway/code_mode/tool_error.rs::adapter_preserves_shared_analysis`.

    /// C6 rule 1: a present-but-falsy structured value MUST NOT be treated as
    /// absent — `if let Some(..)` tests presence, not truthiness.
    #[test]
    fn unwrap_returns_falsy_structured_values_verbatim() {
        for falsy in [
            Value::Bool(false),
            serde_json::json!(0),
            Value::Null,
            Value::String(String::new()),
        ] {
            let mut result = CallToolResult::success(vec![ContentBlock::text("ignored")]);
            result.structured_content = Some(falsy.clone());
            assert_eq!(
                unwrap_code_mode_upstream_result(result),
                falsy,
                "falsy structured value must be returned as-is"
            );
        }
    }

    /// C6 rule 1 precedes content inspection: when both are present the
    /// structured value wins, content blocks are discarded, and the mcp-ui
    /// link is unaffected because it reads `_meta`, not `content`.
    #[test]
    fn unwrap_prefers_structured_content_and_meta_ui_link_survives() {
        let mut result = CallToolResult::success(vec![
            ContentBlock::text("textual rendering"),
            ContentBlock::text("of the same data"),
        ]);
        result.structured_content = Some(serde_json::json!({"rows": [1, 2, 3]}));
        result.meta = Some(MetaObject(Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({"resourceUri": "ui://demo/widget.html"}),
        )])));

        let ui = extract_ui_link(&result).expect("ui link from _meta");
        assert_eq!(
            ui_resource_uri(&ui.ui_meta),
            Some("ui://demo/widget.html"),
            "_meta ui link is captured independently of the unwrap"
        );
        assert_eq!(
            unwrap_code_mode_upstream_result(result),
            serde_json::json!({"rows": [1, 2, 3]}),
            "structured content wins over text blocks"
        );
    }

    /// C6 rule 2: all text blocks are joined with `\n` before a SINGLE parse
    /// attempt — valid-after-join parses, split-mid-token falls back to the
    /// joined string (never a per-block parse, never a stringified re-wrap).
    #[test]
    fn unwrap_joins_all_text_blocks_before_one_parse() {
        let valid_after_join = CallToolResult::success(vec![
            ContentBlock::text("{\"page\": 1,"),
            ContentBlock::text("\"total\": 2}"),
        ]);
        assert_eq!(
            unwrap_code_mode_upstream_result(valid_after_join),
            serde_json::json!({"page": 1, "total": 2}),
            "blocks that form valid JSON after the newline join must parse"
        );

        let split_mid_token = CallToolResult::success(vec![
            ContentBlock::text("{\"pa"),
            ContentBlock::text("ge\": 1}"),
        ]);
        assert_eq!(
            unwrap_code_mode_upstream_result(split_mid_token),
            Value::String("{\"pa\nge\": 1}".to_string()),
            "the newline join lands inside the token, so the parse fails and the joined string is returned"
        );
    }

    /// C6 rule 3: empty content unwraps to `Null` (divergence from
    /// Cloudflare's `unwrapMcpResult`, which returns the raw result).
    #[test]
    fn unwrap_empty_content_yields_null() {
        let result = CallToolResult::success(vec![]);
        assert_eq!(unwrap_code_mode_upstream_result(result), Value::Null);
    }

    /// C6 rule 4: mixed content returns the whole `CallToolResult` as JSON.
    /// The upstream's `_meta` is deliberately exposed to sandbox code on this
    /// path — assert exactly that, so the exposure is a pinned decision
    /// rather than an accident.
    #[test]
    fn unwrap_mixed_content_returns_raw_result_including_meta() {
        let mut result = CallToolResult::success(vec![
            ContentBlock::text("caption"),
            ContentBlock::image("aGVsbG8=", "image/png"),
        ]);
        result.meta = Some(MetaObject(Map::from_iter([(
            "upstreamKey".to_string(),
            serde_json::json!("upstream-controlled"),
        )])));

        let value = unwrap_code_mode_upstream_result(result);
        assert!(
            value.get("content").is_some(),
            "mixed content must expose the raw result"
        );
        assert_eq!(
            value["_meta"]["upstreamKey"],
            serde_json::json!("upstream-controlled"),
            "rule 4 exposes upstream _meta to sandbox code (CONTRACT §C6)"
        );
        assert_eq!(
            value["isError"],
            serde_json::json!(false),
            "rule 4 serializes the whole result, including the explicit isError: false"
        );
    }
}
