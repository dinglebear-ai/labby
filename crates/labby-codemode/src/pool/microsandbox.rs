//! Microsandbox lifecycle and byte-faithful stdio attachment for a runner.

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock, atomic::AtomicUsize, mpsc};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::Command;
use tokio::sync::OwnedSemaphorePermit;

use crate::error::ToolError;
use crate::pool::RunnerSpawn;

static NEXT_SANDBOX_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static ACTIVE_SANDBOXES: AtomicUsize = AtomicUsize::new(0);
static FAILED_CLEANUPS: OnceLock<Mutex<HashSet<CleanupIdentity>>> = OnceLock::new();
static CLEANUP_EXECUTOR: OnceLock<mpsc::SyncSender<CleanupJob>> = OnceLock::new();
static RECONCILED_EXECUTABLES: OnceLock<tokio::sync::Mutex<HashSet<std::path::PathBuf>>> =
    OnceLock::new();

const HELPER_OUTPUT_LIMIT: usize = 8 * 1024;
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
    _admission_permit: Option<OwnedSemaphorePermit>,
}

pub(super) struct MicrosandboxGuard {
    executable: std::path::PathBuf,
    name: String,
    removed: bool,
    counted: bool,
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
        match run_helper(remove, Duration::from_secs(2)).await {
            Ok(output) if output.status.success() => {
                self.removed = true;
                cleanup_succeeded(&identity, self.counted);
                self.counted = false;
                log_lifecycle(
                    "microsandbox.remove",
                    &identity.name,
                    started,
                    "success",
                    None,
                );
            }
            Ok(output) => {
                let error = helper_diagnostic(&output);
                if sandbox_absent(&identity).await {
                    self.removed = true;
                    cleanup_succeeded(&identity, self.counted);
                    self.counted = false;
                    tracing::info!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "microsandbox.remove",
                        kind = "already_absent",
                        sandbox = %self.name,
                        status = %output.status,
                        error,
                        "Microsandbox cleanup target is already absent"
                    );
                    log_lifecycle(
                        "microsandbox.remove",
                        &identity.name,
                        started,
                        "success",
                        Some("already_absent"),
                    );
                } else {
                    cleanup_failed(&identity);
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
            }
            Err(error) => {
                if sandbox_absent(&identity).await {
                    self.removed = true;
                    cleanup_succeeded(&identity, self.counted);
                    self.counted = false;
                    tracing::info!(
                        surface = "dispatch", service = "code_mode",
                        action = "microsandbox.remove", kind = "already_absent",
                        sandbox = %self.name, executable = %self.executable.display(),
                        error = %error.message, "Microsandbox cleanup target is already absent"
                    );
                    log_lifecycle(
                        "microsandbox.remove",
                        &identity.name,
                        started,
                        "success",
                        Some("already_absent"),
                    );
                } else {
                    cleanup_failed(&identity);
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
    }

    fn remove_in_background(&mut self) {
        if self.removed {
            return;
        }
        self.removed = true;
        let job = CleanupJob {
            identity: self.identity(),
            counted: self.counted,
            _admission_permit: self.admission_permit.take(),
        };
        self.counted = false;
        if let Err(error) = cleanup_executor().try_send(job) {
            let job = match error {
                mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job,
            };
            cleanup_failed(&job.identity);
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
    let output = match run_helper(create, Duration::from_secs(15)).await {
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

async fn reconcile_stale_sandboxes(config: &super::MicrosandboxSpawn) -> Result<(), ToolError> {
    let reconciled = RECONCILED_EXECUTABLES.get_or_init(|| tokio::sync::Mutex::new(HashSet::new()));
    let mut reconciled = reconciled.lock().await;
    if reconciled.contains(&config.executable) {
        return Ok(());
    }

    let started = Instant::now();
    let mut list = Command::new(&config.executable);
    list.args(["list", "--quiet", "--label", "labby.owner=codemode"])
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run_helper(list, Duration::from_secs(5))
        .await
        .map_err(|error| ToolError::Sdk {
            sdk_kind: error.sdk_kind.into(),
            message: format!(
                "failed to reconcile stale Microsandbox runners: {}",
                error.message
            ),
        })?;
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "cleanup_failed".into(),
            message: format!(
                "failed to reconcile stale Microsandbox runners: {}",
                sanitize_stderr(&output.stderr)
            ),
        });
    }
    for name in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(pid) = sandbox_owner_pid(name) else {
            continue;
        };
        if std::path::Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        let mut remove = Command::new(&config.executable);
        remove
            .args(["remove", "--quiet", "--force", name])
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let removed = run_helper(remove, Duration::from_secs(5))
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: error.sdk_kind.into(),
                message: format!(
                    "failed to remove stale Microsandbox `{name}`: {}",
                    error.message
                ),
            })?;
        if !removed.status.success() {
            return Err(ToolError::Sdk {
                sdk_kind: "cleanup_failed".into(),
                message: format!(
                    "failed to remove stale Microsandbox `{name}`: {}",
                    sanitize_stderr(&removed.stderr)
                ),
            });
        }
    }
    reconciled.insert(config.executable.clone());
    log_lifecycle(
        "microsandbox.reconcile",
        "labby.owner=codemode",
        started,
        "success",
        None,
    );
    Ok(())
}

