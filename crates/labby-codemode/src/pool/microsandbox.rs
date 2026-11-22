//! Microsandbox lifecycle and byte-faithful stdio attachment for a runner.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock, atomic::AtomicUsize, mpsc};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::Command;
use tokio::sync::OwnedSemaphorePermit;

use crate::error::ToolError;
use crate::pool::RunnerSpawn;

use reconciliation::{
    ensure_cleanup_circuit_closed, reconcile_failed_cleanups, reconcile_stale_sandboxes,
};

use cleanup::{
    cleanup_executor, cleanup_failed, complete_cleanup, failed_cleanups, resolve_failed_cleanup,
};

mod cleanup;
mod reconciliation;
#[cfg(all(test, unix))]
mod tests;

static NEXT_SANDBOX_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static ACTIVE_SANDBOXES: AtomicUsize = AtomicUsize::new(0);
static FAILED_CLEANUPS: OnceLock<Mutex<HashMap<CleanupIdentity, bool>>> = OnceLock::new();
static CLEANUP_EXECUTOR: OnceLock<mpsc::SyncSender<CleanupJob>> = OnceLock::new();
static RECONCILED_EXECUTABLES: OnceLock<
    tokio::sync::Mutex<HashMap<std::path::PathBuf, Option<(Instant, ToolError)>>>,
> = OnceLock::new();

const HELPER_OUTPUT_LIMIT: usize = 8 * 1024;
/// Default wall-clock bound for one `msb` helper invocation.
const DEFAULT_HELPER_TIMEOUT: Duration = Duration::from_secs(5);
/// Operator override for [`DEFAULT_HELPER_TIMEOUT`], in milliseconds.
///
/// The bound is wall-clock, so on a saturated host a helper that would have
/// finished can be killed and reported as `cleanup_timeout`. Raising it trades
/// slower failure detection for fewer false timeouts.
const HELPER_TIMEOUT_ENV: &str = "LABBY_CODE_MODE_MICROSANDBOX_HELPER_TIMEOUT_MS";
static HELPER_TIMEOUT: OnceLock<Option<Duration>> = OnceLock::new();

/// Wall-clock bound applied to every `msb` helper invocation. Read once.
fn helper_timeout(default: Duration) -> Duration {
    HELPER_TIMEOUT
        .get_or_init(|| {
            std::env::var(HELPER_TIMEOUT_ENV)
                .ok()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|millis| *millis > 0)
                .map(Duration::from_millis)
        })
        .unwrap_or(default)
}
const CLEANUP_WORKERS: usize = 2;
const CLEANUP_QUEUE_CAPACITY: usize = 32;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CleanupIdentity {
    executable: std::path::PathBuf,
    name: String,
}

struct CleanupJob {
    identity: CleanupIdentity,
    counted: bool,
    failed_cleanup_recorded: bool,
    _admission_permit: Option<OwnedSemaphorePermit>,
}

pub(super) struct MicrosandboxGuard {
    executable: std::path::PathBuf,
    name: String,
    removed: bool,
    counted: bool,
    failed_cleanup_recorded: bool,
    admission_permit: Option<OwnedSemaphorePermit>,
}

