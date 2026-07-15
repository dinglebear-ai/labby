//! `CodeModeHost`: the tool-source-neutral seam between the Code Mode kernel and
//! whatever provides its tools (an MCP proxy pool, a REST client, an in-memory
//! stub — the kernel can't tell).
//!
//! The trait vocabulary is deliberately neutral. A tool is an opaque string
//! `id` (`<namespace>::<tool>`) plus JSON params; a tool descriptor is the
//! neutral [`ToolDescriptor`]; the visibility filter is the neutral
//! [`ToolScope`]. Each host converts its own tool representation into a
//! `ToolDescriptor` inside its `CodeModeHost` impl, so the kernel never learns
//! what backs the namespace.

use std::sync::Arc;

use serde_json::Value;

use crate::error::ToolError;
use crate::pool::RunnerPool;
use crate::types::{CodeModeCaller, CodeModeSurface, ToolDescriptor, ToolScope, UiLink};
use labby_runtime::CodeModeConfig;

/// A rendered Code Mode discovery catalog: the descriptors the sandbox's
/// `search`/`describe`/proxy read, plus their pre-serialized JSON form.
///
/// Hosts may serve this from a render cache keyed on a cheap fingerprint of
/// their tool set; the kernel does not require caching and treats this purely
/// as a projection.
///
/// `entries`/`catalog_json` are `Arc`-wrapped so a cache hit is a refcount
/// bump, not a deep clone — `codemode.describe()` calls `list_tools()` again
/// per invocation (see `execute.rs`'s `describe_types` dispatch), so a host
/// whose cache stores owned `Vec`/`String` would re-pay a full catalog clone
/// on every `describe()` call within one execution, not just once at start.
#[derive(Debug, Clone)]
pub struct ToolsRender {
    /// Fingerprint of the live tool set this render was built from (sorted
    /// tool ids + snippet directory state). Hosts key auxiliary per-catalog
    /// caches (e.g. embedding vectors) off this without recomputing it
    /// themselves.
    pub fingerprint: String,
    /// The descriptors (tools + snippets) visible to this execution.
    pub entries: Arc<Vec<ToolDescriptor>>,
    /// `serde_json::to_string(&entries)` — the `const tools = ...` payload.
    pub catalog_json: Arc<str>,
    /// Serialized catalog size in bytes (for tracing).
    pub serialized_size: usize,
}

/// A snippet resolved by the host: its canonical name plus the JS source and
/// the merged input the runner should execute it with.
#[derive(Debug, Clone)]
pub struct ResolvedSnippet {
    pub name: String,
    pub code: String,
    pub input: Value,
}

/// The result of one host-brokered tool call: the unwrapped JSON value plus an
/// optional captured MCP Apps (mcp-ui) widget link (last-wins across the run).
#[derive(Debug, Clone)]
pub struct ToolCallOutcome {
    pub value: Value,
    pub ui: Option<UiLink>,
}

/// Per-call execution context threaded from the runner drive layer into
/// [`CodeModeHost`] hooks. Carries the protocol `seq` for this call, the
/// durable-run `execution_id` (None on the write-free/standalone path), and,
/// for a `codemode.step` boundary, the parent-derived `step_ordinal` (a
/// monotonic count of step_begin events — the journal key ordinate, distinct
/// from `seq`).
#[derive(Debug, Clone)]
pub struct ExecCtx {
    pub seq: u64,
    pub execution_id: Option<Arc<str>>,
    pub step_ordinal: Option<u64>,
}

impl ExecCtx {
    /// The write-free context used when no durable run is active.
    #[must_use]
    pub fn none() -> Self {
        Self {
            seq: 0,
            execution_id: None,
            step_ordinal: None,
        }
    }
}

/// The durable decision for one `codemode.step(name, fn)` boundary, returned by
/// [`CodeModeHost::decide_step`] BEFORE the sandbox runs `fn`.
///
/// Port of the step half of Cloudflare's `codemode.step` prelude
/// (`proxy-tool.ts:231-241`): `decide()` runs first; a non-execute decision is
/// a replay (return the cached value without running `fn`), an execute decision
/// runs `fn` then records the result. Labby folds Cloudflare's pause/diverge
/// reasons into an explicit `Error` variant so the driver can map them onto a
/// sandbox rejection.
#[derive(Debug, Clone)]
pub enum StepDecision {
    /// The step was journaled on a prior pass — return `value`, do NOT run `fn`.
    Replay(Value),
    /// Run `fn` for real, then call [`CodeModeHost::record_step`].
    Execute,
    /// Divergence / pause / fail — reject the step in the sandbox with this
    /// `(kind, message)` (mirrors a rejected `callTool`).
    Error { kind: String, message: String },
}

