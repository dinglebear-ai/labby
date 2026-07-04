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

/// Why a [`DecideOutcome::Fail`] terminated the run. Preserved across the decide
/// boundary so the host can map each cause onto the correct `sdk_kind` instead of
/// collapsing every failure into `internal_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    /// No durable run exists for this execution id.
    UnknownExecution,
    /// The run row failed HMAC integrity verification (tampered) — fail closed.
    IntegrityFailure,
    /// A journaled value exceeded the durable size cap. Caller-shaped
    /// (`invalid_param`), not an internal fault — the args/result are too big to
    /// record and must be reduced.
    ValueTooLarge,
    /// A genuine internal fault (SQLite/serialization/status-write failure).
    Internal,
}

impl FailReason {
    /// The host `sdk_kind` this failure maps onto. `ValueTooLarge` is
    /// caller-shaped (`invalid_param`); every other cause is `internal_error`.
    #[must_use]
    pub fn sdk_kind(self) -> &'static str {
        match self {
            FailReason::ValueTooLarge => "invalid_param",
            FailReason::UnknownExecution | FailReason::IntegrityFailure | FailReason::Internal => {
                "internal_error"
            }
        }
    }
}

/// Whether a call needs per-call human approval before it may dispatch. A named
/// two-variant enum replacing a bare `bool` at the [`CodeModeDecider::decide`]
/// boundary, so it cannot be transposed with [`Journaling`] (both were adjacent
/// `bool`s). A fresh `Required` call journals `pending` and pauses the run; a
/// `NotNeeded` call executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// The call is destructive/unconfirmed — journal pending and pause.
    Required,
    /// The call may execute without per-call approval.
    NotNeeded,
}

impl Approval {
    /// `true` iff approval is required (the wire/store bool).
    #[must_use]
    pub fn is_required(self) -> bool {
        matches!(self, Approval::Required)
    }
}

/// Whether a journaled entry replays its recorded result or re-executes on
/// resume. A named two-variant enum replacing a bare `bool` at the
/// [`CodeModeDecider::decide`] boundary (adjacent to [`Approval`], hence the
/// transposition risk this removes). `Ephemeral` entries (local `state`/`git`
/// providers) RE-EXECUTE on replay so the side effect re-runs; `Durable` entries
/// replay their stored result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Journaling {
    /// Re-execute on replay (never serve a stored result). Local FS/git calls.
    Ephemeral,
    /// Replay the stored result on resume (journal-once). Normal tool calls/steps.
    Durable,
}

