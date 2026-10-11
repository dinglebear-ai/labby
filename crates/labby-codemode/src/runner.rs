//! Code Mode runner subprocess entry point (`internal code-mode-runner`):
//! the in-process Javy/QuickJS stdio loop.

use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;

use serde_json::Value;

use crate::CodeModeCallError;

mod bridge;
use bridge::*;

use super::config::MAX_SNIPPET_RESOLVED_BYTES_PER_RUN;
use super::protocol::CODE_MODE_STACK_SIZE_LIMIT;
use super::protocol::{
    CodeModeRunnerInput, CodeModeRunnerOutput, CodeModeRunnerResult, CodeModeRunnerState,
    RUNNER_STATE,
};
use super::wrapper::{CODE_MODE_VALUE_CODEC_JS, code_mode_main_invoker};

/// Run the long-lived Code Mode runner protocol loop over process stdio.
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
    // one execution per `Start` message, building a FRESH `javy::Runtime` per
    // execution so no JS state (globals, `__labPendingToolCalls`, captured data)
    // can leak across callers. After each execution the process parks on the
    // next `read_line`. The parent pools these processes to amortize the fork
    // cost. EOF on the input (parent closed stdin / dropped the handle) ends the
    // loop with a clean exit. A genuine per-execution failure is reported as an
    // `Error` line; the runner then continues to the next `Start` so a single bad
    // snippet does not poison a pooled process (the parent decides whether to
    // recycle it).
    loop {
        match run_code_mode_runner() {
            Ok(RunnerLoopOutcome::Completed) => {
                // Reset per-execution state and park for the next Start.
                cleanup_execution_jail(false);
                reset_runner_seq();
            }
            Ok(RunnerLoopOutcome::InputClosed) => {
                // Parent closed the pipe; shut the process down cleanly.
                cleanup_execution_jail(true);
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                // Emit failure is acceptable: if stdout is gone the parent has
                // already dropped this runner and will evict it regardless.
                runner_emit(CodeModeRunnerOutput::Error { error: err.error }).ok();
                // Reset and continue: the per-execution javy runtime is dropped
                // at the end of `run_code_mode_runner`, so a failed execution
                // leaves no JS state behind. Whether to reuse or recycle this
                // process is the parent pool's decision.
                cleanup_execution_jail(false);
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
    /// Stable temp directory the per-execution jails live under. The runner owns
    /// this `TempDir` directly so jails never land in the repo even if the
    /// parent process starts the runner from a workspace cwd.
    static JAIL_BASE: std::cell::RefCell<Option<tempfile::TempDir>> =
        const { std::cell::RefCell::new(None) };
    /// The current per-execution jail subdir, so the next execution can remove
    /// it before creating a fresh one. `None` until the first execution.
    static EXECUTION_JAIL: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Create a fresh empty per-execution working directory and `chdir` into it,
/// removing the previous execution's directory first. Best-effort: on any
/// failure the process is left in a still-valid isolated cwd — the owned
/// `/tmp/labby-codemode-*` base when available. See the call site for why this
/// is defense-in-depth rather than a hard containment boundary.
fn reset_execution_jail() {
    let base = jail_base_path();
    let Some(base) = base else {
        return;
    };

    EXECUTION_JAIL.with(|cell| {
        let mut cell = cell.borrow_mut();
        // Remove the previous execution's jail (if any) so no file state from a
        // prior caller survives on this pooled process.
        if let Some(previous) = cell.take() {
            // Best-effort cleanup: the base TempDir is already an isolated cwd,
            // and a leftover previous jail is removed again on runner drop.
            std::env::set_current_dir(&base).ok();
            std::fs::remove_dir_all(&previous).ok();
        }
        let unique = format!("exec-{}-{}", std::process::id(), next_jail_seq());
        let jail = base.join(unique);
        if std::fs::create_dir(&jail).is_err() {
            // We already removed the previous jail above, so the process must not
            // be left `chdir`'d inside a now-deleted directory. Fall back to the
            // stable base (the per-runner spawn TempDir) so cwd stays valid and
            // isolated. `*cell` is already `None` (taken above). A chdir failure
            // is acceptable: the process is still inside an isolated TempDir.
            std::env::set_current_dir(&base).ok();
            return;
        }
        if std::env::set_current_dir(&jail).is_ok() {
            *cell = Some(jail);
        } else {
            // Could not enter the jail; clean it up and fall back to the stable
            // base rather than the just-removed previous jail. Both are
            // best-effort: failure leaves the process in a still-isolated cwd.
            std::fs::remove_dir_all(&jail).ok();
            std::env::set_current_dir(&base).ok();
        }
    });
}

fn jail_base_path() -> Option<std::path::PathBuf> {
    JAIL_BASE.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            let temp_dir = tempfile::Builder::new()
                .prefix("labby-codemode-")
                .tempdir()
                .ok()?;
            *cell = Some(temp_dir);
        }
        cell.as_ref().map(|dir| dir.path().to_path_buf())
    })
}

