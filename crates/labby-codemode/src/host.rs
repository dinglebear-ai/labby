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
#[derive(Debug, Clone)]
pub struct ToolsRender {
    /// Fingerprint of the live tool set this render was built from (sorted
    /// tool ids + snippet directory state). Hosts key auxiliary per-catalog
    /// caches (e.g. embedding vectors) off this without recomputing it
    /// themselves.
    pub fingerprint: String,
    /// The descriptors (tools + snippets) visible to this execution.
    pub entries: Vec<ToolDescriptor>,
    /// `serde_json::to_string(&entries)` — the `const tools = ...` payload.
    pub catalog_json: String,
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
/// [`CodeModeHost::call_tool`]. Carries the durable-run `execution_id` (when the
/// run is on the pause-capable path) and the protocol `seq` for this call.
///
/// `execution_id` is `None` for the no-decider / standalone / write-free path
/// (CLI runs, pre-confirmed runs, tests) — the host then dispatches directly
/// without journaling.
#[derive(Debug, Clone, Copy)]
pub struct ExecCtx<'a> {
    pub execution_id: Option<&'a str>,
    pub seq: u64,
}

impl ExecCtx<'_> {
    /// The write-free context used when no durable run is active.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            execution_id: None,
            seq: 0,
        }
    }
}

/// The durable-execution decision for a single tool call or step.
///
/// Port of Cloudflare's `ToolDecision` (`runtime.ts:127-134`), plus explicit
/// `Diverge`/`Fail` terminal variants (Cloudflare routes both through a `pause`
/// decision whose reason lives on the execution record; Labby's host returns a
/// typed outcome so the driver can map it onto a `ToolError` directly).
#[derive(Debug, Clone)]
pub enum DecideOutcome {
    /// Return the cached result, do NOT dispatch (`runtime.ts:464`).
    Replay(Value),
    /// Dispatch for real, then call `record_result` (`runtime.ts:527`).
    Execute,
    /// Run flipped to `paused`; return the pause sentinel (`runtime.ts:524`).
    /// Best-effort sandbox halt only — the durable status is the source of truth.
    Pause,
    /// Hard, model-actionable replay divergence (`runtime.ts:437/448`).
    Diverge(String),
    /// Oversize/unserializable args → terminal run failure (`runtime.ts:494`).
    Fail(String),
}

/// Neutral lifecycle status of a durable Code Mode run, read by the host after a
/// pass settles to decide the paused/completed/error envelope. Mirrors the
/// binary-side `RunStatus`; `Unknown` also covers a tampered (HMAC-failed) row
/// so callers fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLifecycle {
    Running,
    Paused,
    Completed,
    Error,
    Rejected,
    Expired,
    /// Missing run OR failed integrity verification — treat as fail-closed.
    Unknown,
}

/// Fields to begin a fresh durable run (`store.begin`). Neutral mirror of the
/// binary-side `NewRun` so the MCP surface can start a run through the decider
/// trait without depending on the SQLite store.
#[derive(Debug, Clone)]
pub struct BeginRun {
    pub execution_id: String,
    pub code_hash: String,
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub route_scope: String,
    pub capability_filter_fingerprint: String,
    pub expires_at_ms: i64,
}

/// A pending (awaiting-approval) call surfaced in a paused run's summary.
#[derive(Debug, Clone)]
pub struct PendingCall {
    pub seq: u64,
    pub tool_id: String,
}

/// Authorization fields recorded on a run, re-read at resume time to recompute
/// live authorization (V1/V3). Returned by `run_auth_fields`.
#[derive(Debug, Clone)]
pub struct RunAuthFields {
    pub code_hash: String,
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub capability_filter_fingerprint: String,
    /// HMAC integrity verification result — `false` ⇒ tampered, fail closed.
    pub verified: bool,
    pub status: RunLifecycle,
}