impl Journaling {
    /// `true` iff the entry is ephemeral (the wire/store bool).
    #[must_use]
    pub fn is_ephemeral(self) -> bool {
        matches!(self, Journaling::Ephemeral)
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
    /// Terminal run failure. `reason` distinguishes the cause (so the host can
    /// pick the right `sdk_kind`); `message` is the human-facing detail.
    Fail { reason: FailReason, message: String },
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
/// live authorization (V1/V3).
///
/// A `VerifiedAuth` can ONLY be produced from a row that passed HMAC integrity
/// verification — it carries no `verified` flag because its mere existence is
/// the proof. It is reachable exclusively through [`AuthLoad::Ok`], so a caller
/// can never read `is_admin`/`route_scope`/`actor_key` off a tampered row: the
/// tampered case is a distinct [`AuthLoad::Tampered`] variant with no fields.
#[derive(Debug, Clone)]
pub struct VerifiedAuth {
    pub code_hash: String,
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub capability_filter_fingerprint: String,
    /// The route scope label the run was started under. Re-checked at resume
    /// time (F2) so a run paused under route A cannot be resumed under route B
    /// even when the caller shares the same capability fingerprint/actor.
    pub route_scope: String,
    pub status: RunLifecycle,
}

/// The outcome of loading a run's authorization fields via
/// [`CodeModeDecider::run_auth_fields`].
///
/// This models the three states a security gate MUST handle explicitly, so that
/// tampered auth fields can never be read as usable. There is no `verified: bool`
/// to forget: only [`AuthLoad::Ok`] yields a [`VerifiedAuth`], and it is
/// unreachable for a missing or HMAC-failed row.
#[derive(Debug, Clone)]
pub enum AuthLoad {
    /// No run exists for this execution id.
    Missing,
    /// A run row exists but failed HMAC integrity verification — fail closed.
    Tampered,
    /// A run row exists and passed integrity verification.
    Ok(VerifiedAuth),
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
///
/// # Fail-closed contract
///
/// Every method fails closed — an ambiguous, missing, or tampered run must never
/// be readable as a usable/authorized state:
///
/// - [`run_status`](Self::run_status) returns [`RunLifecycle::Unknown`] for a
///   missing OR HMAC-tampered run; `Unknown` is never `Running`, so no work
///   proceeds.
/// - [`run_auth_fields`](Self::run_auth_fields) returns [`AuthLoad::Ok`] ONLY
///   for a row that passed integrity verification; a missing row is
///   [`AuthLoad::Missing`] and a tampered row is [`AuthLoad::Tampered`], neither
///   of which exposes a [`VerifiedAuth`].
/// - CAS transitions ([`resume_to_running`](Self::resume_to_running),
///   [`reject_paused`](Self::reject_paused)) return `false` when no eligible row
///   transitioned (already resumed/terminal, missing, or tampered) — the loser
///   of a race or a tampered row is refused, never force-applied.
/// - [`decide`](Self::decide) returns [`DecideOutcome::Fail`] with a
///   [`FailReason`] of `UnknownExecution`/`IntegrityFailure` for a
///   missing/tampered run and never treats a tampered row's recorded fields as
///   trustworthy.
///
/// Uniformly: `None`/`Unknown`/`false`/`Tampered` mean "refuse", never "allow".
pub trait CodeModeDecider: Send + Sync {
    /// Decide what to do with the call at `(execution_id, seq)`. Ports
    /// `runtime.ts:411 decide`: monotonic pause gate first, then
    /// replay/divergence/journal-and-pause/execute.
    ///
    /// `approval` and `journaling` are named enums (not adjacent `bool`s) so the
    /// two cannot be transposed at a call site.
    fn decide<'a>(
        &'a self,
        execution_id: &'a str,
        seq: u64,
        tool_id: &'a str,
        args: &'a Value,
        approval: Approval,
        journaling: Journaling,
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
    fn run_error<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, Option<String>>;

    /// Pending (awaiting-approval) calls of a paused run (port of
    /// `runtime.ts:640 listPending`).
    fn list_pending<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, Vec<PendingCall>>;

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
    fn resume_to_running<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, bool>;

    /// Reject a run, guarded so it only fires on a still-`paused` run (port of
    /// `runtime.ts:668 reject`). Unlike `set_status`, this cannot force-terminate
    /// a live `running` run: the transition is conditional on the run observing
    /// `status='paused'`. Returns `true` iff a paused row transitioned.
    fn reject_paused<'a>(
        &'a self,
        execution_id: &'a str,
        error: Option<&'a str>,
    ) -> BoxDecideFuture<'a, Result<bool, ToolError>>;

    /// The recorded authorization fields for a run, for live re-authorization at
    /// resume time (V1/V3).
    ///
    /// Returns [`AuthLoad::Ok`] ONLY for a row that passed HMAC integrity
    /// verification; a missing row is [`AuthLoad::Missing`] and a tampered
    /// (HMAC-failed) row is [`AuthLoad::Tampered`]. A [`VerifiedAuth`] is thus
    /// reachable exclusively down the `Ok` path, so no caller can trust the
    /// auth-bearing fields of an unverified row.
    fn run_auth_fields<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, AuthLoad>;

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

    /// Decide replay-vs-execute for a `codemode.step(name, fn)` boundary at
    /// `(execution_id, seq)`, BEFORE the sandbox runs `fn`. The step consumes a
    /// `seq` from the same monotonic spine as `call_tool`, so it participates in
    /// the durable replay cursor.
    ///
    /// The default impl always returns [`StepDecision::Execute`] — the
    /// no-decider / standalone / write-free path (CLI, pre-confirmed, tests)
    /// simply runs `fn` normally, exactly as before this primitive existed.
    fn decide_step(
        &self,
        ctx: ExecCtx<'_>,
        name: &str,
    ) -> impl Future<Output = StepDecision> + Send {
        let _ = (ctx, name);
        async { StepDecision::Execute }
    }

    /// Record the value a step's `fn` produced (decision was execute) so a later
    /// resume replays it without re-running `fn`.
    ///
    /// The default impl is a no-op `Ok(())` — the write-free path records
    /// nothing, so `fn` is simply re-run on any (non-durable) re-execution.
    fn record_step(
        &self,
        ctx: ExecCtx<'_>,
        value: &Value,
    ) -> impl Future<Output = Result<(), ToolError>> + Send {
        let _ = (ctx, value);
        async { Ok(()) }
    }

    /// Decide replay-vs-execute for a runner-reserved LOCAL provider call
    /// (`state::*` / `git::*`) at `(execution_id, seq)`, journaling it as an
    /// **ephemeral** durable entry.
    ///
    /// Local providers dispatch inside the runner crate (not via `call_tool`),
    /// so before this hook they were invisible to the durable log: on the
    /// pause-capable path a resumed run would silently re-apply them out of the
    /// `seq` spine (the fail-closed exclusion that used to make such runs
    /// non-pausable). Journaling them ephemerally keeps the `seq` spine aligned
    /// and enforces the tool_id/args divergence check, while `ephemeral = true`
    /// means a recorded entry RE-EXECUTES on replay (the local FS/git side
    /// effect is re-run deterministically-enough, never replayed from a stale
    /// stored result).
    ///
    /// `id` is the reserved `<namespace>::<method>` id; `params` are the call
    /// args (the divergence key). The default impl returns [`StepDecision::Execute`]
    /// (no-decider / write-free path) so local calls dispatch unchanged.
    fn decide_local(
        &self,
        ctx: ExecCtx<'_>,
        id: &str,
        params: &Value,
    ) -> impl Future<Output = StepDecision> + Send {
        let _ = (ctx, id, params);
        async { StepDecision::Execute }
    }

    /// Record that a local-provider call was applied (marks the ephemeral entry
    /// `applied`). Because the entry is ephemeral it re-executes on replay
    /// regardless, so the recorded value is a marker, not a replay source. The
    /// default impl is a no-op `Ok(())` for the write-free path.
    fn record_local(
        &self,
        ctx: ExecCtx<'_>,
        value: &Value,
    ) -> impl Future<Output = Result<(), ToolError>> + Send {
        let _ = (ctx, value);
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

    /// A host with NO decider (NoopHost does not override the journaling hooks)
    /// uses the trait DEFAULT impls: `decide_step`/`decide_local` return
    /// `Execute` and `record_step`/`record_local` are no-op `Ok(())`. This is
    /// the write-free / standalone path — `codemode.step`'s `fn` and local
    /// provider calls run normally without any durable journaling, exactly as
    /// before the primitive existed.
    #[tokio::test]
    async fn default_step_and_local_hooks_execute_and_noop() {
        let host = NoopHost::default();
        let ctx = ExecCtx::none();
        assert!(matches!(
            host.decide_step(ctx, "s").await,
            StepDecision::Execute
        ));
        assert!(host.record_step(ctx, &Value::Null).await.is_ok());
        assert!(matches!(
            host.decide_local(ctx, "state::writeFile", &Value::Null)
                .await,
            StepDecision::Execute
        ));
        assert!(host.record_local(ctx, &Value::Null).await.is_ok());
    }
}