impl MicrosandboxGuard {
    pub(super) async fn remove(&mut self) {
        if self.removed {
            return;
        }
        let started = Instant::now();
        let identity = self.identity();
        let mut remove = Command::new(&identity.executable);
        remove
            .args(["remove", "--quiet", "--force", &self.name])
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        match run_helper(remove, helper_timeout(Duration::from_secs(2))).await {
            Ok(output) if output.status.success() => {
                self.removed = true;
                complete_cleanup(&identity, self.counted, self.failed_cleanup_recorded);
                self.counted = false;
                self.failed_cleanup_recorded = false;
                log_lifecycle(
                    "microsandbox.remove",
                    &identity.name,
                    started,
                    "success",
                    None,
                );
            }
            Ok(output) => {
                cleanup_failed(&identity, self.counted);
                self.failed_cleanup_recorded = true;
                let error = sanitize_stderr(&output.stderr);
                tracing::warn!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "microsandbox.remove",
                    kind = "cleanup_failed",
                    sandbox = %self.name,
                    status = %output.status,
                    error,
                    "Microsandbox cleanup failed"
                );
                log_lifecycle(
                    "microsandbox.remove",
                    &identity.name,
                    started,
                    "failed",
                    Some("cleanup_failed"),
                );
            }
            Err(error) => {
                cleanup_failed(&identity, self.counted);
                self.failed_cleanup_recorded = true;
                tracing::warn!(
                    surface = "dispatch", service = "code_mode",
                    action = "microsandbox.remove", kind = error.kind,
                    sandbox = %self.name, executable = %self.executable.display(),
                    error = %error.message, "Microsandbox cleanup failed"
                );
                log_lifecycle(
                    "microsandbox.remove",
                    &identity.name,
                    started,
                    "failed",
                    Some(error.kind),
                );
            }
        }
    }

    fn remove_in_background(&mut self) {
        if self.removed {
            return;
        }
        self.removed = true;
        let job = CleanupJob {
            identity: self.identity(),
            counted: self.counted,
            failed_cleanup_recorded: self.failed_cleanup_recorded,
            _admission_permit: self.admission_permit.take(),
        };
        self.counted = false;
        if let Err(error) = cleanup_executor().try_send(job) {
            let job = match error {
                mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job,
            };
            cleanup_failed(&job.identity, job.counted);
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "microsandbox.remove_fallback",
                kind = "cleanup_queue_full", sandbox = %self.name,
                "Microsandbox cleanup queue is full; creation circuit opened"
            );
        }
    }

    fn identity(&self) -> CleanupIdentity {
        CleanupIdentity {
            executable: self.executable.clone(),
            name: self.name.clone(),
        }
    }
}

impl Drop for MicrosandboxGuard {
    fn drop(&mut self) {
        self.remove_in_background();
    }
}

pub(super) async fn runner_command(
    spawn: &RunnerSpawn,
    config: Option<&super::MicrosandboxSpawn>,
    admission_permit: Option<OwnedSemaphorePermit>,
) -> Result<(Command, Option<MicrosandboxGuard>), ToolError> {
    let Some(config) = config else {
        let mut command = Command::new(&spawn.program);
        command.args(&spawn.args);
        return Ok((command, None));
    };

    reconcile_stale_sandboxes(config).await?;
    reconcile_failed_cleanups(&config.executable).await?;

    let ordinal = NEXT_SANDBOX_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("labby-codemode-{}-{ordinal}", std::process::id());
    let mount = format!(
        "{}:/opt/labby/labby:ro,nodev,nosuid",
        spawn.program.display()
    );
    ensure_cleanup_circuit_closed(&config.executable)?;
    let mut guard = MicrosandboxGuard {
        executable: config.executable.clone(),
        name: name.clone(),
        removed: false,
        counted: false,
        failed_cleanup_recorded: false,
        admission_permit,
    };
    if let Err(error) = create(config, &name, &mount).await {
        guard.remove().await;
        return Err(error);
    }
    guard.counted = true;

    let mut command = Command::new(&config.executable);
    command.args([
        "exec",
        "--quiet",
        "--no-tty",
        "--stream",
        &name,
        "--",
        "/opt/labby/labby",
    ]);
    command.args(&spawn.args);
    Ok((command, Some(guard)))
}

