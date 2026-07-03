//! Code Mode runner subprocess entry point (`internal code-mode-runner`):
//! the in-process Javy/QuickJS stdio loop.

use std::io::{self, BufRead, BufReader, BufWriter};
use std::process::ExitCode;

use serde_json::Value;

use super::protocol::{
    CodeModeRunnerInput, CodeModeRunnerOutput, CodeModeRunnerState, RUNNER_STATE,
};
use super::runner_io::runner_emit_blocking;

pub fn run_code_mode_runner_stdio() -> ExitCode {
    // Security: prevent /proc/<pid>/environ readback of the runner process.
    // Must be the very first act — do this before any state is initialized.
    #[cfg(all(unix, target_os = "linux"))]
    {
        use nix::sys::prctl;
        if prctl::set_dumpable(false).is_err() {
            // Non-fatal — execution continues but /proc/<pid>/environ may be readable.
            // This runs inside the runner SUBPROCESS, which has no tracing
            // subscriber installed; tracing::warn! here would be dropped. The
            // parent drains this child's stderr into the response logs, so
            // eprintln! is the channel that actually surfaces the warning.
            #[allow(clippy::print_stderr)]
            {
                eprintln!(
                    "WARNING: prctl(PR_SET_DUMPABLE, 0) failed; runner environment may be readable via /proc"
                );
            }
        }
    }

    RUNNER_STATE.with(|state| {
        *state.borrow_mut() = Some(CodeModeRunnerState {
            reader: BufReader::new(io::stdin()),
            writer: BufWriter::new(io::stdout()),
            next_seq: 0,
        });
    });

    // Warm-runner pool (Perf H1): the runner process is long-lived and serves
    // one execution per `Start` message, building a FRESH Wasmtime Store and
    // generated JS instance per execution so no JS state (globals,
    // `__labPendingToolCalls`, captured data) can leak across callers. After
    // each execution the process parks on the next `read_line`. The parent pools
    // these processes to amortize the fork/plugin setup cost. EOF on the input
    // (parent closed stdin / dropped the handle) ends the loop with a clean
    // exit. A genuine per-execution failure is reported as an `Error` line; the
    // runner then continues to the next `Start` so a single bad snippet does not
    // poison a pooled process (the parent decides whether to recycle it).
    loop {
        match run_code_mode_runner() {
            Ok(RunnerLoopOutcome::Completed) => {
                // Reset per-execution state and park for the next Start.
                reset_runner_seq();
            }
            Ok(RunnerLoopOutcome::InputClosed) => {
                // Parent closed the pipe; shut the process down cleanly.
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                drop(runner_emit_blocking(CodeModeRunnerOutput::Error {
                    kind: err.kind,
                    message: err.message,
                    runner_unhealthy: err.runner_unhealthy,
                }));
                // Reset and continue: the per-execution javy runtime is dropped
                // at the end of `run_code_mode_runner`, so a failed execution
                // leaves no JS state behind. Whether to reuse or recycle this
                // process is the parent pool's decision.
                reset_runner_seq();
            }
        }
    }
}

/// Why the per-execution loop body returned.
enum RunnerLoopOutcome {
    /// An execution ran to a `Done` and the runner is ready for the next Start.
    Completed,
    /// The input stream reached EOF before a Start arrived; the process should
    /// exit cleanly (the parent dropped this pooled runner).
    InputClosed,
}

/// Reset the per-execution sequence counter so the next pooled execution starts
/// from `seq = 0`, matching the spawn-per-execution contract. The javy runtime
/// (and all JS globals) is constructed fresh inside `run_code_mode_runner`, so
/// this is the only thread-local carried across executions that needs clearing.
fn reset_runner_seq() {
    RUNNER_STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.next_seq = 0;
        }
    });
}

thread_local! {
    /// Stable base directory the per-execution jails live under — the runner's
    /// spawn cwd (the per-runner `TempDir` the parent set). Captured lazily on
    /// the first execution so each new jail is anchored here, never nested inside
    /// the previous execution's jail.
    static JAIL_BASE: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
    /// The current per-execution jail subdir, so the next execution can remove
    /// it before creating a fresh one. `None` until the first execution.
    static EXECUTION_JAIL: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Create a fresh empty per-execution working directory and `chdir` into it,
/// removing the previous execution's directory first. Best-effort: on any
/// failure the process is left in a still-valid isolated cwd — the prior jail if
/// it was never touched, otherwise the stable spawn base (the per-runner
/// `TempDir`), since the prior jail is removed up front. See the call site for
/// why this is defense-in-depth rather than a hard containment boundary.
fn reset_execution_jail() {
    // Resolve (and remember) the stable base = the spawn cwd. The first call
    // captures it before we ever chdir into a subdir, so subsequent jails are
    // siblings, not nested.
    let base = JAIL_BASE.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            *cell = std::env::current_dir().ok();
        }
        cell.clone()
    });
    let Some(base) = base else {
        return;
    };

    EXECUTION_JAIL.with(|cell| {
        let mut cell = cell.borrow_mut();
        // Remove the previous execution's jail (if any) so no file state from a
        // prior caller survives on this pooled process.
        if let Some(previous) = cell.take() {
            drop(std::fs::remove_dir_all(&previous));
        }
        let unique = format!("exec-{}-{}", std::process::id(), next_jail_seq());
        let jail = base.join(unique);
        if std::fs::create_dir(&jail).is_err() {
            // We already removed the previous jail above, so the process must not
            // be left `chdir`'d inside a now-deleted directory. Fall back to the
            // stable base (the per-runner spawn TempDir) so cwd stays valid and
            // isolated. `*cell` is already `None` (taken above).
            drop(std::env::set_current_dir(&base));
            return;
        }
        if std::env::set_current_dir(&jail).is_ok() {
            *cell = Some(jail);
        } else {
            // Could not enter the jail; clean it up and fall back to the stable
            // base rather than the just-removed previous jail.
            drop(std::fs::remove_dir_all(&jail));
            drop(std::env::set_current_dir(&base));
        }
    });
}