fn sandbox_owner_pid(name: &str) -> Option<u32> {
    name.strip_prefix("labby-codemode-")?
        .split_once('-')?
        .0
        .parse()
        .ok()
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

fn failed_cleanups() -> &'static Mutex<HashSet<CleanupIdentity>> {
    FAILED_CLEANUPS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn cleanup_failed(identity: &CleanupIdentity) {
    failed_cleanups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(identity.clone());
}

fn cleanup_succeeded(identity: &CleanupIdentity, counted: bool) {
    failed_cleanups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(identity);
    if counted {
        ACTIVE_SANDBOXES.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

fn ensure_cleanup_circuit_closed(executable: &std::path::Path) -> Result<(), ToolError> {
    let failed = failed_cleanups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let identities = failed
        .iter()
        .filter(|identity| identity.executable == executable)
        .count();
    if identities == 0 {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "cleanup_failed".into(),
        message: format!(
            "refusing to create a Microsandbox runner while {identities} sandbox cleanup(s) remain unresolved"
        ),
    })
}

fn cleanup_executor() -> &'static mpsc::SyncSender<CleanupJob> {
    CLEANUP_EXECUTOR.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(CLEANUP_QUEUE_CAPACITY);
        let receiver = std::sync::Arc::new(Mutex::new(receiver));
        for index in 0..CLEANUP_WORKERS {
            let receiver = std::sync::Arc::clone(&receiver);
            if let Err(error) = std::thread::Builder::new()
                .name(format!("labby-msb-cleanup-{index}"))
                .spawn(move || {
                    loop {
                        let job = receiver
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .recv();
                        let Ok(job) = job else { break };
                        run_fallback_cleanup(job);
                    }
                })
            {
                tracing::error!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "microsandbox.cleanup_executor",
                    kind = "cleanup_spawn_failed",
                    worker = index,
                    %error,
                    "failed to start bounded Microsandbox cleanup worker"
                );
            }
        }
        sender
    })
}

fn run_fallback_cleanup(job: CleanupJob) {
    let started = Instant::now();
    let mut child = std::process::Command::new(&job.identity.executable)
        .args(["remove", "--quiet", "--force", &job.identity.name])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let success = match child.as_mut() {
        Ok(child) => {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status.success(),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Ok(None) => {
                        drop(child.kill());
                        drop(child.wait());
                        break false;
                    }
                    Err(_) => break false,
                }
            }
        }
        Err(_) => false,
    };
    if success {
        cleanup_succeeded(&job.identity, job.counted);
        log_lifecycle(
            "microsandbox.remove_fallback",
            &job.identity.name,
            started,
            "success",
            None,
        );
    } else {
        cleanup_failed(&job.identity);
        log_lifecycle(
            "microsandbox.remove_fallback",
            &job.identity.name,
            started,
            "failed",
            Some("cleanup_failed"),
        );
    }
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

