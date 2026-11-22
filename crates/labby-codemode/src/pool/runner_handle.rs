//! A single long-lived Code Mode runner process and its parent-side I/O.
//!
//! A `PooledRunner` owns one `labby internal code-mode-runner` subprocess that
//! stays alive across executions. The expensive `fork()` + process startup is
//! paid once at spawn; each execution builds a FRESH `javy::Runtime` inside the
//! process (runner-side contract), so no JS state leaks between callers.
//!
//! Security invariants preserved at spawn (set once, persist for the process):
//! - `env_clear()` — the child inherits no `LABBY_*`/ambient env.
//! - `process_group(0)` (Unix) / Job Object (Windows) — `killpg`/job close
//!   reaps assigned descendants on shutdown/eviction/drop. Windows assignment
//!   fails runner startup rather than silently degrading; the unavoidable
//!   spawn-to-assignment race is documented by `labby-winjob`.
//! - `kill_on_drop(true)` — dropping the handle kills the process.
//!
//! Per-execution invariants (heap/timeout/stack, fresh jail) are enforced
//! runner-side per `Start`.

mod spawn;
use std::sync::Arc;

use futures::StreamExt as _;
use tokio::process::ChildStdin;
use tokio::sync::{Mutex, Notify};
use tokio_util::codec::{FramedRead, LinesCodec};

use super::microsandbox::MicrosandboxGuard;
#[cfg(windows)]
use crate::error::ToolError;

#[derive(Debug)]
pub(crate) struct RunnerShutdownError {
    pid: Option<u32>,
    failures: Vec<String>,
}

impl std::fmt::Display for RunnerShutdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Code Mode runner shutdown failed for pid {:?}: {}",
            self.pid,
            self.failures.join("; ")
        )
    }
}

impl std::error::Error for RunnerShutdownError {}

/// Per-line safety cap mirrored from the original driver: 64 MiB heap + framing
/// headroom. A longer line is a protocol violation.
///
/// Note this is a per-runner transient ceiling: the parent may buffer up to this
/// much for a single oversized stdout line, multiplied by the number of live
/// runners (`LABBY_CODE_MODE_POOL_SIZE` + `LABBY_CODE_MODE_POOL_MAX_OVERFLOW`, ~24 at
/// defaults). It is a hard bound that errors rather than growing unbounded, not a
/// steady-state allocation; raising the pool/overflow knobs raises this worst
/// case proportionally.
pub(crate) const MAX_LINE_BYTES: usize = 64 * 1024 * 1024 + 4 * 1024;

/// Stdout line stream type for a pooled runner.
pub(crate) type RunnerLines = FramedRead<tokio::process::ChildStdout, LinesCodec>;

#[cfg(all(windows, test))]
fn missing_child_pid_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "spawned Code Mode runner did not expose a process id".to_string(),
    }
}

#[cfg(windows)]
fn job_assignment_error(pid: u32, error: &labby_winjob::WinJobError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!(
            "failed to contain Code Mode runner pid {pid} in a Windows Job Object: {error}"
        ),
    }
}

#[cfg(windows)]
async fn arm_job_object_with<F>(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
    assign: F,
) -> Result<labby_winjob::JobObject, ToolError>
where
    F: FnOnce(u32) -> Result<labby_winjob::JobObject, labby_winjob::WinJobError>,
{
    let pid = pid.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "spawned Code Mode runner did not expose a process id".to_string(),
    })?;
    match assign(pid) {
        Ok(job) => Ok(job),
        Err(error) => {
            let mut cleanup = Vec::new();
            if let Err(kill_error) = child.start_kill() {
                cleanup.push(format!("direct-child termination failed: {kill_error}"));
            }
            match tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(wait_error)) => {
                    cleanup.push(format!("direct-child wait failed: {wait_error}"))
                }
                Err(_) => cleanup.push("direct-child reap timed out after 1 second".to_string()),
            }
            let mut mapped = job_assignment_error(pid, &error);
            if !cleanup.is_empty()
                && let ToolError::Sdk { message, .. } = &mut mapped
            {
                message.push_str(&format!("; cleanup also failed: {}", cleanup.join("; ")));
            }
            Err(mapped)
        }
    }
}

