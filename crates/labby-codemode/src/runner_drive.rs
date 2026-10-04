//! `CodeModeBroker::run_in_runner`: spawn the runner subprocess and drive the
//! tool-call/log/completion protocol loop.
//!
//! The public entry point is `run_in_runner`, which packs runtime parameters
//! into a `RunnerConfig` struct and delegates to `run_in_runner_with_config`.
//! Each major event arm (`Done`, `ToolCall`, `ArtifactWrite`,
//! `SnippetResolve`, `Error`) is handled by a named async helper to keep the
//! select loop readable.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use futures::{StreamExt, stream::FuturesUnordered};
use labby_primitives::trace::TraceContext;
use serde_json::{Value, json};
use tokio::process::ChildStdin;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use crate::CodeModeCallError;
use crate::error::ToolError;
use crate::git::provider::dispatch_git_method;
use crate::host::{CodeModeHost, ExecCtx, StepDecision, ToolCallOutcome};
use crate::local_provider::{LocalProviderCall, LocalProviderName};
use crate::state::provider::dispatch_state_method;
use crate::state::quota::StateWorkspaceLimits;
use crate::state::workspace::StateWorkspace;

use super::CodeModeBroker;
use super::artifacts::{
    ActiveArtifactRun, CodeModeArtifactReceipt, CodeModeArtifactWrite, code_mode_artifact_root,
    write_code_mode_artifact,
};
use super::config::{
    MAX_INTERNAL_CALLS_PER_RUN, MAX_SNIPPET_RESOLVED_BYTES_PER_RUN, MAX_SNIPPET_RESOLVES_PER_RUN,
    calltool_result_max_bytes, max_calltool_per_run,
};
use super::pool::RunnerPool;
use super::pool::runner_handle::PooledRunner;
use super::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput};
use super::runner_io::{terminate_code_mode_runner, write_runner_input};
use super::truncate::apply_log_caps;
use super::types::{
    CodeModeCaller, CodeModeExecutedCall, CodeModeExecutionError, CodeModeExecutionResponse,
    CodeModeSurface, ToolScope,
};

mod artifacts;
mod finalize;
mod steps;
use artifacts::{handle_artifact_write_event, handle_snippet_resolve_event};
use finalize::{code_mode_timeout_error, finalize_done, sorted_calls};
use steps::{handle_step_begin_event, handle_step_result_event};

static LOCAL_PROVIDER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static RESULT_ACK_RESERVE_USES: AtomicU64 = AtomicU64::new(0);
static SETTLEMENT_WATCHDOG_EXPIRIES: AtomicU64 = AtomicU64::new(0);

#[cfg(all(test, not(windows)))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CodeModeRuntimeCounters {
    pub result_ack_reserve_uses: u64,
    pub settlement_watchdog_expiries: u64,
}

#[cfg(all(test, not(windows)))]
#[must_use]
pub(crate) fn code_mode_runtime_counters() -> CodeModeRuntimeCounters {
    CodeModeRuntimeCounters {
        result_ack_reserve_uses: RESULT_ACK_RESERVE_USES.load(Ordering::Relaxed),
        settlement_watchdog_expiries: SETTLEMENT_WATCHDOG_EXPIRIES.load(Ordering::Relaxed),
    }
}

fn increment_saturating_counter(counter: &AtomicU64) -> u64 {
    match counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    }) {
        Ok(previous) | Err(previous) => previous.saturating_add(1),
    }
}

fn record_result_ack_reserve_use() -> u64 {
    increment_saturating_counter(&RESULT_ACK_RESERVE_USES)
}

fn record_settlement_watchdog_expiry() -> u64 {
    increment_saturating_counter(&SETTLEMENT_WATCHDOG_EXPIRIES)
}

const ARTIFACT_WRITE_CALL_ID: &str = "code_mode::write_artifact";
/// Once every pending external tool call has settled, the sandbox has no timers
/// or other external I/O that justifies consuming the remainder of the full
/// execution timeout. Give promise/microtask settlement a brief grace period,
/// then evict a runner that never emits Done/Error.
const RUNNER_SETTLEMENT_GRACE: Duration = Duration::from_secs(5);
/// Normal ToolResult/ToolError -> Done/Error acknowledgement budget reserved
/// inside the execution deadline. Keep this much smaller than the 5-second
/// hung-runner watchdog: it protects the control-plane handshake without
/// materially shortening legitimate upstream tool execution.
const RUNNER_RESULT_ACK_RESERVE: Duration = Duration::from_millis(250);
/// Extra acknowledgement budget per call the handshake may have to drain.
///
/// The reserve covers writing one ToolResult/ToolError per in-flight call and
/// reading Done/Error back. That work grows with fanout, so a constant budget
/// silently shrinks to microseconds per ack at high fanout (0.5 ms/ack at the
/// 512-call default) and turns a completed run into a timeout that discards
/// every real result.
const RUNNER_RESULT_ACK_PER_CALL: Duration = Duration::from_millis(2);
/// Ceiling for the scaled reserve, kept well under [`RUNNER_SETTLEMENT_GRACE`].
const RUNNER_RESULT_ACK_RESERVE_MAX: Duration = Duration::from_secs(2);