/// A boxed, `Send` future — the object-safe return type the `CodeModeDecider`
/// trait uses so it can be held as `Arc<dyn CodeModeDecider>`.
///
/// Native `async fn in trait` (RPITIT) is NOT `dyn`-compatible, and the repo
/// forbids the `async-trait` crate; boxing the future by hand keeps the trait
/// object-safe without pulling in `async_trait`. Implementations still write
/// `async` bodies internally and wrap them in `Box::pin(async move { … })`.
pub type BoxDecideFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The durable-execution decision layer the Code Mode host consults around each
/// upstream tool call. Storage-neutral: the concrete SQLite-backed
/// implementation (`SqliteDecider`) lives in the `labby` binary crate, injected
/// into the gateway host as `Arc<dyn CodeModeDecider>`.
///
/// Port of the `decide()`/`recordResult()` half of Cloudflare's
/// `CodemodeRuntime` (`runtime.ts:411-572`). Methods return boxed futures so the
/// trait is `dyn`-compatible (see [`BoxDecideFuture`]).
pub trait CodeModeDecider: Send + Sync {
    /// Decide what to do with the call at `(execution_id, seq)`. Ports
    /// `runtime.ts:411 decide`: monotonic pause gate first, then
    /// replay/divergence/journal-and-pause/execute.
    fn decide<'a>(
        &'a self,
        execution_id: &'a str,
        seq: u64,
        tool_id: &'a str,
        args: &'a Value,
        requires_approval: bool,
        ephemeral: bool,
    ) -> BoxDecideFuture<'a, DecideOutcome>;

    /// Record the real result of an executed call and mark it applied. Ports
    /// `runtime.ts:543 recordResult`.
    fn record_result<'a>(
        &'a self,
        execution_id: &'a str,
        seq: u64,
        result: &'a Value,
    ) -> BoxDecideFuture<'a, Result<(), ToolError>>;

    // ── Run lifecycle (driven by the MCP surface) ────────────────────────────

    /// Insert a fresh `running` run (port of `runtime.ts:355 begin`). Called on
    /// the pause-capable path before driving the snippet.
    fn begin(&self, run: BeginRun) -> BoxDecideFuture<'_, Result<(), ToolError>>;

    /// The durable status of a run after a pass settles (the C1 payoff): the
    /// host reads this — NOT the sandbox's own result — to decide the envelope.
    /// Returns `Unknown` for a missing/tampered run.
    fn run_status<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, RunLifecycle>;

    /// The recorded error message for an `Error` run (audit / envelope).
    fn run_error<'a>(
        &'a self,
        execution_id: &'a str,
    ) -> BoxDecideFuture<'a, Option<String>>;

    /// Pending (awaiting-approval) calls of a paused run (port of
    /// `runtime.ts:640 listPending`).
    fn list_pending<'a>(
        &'a self,
        execution_id: &'a str,
    ) -> BoxDecideFuture<'a, Vec<PendingCall>>;

    /// Mark a run terminal with the given lifecycle status + optional error.
    /// Used for `Completed` after settle and `Rejected` from the reject action.
    fn set_status<'a>(
        &'a self,
        execution_id: &'a str,
        to: RunLifecycle,
        error: Option<&'a str>,
    ) -> BoxDecideFuture<'a, Result<bool, ToolError>>;

    /// CAS a paused run `paused → running` (port of `runtime.ts:383 resume`).
    /// Returns `true` iff a paused row transitioned (loser gets `false`).
    fn resume_to_running<'a>(
        &'a self,
        execution_id: &'a str,
    ) -> BoxDecideFuture<'a, bool>;

    /// The recorded authorization fields for a run, for live re-authorization at
    /// resume time (V1/V3).
    fn run_auth_fields<'a>(
        &'a self,
        execution_id: &'a str,
    ) -> BoxDecideFuture<'a, Option<RunAuthFields>>;

    /// Lazy, throttled TTL expiry sweep (Wave 4 Task 4.2). Best-effort;
    /// no-op if the throttle interval has not elapsed.
    fn maybe_expire(&self) -> BoxDecideFuture<'_, ()>;
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
    /// `ctx` carries the durable-run `execution_id` + protocol `seq`; when the
    /// host has an injected decider AND `ctx.execution_id` is `Some`, the call
    /// runs through the decide→dispatch→record durable dance (pause/replay/
    /// diverge/fail), otherwise it dispatches directly (write-free path).
    fn call_tool(
        &self,
        id: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        ctx: ExecCtx<'_>,
    ) -> impl Future<Output = Result<ToolCallOutcome, ToolError>> + Send;

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
            entries: Vec::new(),
            catalog_json: "[]".to_string(),
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
        _ctx: ExecCtx<'_>,
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
}