/// Injects the tool source into the Code Mode kernel.
///
/// Implementations live entirely outside this crate. Methods take the neutral
/// [`ToolScope`] / [`CodeModeCaller`] / [`CodeModeSurface`]; how those map onto
/// a concrete credential or connection model is the host's business.
pub trait CodeModeHost: Send + Sync {
    /// Project the host's tool source into the in-sandbox discovery catalog the
    /// `tools` proxy + in-sandbox `search`/`describe` read. Pure projection; no
    /// transport implied.
    fn list_tools(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        include_snippets: bool,
        use_cache: bool,
    ) -> impl Future<Output = Result<ToolsRender, ToolError>> + Send;

    /// Route a `callTool(id, params)` to the host's tool source and return the
    /// unwrapped result (plus any captured widget link). The kernel has already
    /// checked the id against `scope`.
    ///
    /// `ctx` carries the protocol `seq` for this call.
    fn call_tool(
        &self,
        id: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        ctx: ExecCtx,
    ) -> impl Future<Output = Result<ToolCallOutcome, ToolError>> + Send;

    /// Decide replay-vs-execute for a `codemode.step(name, fn)` boundary at
    /// `(execution_id, seq)`, BEFORE the sandbox runs `fn`. The step consumes a
    /// `seq` from the same monotonic spine as `call_tool`, so it participates in
    /// the durable replay cursor.
    ///
    /// The default impl always returns [`StepDecision::Execute`], so `fn` runs
    /// normally; no host currently overrides this hook.
    fn decide_step(&self, ctx: ExecCtx, name: &str) -> impl Future<Output = StepDecision> + Send {
        let _ = (&ctx, name);
        async { StepDecision::Execute }
    }

    /// Record the value a step's `fn` produced (decision was execute) so a
    /// later resume replays it without re-running `fn`. `name` + `ctx.step_ordinal`
    /// + `ctx.execution_id` form the journal key.
    ///
    /// The default impl is a no-op `Ok(())`; no host currently overrides this
    /// hook, so `fn` is simply re-run on any re-execution.
    fn record_step(
        &self,
        ctx: ExecCtx,
        name: &str,
        value: &Value,
    ) -> impl Future<Output = Result<(), ToolError>> + Send {
        let _ = (&ctx, name, value);
        async { Ok(()) }
    }

    /// Decide replay-vs-execute for a runner-reserved LOCAL provider call
    /// (`state::*` / `git::*`) at `(execution_id, seq)`, journaling it as an
    /// **ephemeral** durable entry.
    ///
    /// Local providers dispatch inside the runner crate (not via `call_tool`),
    /// so this hook is where a future durable-journaling host would keep them
    /// aligned with the `seq` spine and divergence-check their tool_id/args.
    ///
    /// `id` is the reserved `<namespace>::<method>` id; `params` are the call
    /// args (the divergence key). The default impl returns [`StepDecision::Execute`]
    /// so local calls dispatch unchanged; no host currently overrides this hook.
    fn decide_local(
        &self,
        ctx: ExecCtx,
        id: &str,
        params: &Value,
    ) -> impl Future<Output = StepDecision> + Send {
        let _ = (&ctx, id, params);
        async { StepDecision::Execute }
    }

    /// Record that a local-provider call was applied (marks the ephemeral entry
    /// `applied`). Because the entry is ephemeral it re-executes on replay
    /// regardless, so the recorded value is a marker, not a replay source. The
    /// default impl is a no-op `Ok(())` for the write-free path.
    fn record_local(
        &self,
        ctx: ExecCtx,
        value: &Value,
    ) -> impl Future<Output = Result<(), ToolError>> + Send {
        let _ = (&ctx, value);
        async { Ok(()) }
    }

    /// Resolve a Code Mode snippet by name (engine lives in-crate; only the
    /// source lookup is host-provided so policy/visibility stays host-side).
    fn resolve_snippet(
        &self,
        name: &str,
        input: Value,
    ) -> impl Future<Output = Result<ResolvedSnippet, ToolError>> + Send;

