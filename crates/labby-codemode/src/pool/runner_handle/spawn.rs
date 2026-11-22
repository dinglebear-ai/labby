use crate::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput, RUNNER_PROTOCOL_VERSION};
use futures::StreamExt as _;
use std::process::Stdio;

use tokio::sync::OwnedSemaphorePermit;
use tokio_util::codec::{FramedRead, LinesCodec};

#[cfg(windows)]
use super::arm_job_object;
use super::{MAX_LINE_BYTES, PooledRunner, StderrBuffer, spawn_stderr_drain};
#[cfg(all(test, windows))]
use super::{job_assignment_error, missing_child_pid_error};
use crate::error::ToolError;
use crate::pool::microsandbox::runner_command;

impl PooledRunner {
    pub(crate) async fn check_protocol_version(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<(), ToolError> {
        *self.lines.decoder_mut() = LinesCodec::new_with_max_length(256);
        let compatible = async {
            crate::runner_io::write_runner_input(
                self.stdin.as_mut()?,
                &CodeModeRunnerInput::ProtocolVersion,
            )
            .await
            .ok()?;
            let reply = self.lines.next().await?.ok()?;
            match serde_json::from_str::<CodeModeRunnerOutput>(&reply).ok()? {
                CodeModeRunnerOutput::ProtocolVersion {
                    version: RUNNER_PROTOCOL_VERSION,
                } => Some(()),
                _ => None,
            }
        };
        if tokio::time::timeout_at(deadline, compatible)
            .await
            .ok()
            .flatten()
            .is_none()
        {
            return Err(ToolError::Sdk {
                sdk_kind: "runner_version_mismatch".into(),
                message: format!(
                    "Code Mode runner did not confirm wire protocol version {RUNNER_PROTOCOL_VERSION}"
                ),
            });
        }
        *self.lines.decoder_mut() = LinesCodec::new_with_max_length(MAX_LINE_BYTES);
        Ok(())
    }

    /// Spawn a fresh long-lived runner process using the host-supplied
    /// re-invocation (program + args). Defaults to `current_exe()` +
    /// `["internal", "code-mode-runner"]` via [`crate::pool::RunnerSpawn::try_default`].
    pub(crate) async fn spawn(
        spawn: &crate::pool::RunnerSpawn,
        microsandbox_config: Option<&crate::pool::MicrosandboxSpawn>,
        admission_permit: Option<OwnedSemaphorePermit>,
    ) -> Result<Self, ToolError> {
        // Each runner gets its own isolated cwd. It is a long-lived TempDir; the
        // runner creates a fresh per-execution subdir under it on every `Start`.
        let temp_dir = tempfile::TempDir::new().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create Code Mode sandbox directory: {err}"),
        })?;

        let (mut cmd, microsandbox) =
            runner_command(spawn, microsandbox_config, admission_permit).await?;
        cmd.env_clear();
        if microsandbox.is_none() {
            cmd.current_dir(temp_dir.path());
            // Nested execution jails are reclaimed with the parent on forced termination.
            #[cfg(unix)]
            cmd.env("TMPDIR", temp_dir.path());
            #[cfg(windows)]
            cmd.env("TMP", temp_dir.path()).env("TEMP", temp_dir.path());
        }
        cmd.kill_on_drop(true)
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
        let job_guard = Some(arm_job_object(&mut child, child_pid).await?);

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
            stdin: Some(stdin),
            lines,
            stderr,
            executions: 0,
            reuse_ready: false,
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
        let pid = child_pid.ok_or_else(missing_child_pid_error)?;
        #[cfg(windows)]
        let job_guard = Some(
            labby_winjob::JobObject::assign(pid)
                .map_err(|error| job_assignment_error(pid, &error))?,
        );
        let stdin = child.stdin.take().expect("stub stdin");
        let stdout = child.stdout.take().expect("stub stdout");
        let stderr_pipe = child.stderr.take().expect("stub stderr");
        let stderr = StderrBuffer::new();
        let drain_task = spawn_stderr_drain(stderr_pipe, stderr.clone());
        let lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_LINE_BYTES));
        Ok(Self {
            child,
            child_pid,
            stdin: Some(stdin),
            lines,
            stderr,
            executions: 0,
            reuse_ready: false,
            #[cfg(windows)]
            _job_guard: job_guard,
            drain_task,
            _temp_dir: temp_dir,
            microsandbox: None,
            spawned_at: std::time::Instant::now(),
        })
    }
}