async fn sandbox_absent(identity: &CleanupIdentity) -> bool {
    let mut list = Command::new(&identity.executable);
    list.args(["list", "--quiet", "--label", "labby.owner=codemode"])
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    matches!(
        run_helper(list, Duration::from_secs(2)).await,
        Ok(output)
            if output.status.success()
                && !String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|name| name.trim() == identity.name)
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn sandbox_names_carry_parseable_process_ownership() {
        assert_eq!(sandbox_owner_pid("labby-codemode-123-9"), Some(123));
        assert_eq!(sandbox_owner_pid("foreign-123-9"), None);
        assert_eq!(sandbox_owner_pid("labby-codemode-bad-9"), None);
    }

    #[tokio::test]
    async fn reconciliation_removes_dead_owner_and_preserves_live_owner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executable = dir.path().join("msb");
        let calls = dir.path().join("calls");
        let live = format!("labby-codemode-{}-7", std::process::id());
        let dead = "labby-codemode-4294967294-8";
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = list ]; then printf '%s\\n' '{}' '{}'; fi\nexit 0\n",
            calls.display(),
            live,
            dead
        );
        std::fs::write(&executable, script).expect("write fake msb");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");

        let config = config(executable);
        let (_command, guard) = runner_command(&spawn(), Some(&config), None)
            .await
            .expect("reconciliation and create succeed");
        let calls = std::fs::read_to_string(&calls).expect("recorded calls");
        assert!(
            calls
                .lines()
                .any(|line| line == format!("remove --quiet --force {dead}")),
            "dead-owner sandbox was not removed: {calls}"
        );
        assert!(
            !calls
                .lines()
                .any(|line| line.contains(&format!("force {live}"))),
            "live-owner sandbox was removed: {calls}"
        );
        drop(guard);
    }

    fn fake_msb(create_status: i32) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let executable = dir.path().join("msb");
        let calls = dir.path().join("calls");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = create ]; then exit {create_status}; fi\nexit 0\n",
            calls.display()
        );
        std::fs::write(&executable, script).expect("write fake msb");
        let mut permissions = std::fs::metadata(&executable)
            .expect("fake msb metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).expect("make fake msb executable");
        (dir, executable, calls)
    }

    fn spawn() -> RunnerSpawn {
        RunnerSpawn {
            program: "/bin/true".into(),
            args: vec!["internal".into(), "code-mode-runner".into()],
        }
    }

    fn config(executable: std::path::PathBuf) -> super::super::MicrosandboxSpawn {
        super::super::MicrosandboxSpawn {
            executable,
            image: "debian".into(),
        }
    }

    #[tokio::test]
    async fn failed_create_attempts_bounded_cleanup() {
        let (_dir, executable, calls) = fake_msb(23);
        let config = config(executable);
        let error = runner_command(&spawn(), Some(&config), None)
            .await
            .err()
            .expect("create must fail");
        assert_eq!(error.kind(), "internal_error");
        let calls = std::fs::read_to_string(calls).expect("recorded calls");
        assert!(calls.lines().any(|line| line.starts_with("create ")));
        assert!(
            calls
                .lines()
                .any(|line| line.starts_with("remove --quiet --force labby-codemode-")),
            "failed creation must remove its named sandbox: {calls}"
        );
    }

    #[tokio::test]
    async fn cleanup_circuit_rejection_does_not_cleanup_never_created_sandbox() {
        let (_dir, executable, calls) = fake_msb(0);
        let config = config(executable.clone());
        let unresolved = CleanupIdentity {
            executable,
            name: "labby-codemode-existing-leak".to_string(),
        };
        cleanup_failed(&unresolved);

        let error = runner_command(&spawn(), Some(&config), None)
            .await
            .err()
            .expect("creation must fail closed");
        assert_eq!(error.kind(), "cleanup_failed");

        let recorded = std::fs::read_to_string(&calls).expect("recorded calls");
        assert!(
            !recorded.lines().any(|line| line.starts_with("create ")),
            "circuit rejection must happen before create: {recorded}"
        );
        assert!(
            !recorded.lines().any(|line| line.starts_with("remove ")),
            "circuit rejection must not clean up a never-created sandbox: {recorded}"
        );
        cleanup_succeeded(&unresolved, false);
    }

    #[tokio::test]
    async fn failed_create_with_absent_sandbox_does_not_poison_cleanup_circuit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executable = dir.path().join("msb");
        let calls = dir.path().join("calls");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = create ]; then exit 23; fi\nif [ \"$1\" = remove ]; then exit 9; fi\nexit 0\n",
            calls.display()
        );
        std::fs::write(&executable, script).expect("write fake msb");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");
        let config = config(executable);

        for _ in 0..2 {
            let error = runner_command(&spawn(), Some(&config), None)
                .await
                .err()
                .expect("create must fail");
            assert_eq!(
                error.kind(),
                "internal_error",
                "confirmed absence must not leave cleanup circuit debt"
            );
        }

        let recorded = std::fs::read_to_string(calls).expect("recorded calls");
        assert_eq!(
            recorded
                .lines()
                .filter(|line| line.starts_with("create "))
                .count(),
            2,
            "second create should not be blocked by phantom cleanup debt: {recorded}"
        );
    }

    #[tokio::test]
    async fn failed_create_preserves_stdout_and_stderr_diagnostics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executable = dir.path().join("msb");
        let script = "#!/bin/sh\nif [ \"$1\" = create ]; then printf 'backend-detail'; printf 'stderr-detail' >&2; exit 23; fi\nexit 0\n";
        std::fs::write(&executable, script).expect("write fake msb");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");

        let error = runner_command(&spawn(), Some(&config(executable)), None)
            .await
            .err()
            .expect("create must fail");
        let message = error.to_string();
        assert!(message.contains("stderr: stderr-detail"), "{message}");
        assert!(message.contains("stdout: backend-detail"), "{message}");
    }

    #[tokio::test]
    async fn oversized_helper_output_is_drained_but_diagnostic_is_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executable = dir.path().join("msb");
        let script = "#!/bin/sh\nif [ \"$1\" = create ]; then head -c 1048576 /dev/zero | tr '\\0' x >&2; exit 23; fi\nexit 0\n";
        std::fs::write(&executable, script).expect("write fake msb");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");

        let error = runner_command(&spawn(), Some(&config(executable)), None)
            .await
            .err()
            .expect("create must fail");
        let message = error.to_string();
        assert!(message.len() < 2_048, "diagnostic was not bounded");
        assert!(message.contains("failed to create Microsandbox"));
    }

    #[tokio::test]
    async fn successful_create_returns_stream_command_and_async_guard() {
        let (_dir, executable, calls) = fake_msb(0);
        let config = config(executable);
        let (command, guard) = runner_command(&spawn(), Some(&config), None)
            .await
            .expect("create succeeds");
        let debug = format!("{command:?}");
        assert!(debug.contains("--stream"));
        assert!(debug.contains("/opt/labby/labby"));
        let mut guard = guard.expect("Microsandbox guard");
        guard.remove().await;
        let calls = std::fs::read_to_string(calls).expect("recorded calls");
        let create = calls
            .lines()
            .find(|line| line.starts_with("create "))
            .expect("create call");
        for required in [
            "--security restricted",
            "--no-net",
            "--pull never",
            "--cpus 1",
            "--memory 256M",
            "--max-duration 24h",
            ":/opt/labby/labby:ro,nodev,nosuid",
        ] {
            assert!(
                create.contains(required),
                "missing `{required}` in: {create}"
            );
        }
        assert!(calls.lines().any(|line| line.starts_with("remove ")));
    }

    #[tokio::test]
    async fn failed_explicit_remove_gets_bounded_drop_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executable = dir.path().join("msb");
        let calls = dir.path().join("calls");
        let first_remove = dir.path().join("first-remove");
        let sandbox = dir.path().join("sandbox");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = create ]; then printf '%s\\n' \"$4\" > '{}'; exit 0; fi\nif [ \"$1\" = list ]; then if [ -s '{}' ]; then IFS= read -r name < '{}'; printf '%s\\n' \"$name\"; fi; exit 0; fi\nif [ \"$1\" = remove ] && [ ! -e '{}' ]; then touch '{}'; exit 9; fi\nif [ \"$1\" = remove ]; then rm -f '{}'; exit 0; fi\nexit 0\n",
            calls.display(),
            sandbox.display(),
            sandbox.display(),
            sandbox.display(),
            first_remove.display(),
            first_remove.display(),
            sandbox.display()
        );
        std::fs::write(&executable, script).expect("write fake msb");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");
        let config = config(executable);
        let (_command, guard) = runner_command(&spawn(), Some(&config), None)
            .await
            .expect("create succeeds");
        let mut guard = guard.expect("guard");
        guard.remove().await;
        drop(guard);

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let recorded = std::fs::read_to_string(&calls).expect("recorded calls");
            if recorded
                .lines()
                .filter(|line| line.starts_with("remove "))
                .count()
                >= 2
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "fallback did not retry: {recorded}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn repeated_cleanup_failure_opens_creation_circuit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executable = dir.path().join("msb");
        let calls = dir.path().join("calls");
        let sandbox = dir.path().join("sandbox");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = create ]; then printf '%s\\n' \"$4\" > '{}'; exit 0; fi\nif [ \"$1\" = list ]; then if [ -s '{}' ]; then IFS= read -r name < '{}'; printf '%s\\n' \"$name\"; fi; exit 0; fi\nif [ \"$1\" = remove ]; then exit 9; fi\nexit 0\n",
            calls.display(),
            sandbox.display(),
            sandbox.display(),
            sandbox.display()
        );
        std::fs::write(&executable, script).expect("write fake msb");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");
        let config = config(executable);
        let (_command, guard) = runner_command(&spawn(), Some(&config), None)
            .await
            .expect("initial create succeeds");
        let mut guard = guard.expect("guard");
        guard.remove().await;
        drop(guard);

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let remove_count = std::fs::read_to_string(&calls)
                .expect("recorded calls")
                .lines()
                .filter(|line| line.starts_with("remove "))
                .count();
            if remove_count >= 2 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                break;
            }
            assert!(Instant::now() < deadline, "fallback remove did not run");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let error = runner_command(&spawn(), Some(&config), None)
            .await
            .err()
            .expect("creation must fail closed");
        assert_eq!(error.kind(), "cleanup_failed");
        assert!(error.to_string().contains("cleanup(s) remain unresolved"));
    }
}