    /// Rank the host's Code Mode catalog by semantic similarity to `query`,
    /// for the exact same `caller`/`surface`/`scope` that would be passed to
    /// `list_tools`/`call_tool` for this execution. Returns `(entry_id,
    /// similarity)` pairs, descending by similarity, capped to `top_k`.
    ///
    /// Hosts with no embedding service configured (or currently in a failure
    /// cooldown) MUST return `Ok(Vec::new())` rather than an `Err` — an empty
    /// result is the fail-open signal `codemode.search()` uses to skip
    /// semantic scoring for that call. `Err` is reserved for genuine
    /// host-side bugs, not for "the embedding service is unreachable".
    ///
    /// Implementations must only ever return ids that are members of the
    /// SAME scope-filtered entry set `list_tools` would return for these
    /// exact `caller`/`surface`/`scope` — this is a security invariant, not
    /// an optimization: the caller (`call_tool_id`) intentionally does not
    /// re-check `scope.allows()` on this method's results.
    fn semantic_rank(
        &self,
        query: String,
        top_k: usize,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> impl Future<Output = Result<Vec<(String, f32)>, ToolError>> + Send;

    /// Code Mode configuration (timeouts, log/response caps).
    fn config(&self) -> impl Future<Output = CodeModeConfig> + Send;

    /// The host-owned warm runner pool the kernel checks runners out of.
    fn runner_pool(&self) -> &RunnerPool;

    /// The host-owned registry of loaded OpenAPI specs for the `openapi` local
    /// provider. REQUIRED (no default): a missed override is a compile error, not
    /// a silent feature disable. Hosts with no specs return
    /// `OpenApiRegistry::default()` (empty).
    fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry;

    /// The host-owned hardened `reqwest` client for `openapi` dispatch. REQUIRED
    /// (no default). Tests return `labby_openapi::http::build_dispatch_client()`.
    fn openapi_http_client(&self) -> reqwest::Client;
}

/// A no-op host used by tests that drive the runner kernel directly without a
/// real tool source: it exposes no tools, rejects all tool/snippet calls, and
/// owns its own warm pool. Never constructed in the production build.
#[cfg(test)]
pub(crate) struct NoopHost {
    pool: RunnerPool,
}

#[cfg(test)]
impl Default for NoopHost {
    fn default() -> Self {
        Self {
            pool: RunnerPool::from_env().expect("test process must expose current executable"),
        }
    }
}

#[cfg(test)]
impl CodeModeHost for NoopHost {
    async fn list_tools(
        &self,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
        _include_snippets: bool,
        _use_cache: bool,
    ) -> Result<ToolsRender, ToolError> {
        Ok(ToolsRender {
            fingerprint: "noop".to_string(),
            entries: Arc::new(Vec::new()),
            catalog_json: Arc::from("[]"),
            serialized_size: 2,
        })
    }

    async fn call_tool(
        &self,
        _id: &str,
        _params: Value,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
        _ctx: ExecCtx,
    ) -> Result<ToolCallOutcome, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: "NoopHost exposes no tools".to_string(),
        })
    }

    async fn resolve_snippet(
        &self,
        _name: &str,
        _input: Value,
    ) -> Result<ResolvedSnippet, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: "NoopHost exposes no snippets".to_string(),
        })
    }

    async fn semantic_rank(
        &self,
        _query: String,
        _top_k: usize,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        Ok(Vec::new())
    }

    async fn config(&self) -> CodeModeConfig {
        CodeModeConfig::default()
    }

    fn runner_pool(&self) -> &RunnerPool {
        &self.pool
    }

    fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry {
        labby_openapi::OpenApiRegistry::default()
    }

    fn openapi_http_client(&self) -> reqwest::Client {
        labby_openapi::http::build_dispatch_client().expect("test dispatch client")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_ctx_none_is_write_free() {
        let ctx = ExecCtx::none();
        assert_eq!(ctx.seq, 0);
        assert!(ctx.execution_id.is_none());
        assert!(ctx.step_ordinal.is_none());
    }

    #[test]
    fn exec_ctx_carries_execution_id_and_ordinal() {
        let ctx = ExecCtx {
            seq: 7,
            execution_id: Some(Arc::from("exec_abc")),
            step_ordinal: Some(2),
        };
        assert_eq!(ctx.seq, 7);
        assert_eq!(ctx.execution_id.as_deref(), Some("exec_abc"));
        assert_eq!(ctx.step_ordinal, Some(2));
    }

    /// A host that does not override the journaling hooks (`NoopHost`) uses the
    /// trait DEFAULT impls: `decide_step`/`decide_local` return `Execute` and
    /// `record_step`/`record_local` are no-op `Ok(())`, so `codemode.step`'s `fn`
    /// and local provider calls run normally without any durable journaling.
    #[tokio::test]
    async fn default_step_and_local_hooks_execute_and_noop() {
        let host = NoopHost::default();
        assert!(matches!(
            host.decide_step(ExecCtx::none(), "s").await,
            StepDecision::Execute
        ));
        assert!(
            host.record_step(ExecCtx::none(), "s", &Value::Null)
                .await
                .is_ok()
        );
        assert!(matches!(
            host.decide_local(ExecCtx::none(), "state::writeFile", &Value::Null)
                .await,
            StepDecision::Execute
        ));
        assert!(
            host.record_local(ExecCtx::none(), &Value::Null)
                .await
                .is_ok()
        );
    }
}
