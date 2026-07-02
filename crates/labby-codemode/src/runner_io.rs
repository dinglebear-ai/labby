//! Parent-side helpers driving the Code Mode runner subprocess: stdin writes
//! and termination.

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};

use crate::error::ToolError;

use super::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput, RUNNER_STATE};

pub(crate) async fn write_runner_input(
    stdin: &mut ChildStdin,
    input: &CodeModeRunnerInput,
) -> Result<(), ToolError> {
    let mut line = serde_json::to_vec(input).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to encode Code Mode runner input: {err}"),
    })?;
    line.push(b'\n');
    stdin.write_all(&line).await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to write Code Mode runner input: {err}"),
    })?;
    stdin.flush().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to flush Code Mode runner input: {err}"),
    })
}

pub(crate) async fn terminate_code_mode_runner(child: &mut Child, _pid: Option<u32>) {
    // On Unix, kill the entire process group (pgid == pid because we spawned
    // with process_group(0)) so that grandchildren are not re-parented to
    // PID 1 and left running after the runner exits.
    #[cfg(unix)]
    {
        if let Some(raw_pid) = _pid {
            use nix::sys::signal::Signal;
            use nix::unistd::Pid;
            let _ = nix::sys::signal::killpg(Pid::from_raw(raw_pid as i32), Signal::SIGKILL);
        }
    }
    // On Windows, the `PooledRunner._job_guard` (a `JobObjectGuard` armed at
    // spawn in `pool/runner_handle.rs`) owns a Job Object with
    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Dropping that guard when the runner
    // handle drops (on eviction, including after a timeout) lets the OS terminate
    // the whole descendant tree. This kill() call is therefore a
    // belt-and-suspenders direct kill of the immediate child process.
    drop(child.kill().await);
    drop(child.wait().await);
}

pub(crate) fn runner_next_seq_blocking() -> Result<u64, ToolError> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "runner state is not initialized".to_string(),
        })?;
        let seq = state.next_seq;
        state.next_seq = state.next_seq.saturating_add(1);
        Ok(seq)
    })
}

pub(crate) fn runner_emit_blocking(output: CodeModeRunnerOutput) -> Result<(), ToolError> {
    use std::io::Write;

    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "runner state is not initialized".to_string(),
        })?;
        serde_json::to_writer(&mut state.writer, &output).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to encode Code Mode runner output: {err}"),
        })?;
        state
            .writer
            .write_all(b"\n")
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to write Code Mode runner output: {err}"),
            })?;
        state.writer.flush().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to flush Code Mode runner output: {err}"),
        })
    })
}

pub(crate) fn runner_read_input_blocking() -> Result<CodeModeRunnerInput, ToolError> {
    use std::io::BufRead;

    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "runner state is not initialized".to_string(),
        })?;
        let mut line = String::new();
        let read = state
            .reader
            .read_line(&mut line)
            .map_err(|err| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to read Code Mode runner input: {err}"),
            })?;
        if read == 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: "runner input closed".to_string(),
            });
        }
        serde_json::from_str(&line).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to decode Code Mode runner input: {err}"),
        })
    })
}