/// Acknowledgement budget for `in_flight` outstanding calls.
fn result_ack_reserve(in_flight: u64) -> Duration {
    let per_call =
        RUNNER_RESULT_ACK_PER_CALL.saturating_mul(u32::try_from(in_flight).unwrap_or(u32::MAX));
    RUNNER_RESULT_ACK_RESERVE
        .saturating_add(per_call)
        .min(RUNNER_RESULT_ACK_RESERVE_MAX)
}

#[derive(Clone, Copy)]
struct SettlementWatch {
    deadline: tokio::time::Instant,
    /// True when the dedicated settlement grace expires before the overall
    /// execution deadline. False means the outer execution deadline is the
    /// actual limiter and must surface as an ordinary Code Mode timeout.
    grace_limited: bool,
}

impl SettlementWatch {
    fn new(now: tokio::time::Instant, execution_deadline: tokio::time::Instant) -> Self {
        let grace_deadline = now + RUNNER_SETTLEMENT_GRACE;
        Self {
            deadline: grace_deadline.min(execution_deadline),
            grace_limited: grace_deadline < execution_deadline,
        }
    }
}

/// Deadline exposed to an external tool call. Reserve a small control-plane
/// acknowledgement window inside the same wall-clock execution budget so a tool
/// that runs right up to the outer deadline cannot leave the host with no time
/// to deliver its ToolResult/ToolError and receive Done/Error. This is
/// deliberately much smaller than `RUNNER_SETTLEMENT_GRACE`: the latter is a
/// hung-runner diagnostic watchdog, not time that should be taken away from every
/// healthy upstream call. Very short executions that cannot leave at least one
/// equal-sized tool slice keep their original deadline.
fn external_tool_deadline(
    now: tokio::time::Instant,
    execution_deadline: tokio::time::Instant,
    in_flight: u64,
) -> tokio::time::Instant {
    let remaining = execution_deadline
        .checked_duration_since(now)
        .unwrap_or_default();
    let reserve = result_ack_reserve(in_flight);
    if remaining > reserve.saturating_mul(2) {
        execution_deadline - reserve
    } else {
        execution_deadline
    }
}

struct CancelExecutionOnDrop(CancellationToken);

impl Drop for CancelExecutionOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

// Concrete future type for pending tool calls.
// Using Pin<Box<dyn Future>> keeps the FuturesUnordered type concrete so the
// compiler can infer the element type at the declaration site without requiring
// `impl Future` in a non-`async fn` parameter position (which is unsupported).
type ToolCallFut<'a> = std::pin::Pin<
    Box<
        dyn Future<
                Output = (
                    u64,
                    String,
                    Option<Value>,
                    Result<ToolCallOutcome, CodeModeCallError>,
                    u128,
                    u128,
                ),
            > + Send
            + 'a,
    >,
>;

// ---------------------------------------------------------------------------
// RunnerConfig — collects the 10 positional parameters into one struct
// ---------------------------------------------------------------------------

/// All configuration for a single `run_in_runner` invocation.
///
/// Collecting these into a struct eliminates the 10-positional-argument call
/// site (clippy `too_many_arguments`) and makes each field self-documenting.
pub(crate) struct RunnerConfig {
    pub code_to_run: String,
    pub proxy: String,
    pub timeout: Duration,
    pub caller: CodeModeCaller,
    pub surface: CodeModeSurface,
    pub max_log_entries: usize,
    pub max_log_bytes: usize,
    pub trace_params: bool,
    pub capability_filter: ToolScope,
    /// Effective total byte budget for source resolved through `codemode.run`.
    pub snippet_max_bytes: usize,
    /// Durable-run execution id, minted by the caller (binary/gateway). `None`
    /// on the write-free/standalone path; flows into every [`ExecCtx`] so the
    /// host's `record_step` can key its per-execution journal buffer.
    pub execution_id: Option<Arc<str>>,
    /// Request-owned trace context inherited from the outer MCP call.
    pub trace_context: Option<Arc<TraceContext>>,
    /// Loaded OpenAPI specs for the `openapi` local provider (cheap `Arc` clone).
    pub openapi_registry: labby_openapi::OpenApiRegistry,
    /// Hardened dispatch client for the `openapi` provider (cheap `Arc` clone).
    pub openapi_http_client: reqwest::Client,
}