fn cleanup_execution_jail(drop_base: bool) {
    let base = JAIL_BASE.with(|cell| cell.borrow().as_ref().map(|dir| dir.path().to_path_buf()));

    EXECUTION_JAIL.with(|cell| {
        if let Some(jail) = cell.borrow_mut().take() {
            if let Some(base) = &base {
                // Best-effort: cwd stays inside the isolated TempDir on failure.
                std::env::set_current_dir(base).ok();
            }
            // Best-effort cleanup: the TempDir removal on drop sweeps leftovers.
            std::fs::remove_dir_all(jail).ok();
        }
    });

    if drop_base {
        // Leave the base before its TempDir is dropped; a failed chdir only
        // risks a busy-directory removal, which TempDir tolerates silently.
        std::env::set_current_dir(std::env::temp_dir()).ok();
        JAIL_BASE.with(|cell| {
            // Clearing the slot drops the TempDir, which removes the base tree;
            // removal errors are intentionally swallowed by TempDir's Drop
            // (best-effort cleanup). This is a real destructor invocation, not
            // a silenced fallible call.
            *cell.borrow_mut() = None;
        });
    }
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
    error: Box<CodeModeCallError>,
}

impl From<String> for CodeModeRunnerError {
    fn from(message: String) -> Self {
        Self {
            error: Box::new(CodeModeCallError::new("server_error", message)),
        }
    }
}

/// Extract the complete structured Code Mode error embedded in a rejection.
///
/// The bridge keeps `Error.message` as pure JSON so caller JavaScript can use
/// `JSON.parse(e.message)`. QuickJS may prepend `Error: ` and append stack
/// frames when the rejection is uncaught, so locate the first/last braces and
/// parse only that span. Minimal caller-authored `{kind,message}` objects remain
/// valid and are upgraded to the current contract defaults.
///
/// One of three best-effort structured-error recovery seams — keep behavior
/// aligned when changing any of them:
/// - here (runner rejection text → `CodeModeCallError`),
/// - `crates/labby/src/entrypoint.rs` `cli_error_value` (anyhow string → CLI
///   JSON error),
/// - `crates/labby-gateway/src/upstream/tool_error.rs` `parsed_error_object`
///   (upstream MCP content → parsed error object).
fn extract_structured_error(message: &str) -> Option<CodeModeCallError> {
    let start = message.find('{')?;
    let end = message.rfind('}')?;
    if end < start {
        return None;
    }
    let value = serde_json::from_str::<Value>(&message[start..=end]).ok()?;
    let object = value.as_object()?;
    let kind = object.get("kind")?.as_str()?.to_string();
    let human_message = object.get("message")?.as_str()?.to_string();
    serde_json::from_value(value)
        .ok()
        .or_else(|| Some(CodeModeCallError::new(kind, human_message)))
}