#[cfg(windows)]
async fn arm_job_object(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
) -> Result<labby_winjob::JobObject, ToolError> {
    arm_job_object_with(child, pid, labby_winjob::JobObject::assign).await
}

/// Shared, continuously-drained stderr buffer for one runner.
///
/// The background drain keeps native process diagnostics from blocking the child.
#[derive(Clone)]
pub(crate) struct StderrBuffer {
    state: Arc<Mutex<StderrState>>,
    /// Signalled on every push so a waiter can poll for post-`Done` flush.
    notify: Arc<Notify>,
}

#[derive(Default)]
struct StderrState {
    lines: Vec<String>,
    total_bytes: usize,
    capped: bool,
}

impl StderrBuffer {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(StderrState::default())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Current line count — the start index for the next execution's capture.
    pub(crate) async fn mark(&self) -> usize {
        self.state.lock().await.lines.len()
    }

    /// Return lines appended since `start_index`, then release all retained
    /// stderr lines for this runner. A runner executes one request at a time, so
    /// completed executions do not need historical stderr retained after the
    /// response has been materialized.
    pub(crate) async fn take_since_and_clear(&self, start_index: usize) -> Vec<String> {
        let mut state = self.state.lock().await;
        let captured = state
            .lines
            .get(start_index..)
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        *state = StderrState::default();
        captured
    }

    /// Release retained stderr without returning it, used when the runner
    /// reports a reusable per-execution error.
    pub(crate) async fn clear(&self) {
        *self.state.lock().await = StderrState::default();
    }

    /// Allow a bounded interval for diagnostics from a failed process to arrive.
    pub(crate) async fn flush_settle(&self) {
        const SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_millis(50);
        let deadline = tokio::time::Instant::now() + SETTLE_BUDGET;
        let mut last_len = self.state.lock().await.lines.len();
        loop {
            let notified = self.notify.notified();
            match tokio::time::timeout_at(deadline, notified).await {
                Ok(()) => {
                    let len = self.state.lock().await.lines.len();
                    if len == last_len {
                        // Spurious wake without growth; stop polling.
                        break;
                    }
                    last_len = len;
                }
                Err(_) => break, // settle budget elapsed
            }
        }
    }
}

/// A single long-lived runner process plus its parent-side I/O channels.
pub(crate) struct PooledRunner {
    pub(crate) child: tokio::process::Child,
    pub(crate) child_pid: Option<u32>,
    pub(crate) stdin: Option<ChildStdin>,
    pub(crate) lines: RunnerLines,
    pub(crate) stderr: StderrBuffer,
    /// Number of executions this runner has served (for recycle-after-K).
    pub(crate) executions: u64,
    /// Reset before each Start; only native Done proof can qualify pool release.
    pub(crate) reuse_ready: bool,
    /// Windows Job Object guard; reaps the descendant tree when dropped. On Unix
    /// the process-group + `killpg` covers the same role.
    #[cfg(windows)]
    _job_guard: Option<labby_winjob::JobObject>,
    /// Background stderr drain task; aborted on drop.
    drain_task: tokio::task::JoinHandle<()>,
    /// Direct-process spawn cwd and stable jail base. A Microsandbox runner
    /// creates its execution jail inside the guest instead.
    _temp_dir: tempfile::TempDir,
    microsandbox: Option<MicrosandboxGuard>,
    spawned_at: std::time::Instant,
}

impl PooledRunner {
    pub(crate) fn microsandbox_lifetime_elapsed(&self) -> bool {
        self.microsandbox.is_some()
            && self.spawned_at.elapsed() >= std::time::Duration::from_hours(23)
    }