fn next_jail_seq() -> u64 {
    thread_local! {
        static JAIL_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    JAIL_SEQ.with(|seq| {
        let next = seq.get();
        seq.set(next.saturating_add(1));
        next
    })
}

/// Runner failure with an explicit error kind so the contract distinguishes a
/// caller mistake (`invalid_param`: malformed JS that fails to parse/eval, or a
/// non-JSON-serializable result) from a genuine backend fault (`server_error`).
///
/// `From<String>` defaults to `server_error`, so every existing
/// `map_err(|e| e.to_string())?` site keeps the previous behavior. The eval site
/// and the main-promise rejection classifier override the kind explicitly.
struct CodeModeRunnerError {
    kind: String,
    message: String,
    runner_unhealthy: bool,
}

impl From<String> for CodeModeRunnerError {
    fn from(message: String) -> Self {
        Self {
            kind: "server_error".to_string(),
            message,
            runner_unhealthy: false,
        }
    }
}

impl From<crate::error::ToolError> for CodeModeRunnerError {
    fn from(error: crate::error::ToolError) -> Self {
        let runner_unhealthy = error.kind() == "timeout"
            && error
                .to_string()
                .contains(crate::wasm_runner::CODE_MODE_WASM_CODEGEN_TIMEOUT_MESSAGE);
        Self {
            kind: error.kind().to_string(),
            message: error.to_string(),
            runner_unhealthy,
        }
    }
}

/// Extract a structured `{kind,message}` payload embedded in a rejection message
/// and return its `kind`, scanning the ENTIRE message rather than the first line.
///
/// The `__labSettleToolCall` bridge rejects a failed `callTool` with an `Error`
/// whose `.message` is the *pure JSON* `{kind,message}` of the tool-call error.
/// That pure-JSON shape is a load-bearing contract: caller JS recovers the
/// structured error via `JSON.parse(e.message)` (see the runner integration
/// tests), so the bridge must NOT wrap the message in markers or prose. QuickJS
/// then surfaces an uncaught rejection to the host as `Error: <message>`,
/// optionally followed by a `\n    at ...` stack trace. Rather than depend on
/// that exact prefix/first-line shape, locate the embedded JSON object (first
/// `{` to last `}`) and parse it: `JSON.stringify` escapes any newline inside
/// `message`, so the object stays single-line and a multi-line tool message no
/// longer perturbs recovery, and QuickJS stack frames carry no braces so a
/// trailing stack is ignored. A non-JSON span (e.g. `Error: x is not a
/// function`) fails the parse and falls through to the generic classification.
fn extract_structured_kind(message: &str) -> Option<String> {
    let start = message.find('{')?;
    let end = message.rfind('}')?;
    if end < start {
        return None;
    }
    let json_candidate = &message[start..=end];
    let Value::Object(map) = serde_json::from_str::<Value>(json_candidate).ok()? else {
        return None;
    };
    map.get("kind").and_then(Value::as_str).map(str::to_string)
}

/// Classify a main-promise rejection message into an error kind:
/// 1. If the message carries an embedded structured `{kind,message}` JSON object,
///    preserve that kind (structured tool-error rejections re-raised through the
///    sandbox). See `extract_structured_kind`.
/// 2. Else if it mentions `JSON-serializable`, the result could not be
///    serialized — a caller mistake → `invalid_param`.
/// 3. Otherwise it is a runtime throw (e.g. the non-function TypeError) →
///    `server_error`.
///
/// Note: a caller can deliberately set its own execution's error kind by throwing
/// a structured `{kind,message}` Error (intentional, see the
/// `..._preserves_kind_from_uncaught_structured_rejection` integration test). The
/// extracted kind is the caller's OWN result, not a cross-trust signal, so this
/// is by design rather than a forgery boundary.
fn classify_code_mode_rejection(message: String) -> CodeModeRunnerError {
    if let Some(kind) = extract_structured_kind(&message) {
        return CodeModeRunnerError {
            kind,
            message,
            runner_unhealthy: false,
        };
    }
    if message.contains("JSON-serializable") {
        return CodeModeRunnerError {
            kind: "invalid_param".to_string(),
            message,
            runner_unhealthy: false,
        };
    }
    let message = add_code_mode_hint("server_error", &message);
    CodeModeRunnerError {
        kind: "server_error".to_string(),
        message,
        runner_unhealthy: false,
    }
}

pub(crate) fn classify_code_mode_rejection_tool_error(message: String) -> crate::error::ToolError {
    let error = classify_code_mode_rejection(message);
    crate::error::ToolError::Sdk {
        sdk_kind: error.kind,
        message: error.message,
    }
}

fn add_code_mode_hint(kind: &str, message: &str) -> String {
    let mut hints = Vec::new();
    if kind == "ReferenceError"
        || message.contains(" is not defined")
        || message.contains("not defined")
    {
        hints.push(
            "Available globals: codemode, codemode.run, codemode.search, codemode.describe, codemode.step, callTool, writeArtifact. Node/Deno globals such as require, process, fs, fetch, and Bun are not available in the sandbox.",
        );
    }
    if (message.contains(" is not a function") || message.contains("not a function"))
        && message.contains("codemode")
    {
        hints.push(
            "Use await codemode.search(\"...\") or await codemode.describe(\"...\") to find the exact helper name.",
        );
    }
    if hints.is_empty() {
        message.to_string()
    } else {
        format!("{message}\n\nHint: {}", hints.join(" "))
    }
}

fn run_code_mode_runner() -> Result<RunnerLoopOutcome, CodeModeRunnerError> {
    // Read the next Start. EOF here is the normal pool-shutdown path (the parent
    // dropped this runner), NOT an error — return InputClosed so the caller can
    // exit cleanly without emitting a spurious `Error` line.
    let input = match runner_read_input() {
        Ok(input) => input,
        Err(RunnerReadError::InputClosed) => return Ok(RunnerLoopOutcome::InputClosed),
        Err(RunnerReadError::Other(message)) => return Err(message.into()),
    };
    let CodeModeRunnerInput::Start {
        code,
        proxy,
        timeout_ms,
    } = input
    else {
        return Err("runner expected start message".to_string().into());
    };
    let timeout = timeout_ms
        .map(std::time::Duration::from_millis)
        .filter(|timeout| !timeout.is_zero())
        .unwrap_or_else(|| std::time::Duration::from_secs(30));

    // Per-execution cwd jail (Perf H1 isolation): a pooled runner is long-lived,
    // so its process cwd must not accumulate state across executions. Create a
    // fresh empty subdir under the runner's spawn cwd and chdir into it, after
    // removing the previous execution's subdir. The JS sandbox exposes no fs
    // APIs, so this is defense-in-depth — it guarantees that even a future
    // host-side artifact path bug cannot let one execution observe a prior one's
    // working-directory contents on the same pooled process. Failure is
    // non-fatal: the spawn cwd is already an isolated TempDir.
    reset_execution_jail();

    thread_local! {
        static WASM_RUNNER: std::cell::RefCell<Option<crate::wasm_runner::WasmRunner>> =
            const { std::cell::RefCell::new(None) };
    }

    let result = WASM_RUNNER.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            *cell =
                Some(
                    crate::wasm_runner::WasmRunner::new().map_err(|err| CodeModeRunnerError {
                        kind: "server_error".to_string(),
                        message: format!("failed to initialize Code Mode Wasm runner: {err}"),
                        runner_unhealthy: false,
                    })?,
                );
        }
        cell.as_mut()
            .expect("runner initialized above")
            .execute(&code, &proxy, timeout)
            .map_err(CodeModeRunnerError::from)
    })?;

    runner_emit_blocking(CodeModeRunnerOutput::Done {
        result,
        logs: Vec::new(),
    })
    .map_err(CodeModeRunnerError::from)?;
    Ok(RunnerLoopOutcome::Completed)
}

/// Distinguishes a clean end-of-input (EOF on stdin — the parent dropped this
/// pooled runner) from any other read failure. The pool loop treats `InputClosed`
/// as a normal shutdown signal and exits cleanly without emitting an `Error` line.
enum RunnerReadError {
    /// EOF: the parent closed/dropped the input stream.
    InputClosed,
    /// Any other failure (I/O error, malformed protocol JSON, uninitialized state).
    Other(String),
}

fn runner_read_input() -> Result<CodeModeRunnerInput, RunnerReadError> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| RunnerReadError::Other("runner state is not initialized".to_string()))?;
        let mut line = String::new();
        let read = state
            .reader
            .read_line(&mut line)
            .map_err(|err| RunnerReadError::Other(err.to_string()))?;
        if read == 0 {
            return Err(RunnerReadError::InputClosed);
        }
        serde_json::from_str(&line).map_err(|err| RunnerReadError::Other(err.to_string()))
    })
}