/// Classify a main-promise rejection while preserving the complete error object.
fn classify_rejection(message: String) -> CodeModeRunnerError {
    if let Some(error) = extract_structured_error(&message) {
        return CodeModeRunnerError {
            error: Box::new(error),
        };
    }
    if message.contains("JSON-serializable") {
        return CodeModeRunnerError {
            error: Box::new(CodeModeCallError::new("invalid_param", message)),
        };
    }
    let message = add_code_mode_hint("server_error", &message);
    CodeModeRunnerError {
        error: Box::new(CodeModeCallError::new("server_error", message)),
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
    let CodeModeRunnerInput::Start { code, proxy } = input else {
        return Err("runner expected start message".to_string().into());
    };

    // Per-execution cwd jail (Perf H1 isolation): a pooled runner is long-lived,
    // so its process cwd must not accumulate state across executions. Create a
    // fresh empty subdir under the runner's spawn cwd and chdir into it, after
    // removing the previous execution's subdir. The JS sandbox exposes no fs
    // APIs, so this is defense-in-depth — it guarantees that even a future
    // host-side artifact path bug cannot let one execution observe a prior one's
    // working-directory contents on the same pooled process. Failure is
    // non-fatal: the spawn cwd is already an isolated TempDir.
    reset_execution_jail();

    let mut config = javy::Config::default();
    config
        .redirect_stdout_to_stderr(true)
        .memory_limit(64 * 1024 * 1024)
        .max_stack_size(CODE_MODE_STACK_SIZE_LIMIT);
    let runtime = javy::Runtime::new(config).map_err(|err| err.to_string())?;

    runtime
        .context()
        .with(|cx| -> javy::quickjs::Result<()> {
            let globals = cx.globals();
            globals.set(
                "__labEmitToolCall",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_tool_call(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            globals.set(
                "__labEmitArtifactWrite",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_artifact_write(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            globals.set(
                "__labEmitSnippetResolve",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_snippet_resolve(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            globals.set(
                "__labEmitStepBegin",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_step_begin(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            globals.set(
                "__labEmitStepResult",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_step_result(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            Ok(())
        })
        .map_err(javy_error_message)?;

    let wrapped = wrap_code_mode(&code, &proxy);

    // A failure here is a parse/eval error in the caller's code (e.g. the
    // malformed `async () => {`), before the main promise is ever created. That
    // is a caller mistake, not a backend fault → `invalid_param`. (A non-function
    // body like `42` evals fine; its TypeError surfaces later as a promise
    // rejection and is classified by `classify_rejection`.)
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(wrapped))
        .map_err(|err| CodeModeRunnerError {
            error: Box::new(CodeModeCallError::new(
                "invalid_param",
                javy_error_message(err),
            )),
        })?;

    // Run the event loop until the main promise settles.
    let resolved_result = loop {
        runtime
            .resolve_pending_jobs()
            .map_err(|err| err.to_string())?;
        match javy_main_promise_state(&runtime)? {
            JavyMainPromiseState::Resolved(result) => break result,
            JavyMainPromiseState::Rejected(message) => {
                return Err(classify_rejection(message));
            }
            JavyMainPromiseState::Pending => {
                // Mid-execution EOF means the parent died while a tool call was
                // in flight — a genuine fault, not the clean pool-shutdown path.
                let input = runner_read_input().map_err(RunnerReadError::into_runner_error)?;
                javy_settle_tool_promise(&runtime, &input)?;
            }
        }
    };

    runner_emit(CodeModeRunnerOutput::Done {
        result: CodeModeRunnerResult::from_response_result(resolved_result),
        logs: Vec::new(),
    })
    .map_err(CodeModeRunnerError::from)?;
    Ok(RunnerLoopOutcome::Completed)
}

fn runner_emit(output: CodeModeRunnerOutput) -> Result<(), String> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| "runner state is not initialized".to_string())?;
        serde_json::to_writer(&mut state.writer, &output).map_err(|err| err.to_string())?;
        state
            .writer
            .write_all(b"\n")
            .map_err(|err| err.to_string())?;
        state.writer.flush().map_err(|err| err.to_string())
    })
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

impl RunnerReadError {
    /// Collapse to a `CodeModeRunnerError` for mid-execution reads, where an EOF
    /// is a genuine fault (the parent died while a tool call was in flight)
    /// rather than the clean pool-shutdown path.
    fn into_runner_error(self) -> CodeModeRunnerError {
        match self {
            Self::InputClosed => "runner input closed".to_string().into(),
            Self::Other(message) => message.into(),
        }
    }
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
