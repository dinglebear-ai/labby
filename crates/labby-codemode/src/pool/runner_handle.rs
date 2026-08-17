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
//!   reaps grandchildren on shutdown/eviction/drop.
//! - `kill_on_drop(true)` — dropping the handle kills the process.
//!
//! Per-execution invariants (heap/timeout/stack, fresh jail) are enforced
//! runner-side per `Start`.

use std::process::Stdio;
use std::sync::Arc;

use futures::StreamExt as _;
use tokio::process::ChildStdin;
use tokio::sync::{Mutex, Notify};
use tokio_util::codec::{FramedRead, LinesCodec};

use super::microsandbox::{MicrosandboxGuard, runner_command};
use crate::error::ToolError;

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

/// Shared, continuously-drained stderr buffer for one runner.
///
/// The runner redirects `console.*` to stderr; a background task drains it to
/// EOF so a >64 KiB burst can never block the child on a full pipe. Per-execution
/// log capture slices `[start_index..]` of this buffer.
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

    /// Wait (bounded) for the stderr drain to flush lines emitted before `Done`.
    ///
    /// `Done` arrives on stdout once the JS settles; the child has already
    /// *written* its console output to the stderr pipe by then, but the parent's
    /// async drain may not have read it yet. Poll for the buffer to stop growing,
    /// bounded by a short deadline — logs are best-effort, never a correctness
    /// boundary.
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
    pub(crate) stdin: ChildStdin,
    pub(crate) lines: RunnerLines,
    pub(crate) stderr: StderrBuffer,
    /// Number of executions this runner has served (for recycle-after-K).
    pub(crate) executions: u64,
    /// Windows Job Object guard; reaps the descendant tree when dropped. On Unix
    /// the process-group + `killpg` covers the same role.
    #[cfg(windows)]
    _job_guard: Option<crate::pool::job_guard::JobObjectGuard>,
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

    pub(crate) async fn shutdown(mut self) {
        if let Some(pid) = self.child_pid {
            #[cfg(unix)]
            {
                use nix::sys::signal::Signal;
                use nix::unistd::Pid;
                let _ = nix::sys::signal::killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
            }
            #[cfg(windows)]
            let _ = self.child.start_kill();
        }
        drop(tokio::time::timeout(std::time::Duration::from_secs(1), self.child.wait()).await);
        if let Some(mut sandbox) = self.microsandbox.take() {
            sandbox.remove().await;
        }
    }

    /// Spawn a fresh long-lived runner process using the host-supplied
    /// re-invocation (program + args). Defaults to `current_exe()` +
    /// `["internal", "code-mode-runner"]` via [`crate::pool::RunnerSpawn::try_default`].
    pub(crate) async fn spawn(
        spawn: &super::super::pool::RunnerSpawn,
        microsandbox_config: Option<&super::super::pool::MicrosandboxSpawn>,
    ) -> Result<Self, ToolError> {
        // Each runner gets its own isolated cwd. It is a long-lived TempDir; the
        // runner creates a fresh per-execution subdir under it on every `Start`.
        let temp_dir = tempfile::TempDir::new().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create Code Mode sandbox directory: {err}"),
        })?;

        let (mut cmd, microsandbox) = runner_command(spawn, microsandbox_config).await?;
        if microsandbox.is_none() {
            cmd.current_dir(temp_dir.path());
        }
        cmd.env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "failed to spawn Code Mode runner from `{}`: {err}",
                spawn.program.display()
            ),
        })?;
        let child_pid = child.id();

        #[cfg(windows)]
        let job_guard = child_pid.map(crate::pool::job_guard::JobObjectGuard::arm);

        let stdin = child.stdin.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdin was not available".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdout was not available".to_string(),
        })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stderr was not available".to_string(),
        })?;

        let stderr = StderrBuffer::new();
        let drain_task = spawn_stderr_drain(stderr_pipe, stderr.clone());

        let lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_LINE_BYTES));

        Ok(Self {
            child,
            child_pid,
            stdin,
            lines,
            stderr,
            executions: 0,
            #[cfg(windows)]
            _job_guard: job_guard,
            drain_task,
            _temp_dir: temp_dir,
            microsandbox,
            spawned_at: std::time::Instant::now(),
        })
    }

    /// Test-only: spawn a long-lived stand-in process that parks reading stdin
    /// (like a parked runner) without speaking the protocol. Used to unit-test
    /// the pool's lease / free-list / recycle / eviction bookkeeping and PID
    /// reuse without needing the real labby binary (`current_exe()` in a lib
    /// unit test is the test harness, not the runner).
    ///
    /// The stand-in program must exist on the test host AND resolve under the
    /// `env_clear()` in `spawn_stub_command` (no inherited `PATH`). On Unix `cat`
    /// satisfies both; on Windows `cat`/`sleep` don't exist, so we use System32
    /// built-ins, which `CreateProcess` finds via its default search order even
    /// with an empty environment. `findstr` reads stdin and parks until EOF,
    /// mirroring `cat`.
    #[cfg(test)]
    pub(crate) fn spawn_stub() -> Result<Self, ToolError> {
        #[cfg(not(windows))]
        {
            Self::spawn_stub_command("cat", &[])
        }
        #[cfg(windows)]
        {
            // `findstr ^` matches every line and reads stdin until EOF, so it
            // parks on an open-but-idle stdin pipe just like `cat`.
            Self::spawn_stub_command(r"C:\Windows\System32\findstr.exe", &["^"])
        }
    }

    /// Test-only: a stub that consumes nothing on stdout and stays alive for a
    /// long time, modelling a runner that never replies. Used to exercise the
    /// parent-side wall-clock timeout path in `drive_runner`.
    #[cfg(test)]
    pub(crate) fn spawn_stub_silent() -> Result<Self, ToolError> {
        // The program ignores stdin and emits nothing on stdout, so the drive
        // loop's `lines.next()` pends until the wall-clock deadline fires.
        #[cfg(not(windows))]
        {
            Self::spawn_stub_command("sleep", &["3600"])
        }
        #[cfg(windows)]
        {
            // `timeout` refuses redirected stdin, and PowerShell startup can be
            // noisy/host-dependent on self-hosted CI. `cmd /C ping ... >NUL`
            // is quiet, long-lived, and uses absolute System32 paths because
            // this stub runs under `env_clear()`.
            Self::spawn_stub_command(
                r"C:\Windows\System32\cmd.exe",
                &[
                    "/D",
                    "/Q",
                    "/C",
                    r"C:\Windows\System32\ping.exe -n 3600 127.0.0.1 >NUL",
                ],
            )
        }
    }

    /// Test-only (Unix): spawn a stub that runs an arbitrary `sh` script,
    /// letting drive-loop tests emit scripted protocol lines (e.g. `ToolCall`
    /// events) on stdout. `sh` resolves under `env_clear()` via the libc
    /// default path, same as `cat` in `spawn_stub`.
    #[cfg(all(test, not(windows)))]
    pub(crate) fn spawn_stub_script(script: &str) -> Result<Self, ToolError> {
        Self::spawn_stub_command("sh", &["-c", script])
    }

    #[cfg(test)]
    fn spawn_stub_command(program: &str, args: &[&str]) -> Result<Self, ToolError> {
        let temp_dir = tempfile::TempDir::new().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create stub sandbox directory: {err}"),
        })?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        cmd.current_dir(temp_dir.path())
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to spawn stub runner: {err}"),
        })?;
        let child_pid = child.id();
        #[cfg(windows)]
        let job_guard = child_pid.map(crate::pool::job_guard::JobObjectGuard::arm);
        let stdin = child.stdin.take().expect("stub stdin");
        let stdout = child.stdout.take().expect("stub stdout");
        let stderr_pipe = child.stderr.take().expect("stub stderr");
        let stderr = StderrBuffer::new();
        let drain_task = spawn_stderr_drain(stderr_pipe, stderr.clone());
        let lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_LINE_BYTES));
        Ok(Self {
            child,
            child_pid,
            stdin,
            lines,
            stderr,
            executions: 0,
            #[cfg(windows)]
            _job_guard: job_guard,
            drain_task,
            _temp_dir: temp_dir,
            microsandbox: None,
            spawned_at: std::time::Instant::now(),
        })
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
        if let Some(pid) = self.child_pid {
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

#[cfg(test)]
mod tests {
    use super::{PooledRunner, StderrBuffer};

    #[tokio::test]
    async fn stderr_buffer_take_since_and_clear_releases_retained_lines() {
        let buffer = StderrBuffer::new();
        buffer.state.lock().await.lines.push("before".to_string());
        let mark = buffer.mark().await;
        {
            let mut state = buffer.state.lock().await;
            state.lines.push("during-one".to_string());
            state.lines.push("during-two".to_string());
        }

        let captured = buffer.take_since_and_clear(mark).await;

        assert_eq!(captured, ["during-one", "during-two"]);
        assert!(buffer.state.lock().await.lines.is_empty());
    }

    #[tokio::test]
    async fn stderr_buffer_clear_discards_retained_lines() {
        let buffer = StderrBuffer::new();
        buffer
            .state
            .lock()
            .await
            .lines
            .push("discard me".to_string());

        buffer.clear().await;

        assert!(buffer.state.lock().await.lines.is_empty());
    }

    #[tokio::test]
    async fn stderr_buffer_clear_resets_per_execution_caps() {
        let buffer = StderrBuffer::new();
        {
            let mut state = buffer.state.lock().await;
            state.lines.push("[labby] runner stderr truncated".into());
            state.total_bytes = 8 * 1024 * 1024;
            state.capped = true;
        }

        buffer.clear().await;

        let state = buffer.state.lock().await;
        assert!(state.lines.is_empty());
        assert_eq!(state.total_bytes, 0);
        assert!(!state.capped);
    }

    /// Real KVM smoke. Example:
    ///
    /// ```text
    /// LABBY_MICROSANDBOX_SMOKE_MSB=/absolute/path/to/msb \
    /// LABBY_MICROSANDBOX_SMOKE_RUNNER=/absolute/path/to/labby \
    /// LABBY_MICROSANDBOX_SMOKE_IMAGE=debian \
    /// cargo test -p labby-codemode microsandbox_runner_round_trip -- --ignored
    /// ```
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires Linux KVM, Microsandbox, a cached image, and a Labby binary"]
    async fn microsandbox_runner_round_trip() {
        use futures::StreamExt as _;
        use tokio::io::AsyncWriteExt as _;

        let msb =
            std::env::var_os("LABBY_MICROSANDBOX_SMOKE_MSB").expect("LABBY_MICROSANDBOX_SMOKE_MSB");
        let runner = std::env::var_os("LABBY_MICROSANDBOX_SMOKE_RUNNER")
            .expect("LABBY_MICROSANDBOX_SMOKE_RUNNER");
        let image = std::env::var("LABBY_MICROSANDBOX_SMOKE_IMAGE")
            .expect("LABBY_MICROSANDBOX_SMOKE_IMAGE");
        let spawn = crate::pool::RunnerSpawn {
            program: runner.into(),
            args: vec!["internal".into(), "code-mode-runner".into()],
        };
        let microsandbox = crate::pool::MicrosandboxSpawn {
            executable: msb.into(),
            image,
        };

        let mut runner = PooledRunner::spawn(&spawn, Some(&microsandbox))
            .await
            .expect("spawn microVM");
        runner
            .stdin
            .write_all(b"{\"type\":\"start\",\"code\":\"async () => 42\",\"proxy\":\"\"}\n")
            .await
            .expect("write start");
        runner.stdin.flush().await.expect("flush start");
        let line = tokio::time::timeout(std::time::Duration::from_secs(15), runner.lines.next())
            .await
            .expect("runner response timeout")
            .expect("runner stdout closed")
            .expect("runner protocol line");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid protocol JSON");
        assert_eq!(value["type"], "done");
        assert_eq!(value["result"]["value"], 42);
    }
}