async fn create(
    config: &super::MicrosandboxSpawn,
    name: &str,
    mount: &str,
) -> Result<(), ToolError> {
    let started = Instant::now();
    let owner_pid_label = format!("labby.pid={}", std::process::id());
    let mut create = Command::new(&config.executable);
    create
        .args([
            "create",
            "--quiet",
            "--name",
            name,
            "--replace-with-timeout",
            "0",
            "--security",
            "restricted",
            "--no-net",
            "--pull",
            "never",
            "--cpus",
            "1",
            "--memory",
            "256M",
            "--max-duration",
            "24h",
            "--label",
            "labby.owner=codemode",
            "--label",
            &owner_pid_label,
            "--mount-file",
            mount,
            &config.image,
        ])
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = match run_helper(create, helper_timeout(Duration::from_secs(15))).await {
        Ok(output) => output,
        Err(error) => {
            log_lifecycle(
                "microsandbox.create",
                name,
                started,
                "failed",
                Some(error.kind),
            );
            return Err(ToolError::Sdk {
                sdk_kind: error.sdk_kind.into(),
                message: format!(
                    "Microsandbox Code Mode runner creation failed: {}",
                    error.message
                ),
            });
        }
    };
    if output.status.success() {
        ACTIVE_SANDBOXES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        log_lifecycle("microsandbox.create", name, started, "success", None);
        return Ok(());
    }
    log_lifecycle(
        "microsandbox.create",
        name,
        started,
        "failed",
        Some("internal_error"),
    );
    Err(ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!(
            "failed to create Microsandbox Code Mode runner: {}",
            helper_diagnostic(&output)
        ),
    })
}

struct HelperOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct HelperError {
    kind: &'static str,
    sdk_kind: &'static str,
    message: String,
}

async fn run_helper(mut command: Command, timeout: Duration) -> Result<HelperOutput, HelperError> {
    let mut child = command.spawn().map_err(|error| HelperError {
        kind: "cleanup_spawn_failed",
        sdk_kind: "internal_error",
        message: format!("failed to start helper: {error}"),
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_drain = tokio::spawn(drain_bounded(stdout, HELPER_OUTPUT_LIMIT));
    let stderr_drain = tokio::spawn(drain_bounded(stderr, HELPER_OUTPUT_LIMIT));
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            return Err(HelperError {
                kind: "cleanup_wait_failed",
                sdk_kind: "internal_error",
                message: error.to_string(),
            });
        }
        Err(_) => {
            drop(child.kill().await);
            drop(child.wait().await);
            return Err(HelperError {
                kind: "cleanup_timeout",
                sdk_kind: "timeout",
                message: format!("helper timed out after {} seconds", timeout.as_secs()),
            });
        }
    };
    let stdout = stdout_drain.await.unwrap_or_default();
    let stderr = stderr_drain.await.unwrap_or_default();
    Ok(HelperOutput {
        status,
        stdout,
        stderr,
    })
}

async fn drain_bounded<R>(reader: Option<R>, retain: usize) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return Vec::new();
    };
    let mut retained = Vec::with_capacity(retain);
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = retain.saturating_sub(retained.len());
                retained.extend_from_slice(&buffer[..read.min(remaining)]);
            }
        }
    }
    retained
}

fn log_lifecycle(action: &str, sandbox: &str, started: Instant, outcome: &str, kind: Option<&str>) {
    tracing::info!(
        surface = "dispatch",
        service = "code_mode",
        action,
        sandbox,
        elapsed_ms = started.elapsed().as_millis(),
        outcome,
        active_count = ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed),
        kind = kind.unwrap_or(""),
        "Microsandbox lifecycle operation finished"
    );
}

fn sanitize_stderr(stderr: &[u8]) -> String {
    labby_runtime::redact::sanitize_error_text(&String::from_utf8_lossy(stderr), 512)
}

fn helper_diagnostic(output: &HelperOutput) -> String {
    let stderr = sanitize_stderr(&output.stderr);
    let stdout =
        labby_runtime::redact::sanitize_error_text(&String::from_utf8_lossy(&output.stdout), 512);
    match (stderr.is_empty(), stdout.is_empty()) {
        (false, false) => format!("stderr: {stderr}; stdout: {stdout}"),
        (false, true) => stderr,
        (true, false) => stdout,
        (true, true) => "helper returned no diagnostic output".to_string(),
    }
}