// ---------------------------------------------------------------------------
// Drive state — per-run mutable bookkeeping (excludes pending_tool_calls,
// which stays local in run_in_runner_with_config so its lifetime is tied to
// the enclosing async fn and not forced to 'static)
// ---------------------------------------------------------------------------

struct DriveState {
    calls: Vec<(u64, CodeModeExecutedCall)>,
    artifacts: Vec<CodeModeArtifactReceipt>,
    artifact_max_bytes: usize,
    artifact_root: PathBuf,
    snippet_resolves: usize,
    snippet_resolved_bytes: usize,
    calls_enqueued: u64,
    max_calls_per_run: u64,
    /// Reserved `__lab_internal::*` pseudo-tool calls seen this run. Metered
    /// separately from `calls_enqueued` (internal calls never consume the
    /// ordinary budget) against `MAX_INTERNAL_CALLS_PER_RUN`.
    internal_calls_enqueued: usize,
    calltool_result_max_bytes: usize,
    /// Whether this run has already recorded that its external tool deadline
    /// reserved the result-acknowledgement control-plane slice. The counter is
    /// per execution, not per tool call, so fan-out does not inflate it.
    result_ack_reserve_observed: bool,
    /// Monotonic count of `step_begin` events seen so far (the journal ordinate).
    next_step_ordinal: u64,
    /// Maps a step's runner `seq` -> (step_ordinal, name), populated at
    /// step_begin and read at step_result (which reuses the step_begin seq).
    step_ordinals: std::collections::HashMap<u64, (u64, String)>,
    /// Aggregate serialized bytes accepted from `step_result` values.
    step_value_bytes: usize,
}

impl DriveState {
    fn new(artifact_run_id: &str) -> Self {
        let artifact_root = code_mode_artifact_root(artifact_run_id);
        let artifact_max_bytes = super::artifacts::artifact_max_bytes();
        Self {
            calls: Vec::new(),
            artifacts: Vec::new(),
            artifact_max_bytes,
            artifact_root,
            snippet_resolves: 0,
            snippet_resolved_bytes: 0,
            calls_enqueued: 0,
            max_calls_per_run: max_calltool_per_run(),
            internal_calls_enqueued: 0,
            calltool_result_max_bytes: calltool_result_max_bytes(),
            result_ack_reserve_observed: false,
            next_step_ordinal: 0,
            step_ordinals: std::collections::HashMap::new(),
            step_value_bytes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Classification of a single drive: the result plus whether the runner is safe
/// to return to the pool.
enum DriveOutcome {
    /// Clean `Done` — return the response and keep (park) the runner.
    Completed(CodeModeExecutionResponse),
    /// The runner reported a per-execution `Error` and then parked itself; the
    /// process is healthy and may be reused.
    ExecutionError(CodeModeExecutionError),
    /// The runner exited before emitting any valid protocol event. The runner
    /// must be replaced, and the execution may be replayed once because no
    /// host-visible side effect could have crossed the protocol boundary.
    RunnerUnavailableBeforeActivity(CodeModeExecutionError),
    /// The runner crashed, timed out, or violated the protocol after execution
    /// began; it must be killed and replaced without replaying the run.
    RunnerUnhealthy(CodeModeExecutionError),
}

/// Decode a framed-line read result into either the line text or a structured
/// I/O / protocol-violation error.
fn classify_line_result(
    line_result: Result<String, tokio_util::codec::LinesCodecError>,
) -> Result<String, ToolError> {
    line_result.map_err(|err| {
        use tokio_util::codec::LinesCodecError;
        let max = super::pool::runner_handle::MAX_LINE_BYTES;
        let (sdk_kind, message) = match &err {
            LinesCodecError::MaxLineLengthExceeded => (
                "internal_error",
                format!(
                    "Code Mode runner emitted a protocol line exceeding the \
                     {max}-byte safety cap; possible unbounded output"
                ),
            ),
            LinesCodecError::Io(io_err) => (
                "internal_error",
                format!("failed to read Code Mode runner output: {io_err}"),
            ),
        };
        ToolError::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message,
        }
    })
}

mod calls;
mod drive;
mod lifecycle;
mod responses;
use calls::*;
use responses::*;

#[cfg(test)]
mod tests;

#[cfg(all(test, not(windows)))]
mod cancellation_tests;