    pub(crate) async fn shutdown(mut self) -> Result<(), RunnerShutdownError> {
        let mut failures = Vec::new();
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            use nix::sys::signal::Signal;
            use nix::unistd::Pid;
            if let Err(error) = nix::sys::signal::killpg(Pid::from_raw(pid as i32), Signal::SIGKILL)
                && error != nix::errno::Errno::ESRCH
            {
                failures.push(format!("killpg failed: {error}"));
            }
        }
        #[cfg(windows)]
        if let Some(job) = self._job_guard.take()
            && let Err(error) = job.close()
        {
            failures.push(format!(
                "{} failed with Win32 error {}",
                error.operation, error.code
            ));
        }
        #[cfg(windows)]
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = self.child.start_kill() {
                    failures.push(format!("TerminateProcess failed: {error}"));
                }
            }
            Err(error) => failures.push(format!("process status check failed: {error}")),
        }
        match tokio::time::timeout(std::time::Duration::from_secs(1), self.child.wait()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => failures.push(format!("process wait failed: {error}")),
            Err(_) => failures.push("process reap timed out after 1 second".to_string()),
        }
        if let Some(mut sandbox) = self.microsandbox.take() {
            sandbox.remove().await;
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(RunnerShutdownError {
                pid: self.child_pid,
                failures,
            })
        }
    }
}

impl Drop for PooledRunner {
    fn drop(&mut self) {
        // Stop draining stderr.
        self.drain_task.abort();
        // Reap the process group (Unix) so grandchildren are not orphaned. The
        // child itself is killed by `kill_on_drop(true)`; on Windows the Job
        // Object guard's Drop terminates the descendant tree.
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            use nix::sys::signal::Signal;
            use nix::unistd::Pid;
            let _ = nix::sys::signal::killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
    }
}

/// Background task: drain a runner's stderr pipe to EOF, appending lines to the
/// shared buffer (with the same hard caps the original single-run drain used).
fn spawn_stderr_drain(
    stderr: tokio::process::ChildStderr,
    buffer: StderrBuffer,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Caps mirror the original per-run drain so a single runaway execution
        // cannot grow the buffer without bound before the per-execution log caps
        // are applied downstream.
        const CAP_ENTRIES: usize = 100_000;
        const CAP_BYTES: usize = 8 * 1024 * 1024;
        const TRUNCATION_MARKER: &str =
            "[labby] runner stderr truncated after 100000 lines or 8 MiB";
        let mut lines = FramedRead::new(stderr, LinesCodec::new_with_max_length(CAP_BYTES));
        while let Some(result) = lines.next().await {
            match result {
                Ok(line) => {
                    let mut state = buffer.state.lock().await;
                    if state.capped {
                        continue;
                    }
                    state.total_bytes = state.total_bytes.saturating_add(line.len() + 1);
                    if state.lines.len() >= CAP_ENTRIES || state.total_bytes > CAP_BYTES {
                        state.capped = true;
                        if state
                            .lines
                            .last()
                            .is_none_or(|last| last != TRUNCATION_MARKER)
                        {
                            if state.lines.len() >= CAP_ENTRIES {
                                state.lines.pop();
                            }
                            state.lines.push(TRUNCATION_MARKER.to_string());
                        }
                    } else {
                        state.lines.push(line);
                    }
                    drop(state);
                    buffer.notify.notify_waiters();
                }
                Err(tokio_util::codec::LinesCodecError::MaxLineLengthExceeded) => {
                    let mut state = buffer.state.lock().await;
                    state.capped = true;
                    if state
                        .lines
                        .last()
                        .is_none_or(|last| last != TRUNCATION_MARKER)
                    {
                        state.lines.push(TRUNCATION_MARKER.to_string());
                    }
                    drop(state);
                    buffer.notify.notify_waiters();
                }
                Err(tokio_util::codec::LinesCodecError::Io(error)) => {
                    tracing::warn!(
                        target: "labby_codemode.runner",
                        error = %error,
                        "runner stderr drain failed"
                    );
                    {
                        let mut state = buffer.state.lock().await;
                        if state.lines.len() < CAP_ENTRIES {
                            state
                                .lines
                                .push(format!("[labby] runner stderr drain failed: {error}"));
                        }
                    }
                    buffer.notify.notify_waiters();
                    return;
                }
            }
        }
    })
}

#[cfg(all(test, windows))]
mod windows_fixture;

#[cfg(test)]
mod tests;
