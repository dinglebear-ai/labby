//! Local system probes for `system.checks`.
//!
//! All file and env I/O lives here, never in `labby-apis`.

use super::types::{Finding, Severity, service_env_checks};
use futures::StreamExt as _;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DOCTOR_PROBE_CONCURRENCY: usize = 5;
const DOCTOR_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const DOCTOR_AGGREGATE_TIMEOUT: Duration = Duration::from_secs(10);
const DOCTOR_PROCESS_PROBE_GLOBAL_CONCURRENCY: usize = 5;
static DOCTOR_PROCESS_PROBE_BUDGET: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

fn process_probe_budget() -> Arc<tokio::sync::Semaphore> {
    Arc::clone(DOCTOR_PROCESS_PROBE_BUDGET.get_or_init(|| {
        Arc::new(tokio::sync::Semaphore::new(
            DOCTOR_PROCESS_PROBE_GLOBAL_CONCURRENCY,
        ))
    }))
}

#[cfg(test)]
struct BlockingProbe {
    check: String,
    task: Box<dyn FnOnce(Arc<AtomicBool>) -> Finding + Send + 'static>,
}

#[cfg(test)]
impl BlockingProbe {
    fn new(
        check: impl Into<String>,
        task: impl FnOnce(Arc<AtomicBool>) -> Finding + Send + 'static,
    ) -> Self {
        Self {
            check: check.into(),
            task: Box::new(task),
        }
    }
}

#[derive(Clone, Copy)]
struct ProbeLimits {
    concurrency: usize,
    per_probe: Duration,
    aggregate: Duration,
}

struct ProbeCancellationGuard(Arc<AtomicBool>);

impl Drop for ProbeCancellationGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[cfg(test)]
async fn run_bounded_probes(probes: Vec<BlockingProbe>, limits: ProbeLimits) -> Vec<Finding> {
    let probe_count = probes.len();
    let probe_names: Vec<String> = probes.iter().map(|probe| probe.check.clone()).collect();
    let cancellation = Arc::new(AtomicBool::new(false));
    let _cancellation_guard = ProbeCancellationGuard(Arc::clone(&cancellation));
    let mut results = vec![None; probe_count];
    let mut pending = futures::stream::iter(probes.into_iter().enumerate())
        .map(|(index, probe)| {
            let cancellation = Arc::clone(&cancellation);
            async move {
                let check = probe.check;
                if cancellation.load(Ordering::Acquire) {
                    return (index, probe_failure(&check, "probe cancelled".into()));
                }
                let task_cancellation = Arc::clone(&cancellation);
                let mut task = tokio::task::spawn_blocking(move || (probe.task)(task_cancellation));
                let finding = match tokio::time::timeout(limits.per_probe, &mut task).await {
                    Ok(Ok(finding)) => finding,
                    Ok(Err(error)) => {
                        probe_failure(&check, format!("probe task panicked: {error}"))
                    }
                    Err(_) => {
                        cancellation.store(true, Ordering::Release);
                        // `spawn_blocking` tasks cannot be aborted safely.  Every
                        // production probe receives the cancellation token, so
                        // retain ownership and join it before the audit returns;
                        // dropping the handle here would detach filesystem or
                        // subprocess work into the runtime's blocking pool.
                        drop(task.await);
                        probe_failure(
                            &check,
                            format!(
                                "probe timed out after {:.3}s",
                                limits.per_probe.as_secs_f64()
                            ),
                        )
                    }
                };
                (index, finding)
            }
        })
        .buffer_unordered(limits.concurrency.max(1));
    let aggregate_deadline = tokio::time::Instant::now() + limits.aggregate;
    loop {
        match tokio::time::timeout_at(aggregate_deadline, pending.next()).await {
            Ok(Some((index, finding))) => results[index] = Some(finding),
            Ok(None) => break,
            Err(_) => {
                cancellation.store(true, Ordering::Release);
                break;
            }
        }
    }
    if cancellation.load(Ordering::Acquire) {
        // Do not detach already-started blocking probes. Their cooperative
        // cancellation paths must finish and be joined before returning.
        while let Some((index, finding)) = pending.next().await {
            results[index] = Some(finding);
        }
    }
    drop(pending);
    results
        .into_iter()
        .enumerate()
        .map(|(index, finding)| {
            finding.unwrap_or_else(|| {
                probe_failure(
                    &probe_names[index],
                    format!(
                        "doctor aggregate timed out after {:.3}s",
                        limits.aggregate.as_secs_f64()
                    ),
                )
            })
        })
        .collect()
}

struct ProcessProbe {
    check: String,
    command: std::process::Command,
    success: Finding,
    failure: Finding,
    #[cfg(test)]
    before_run: Option<Box<dyn FnOnce() + Send + 'static>>,
    #[cfg(test)]
    after_run: Option<Box<dyn FnOnce() + Send + 'static>>,
}

async fn run_bounded_process_probes(
    probes: Vec<ProcessProbe>,
    limits: ProbeLimits,
) -> Vec<Finding> {
    run_bounded_process_probes_with_budget(probes, limits, process_probe_budget()).await
}

async fn run_bounded_process_probes_with_budget(
    probes: Vec<ProcessProbe>,
    limits: ProbeLimits,
    budget: Arc<tokio::sync::Semaphore>,
) -> Vec<Finding> {
    let probe_count = probes.len();
    let probe_names: Vec<String> = probes.iter().map(|probe| probe.check.clone()).collect();
    let cancellation = Arc::new(AtomicBool::new(false));
    let _cancellation_guard = ProbeCancellationGuard(Arc::clone(&cancellation));
    let mut results = vec![None; probe_count];
    let deadline = tokio::time::Instant::now() + limits.aggregate;
    let mut pending = futures::stream::iter(probes.into_iter().enumerate())
        .map(|(index, mut probe)| {
            let cancellation = Arc::clone(&cancellation);
            let budget = Arc::clone(&budget);
            async move {
                let check = probe.check;
                let permit = match tokio::time::timeout_at(deadline, budget.acquire_owned()).await {
                    Ok(Ok(permit)) => permit,
                    Err(_) => {
                        return (
                            index,
                            probe_failure(
                                &check,
                                format!(
                                    "doctor aggregate timed out after {:.3}s",
                                    limits.aggregate.as_secs_f64()
                                ),
                            ),
                        );
                    }
                    Ok(Err(_)) => {
                        return (
                            index,
                            probe_failure(&check, "probe admission closed".into()),
                        );
                    }
                };
                if cancellation.load(Ordering::Acquire) || tokio::time::Instant::now() >= deadline {
                    drop(permit);
                    return (index, probe_failure(&check, "probe cancelled".into()));
                }
                let task_cancellation = Arc::clone(&cancellation);
                let mut task = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    #[cfg(test)]
                    if let Some(before_run) = probe.before_run.take() {
                        before_run();
                    }
                    let finding =
                        if cancellable_command_status(&mut probe.command, &task_cancellation) {
                            probe.success
                        } else {
                            probe.failure
                        };
                    #[cfg(test)]
                    if let Some(after_run) = probe.after_run.take() {
                        after_run();
                    }
                    finding
                });
                let finding = match tokio::time::timeout(limits.per_probe, &mut task).await {
                    Ok(Ok(finding)) => finding,
                    Ok(Err(error)) => {
                        probe_failure(&check, format!("probe task panicked: {error}"))
                    }
                    Err(_) => {
                        cancellation.store(true, Ordering::Release);
                        // The process probe owns a cancellation-aware child and
                        // cannot outlive its audit. Awaiting the blocking task is
                        // mandatory: dropping this handle would detach the child
                        // cleanup into Tokio's blocking pool.
                        drop(task.await);
                        probe_failure(
                            &check,
                            format!(
                                "probe timed out after {:.3}s",
                                limits.per_probe.as_secs_f64()
                            ),
                        )
                    }
                };
                (index, finding)
            }
        })
        .buffer_unordered(limits.concurrency.max(1));
    while let Ok(Some((index, finding))) = tokio::time::timeout_at(deadline, pending.next()).await {
        results[index] = Some(finding);
    }
    cancellation.store(true, Ordering::Release);
    // Every started process probe has a closed kill/reap path. Drain all of
    // them after cancellation so no JoinHandle or child can be detached.
    while let Some((index, finding)) = pending.next().await {
        results[index] = Some(finding);
    }
    drop(pending);
    results
        .into_iter()
        .enumerate()
        .map(|(index, finding)| {
            finding.unwrap_or_else(|| {
                probe_failure(
                    &probe_names[index],
                    format!(
                        "doctor aggregate timed out after {:.3}s",
                        limits.aggregate.as_secs_f64()
                    ),
                )
            })
        })
        .collect()
}

fn probe_failure(check: &str, message: String) -> Finding {
    Finding {
        service: "system".into(),
        check: check.into(),
        severity: Severity::Fail,
        message,
    }
}

fn process_probe(
    service: &str,
    check: &str,
    command: std::process::Command,
    success_message: String,
    failure_severity: Severity,
    failure_message: String,
) -> ProcessProbe {
    ProcessProbe {
        check: check.into(),
        command,
        success: Finding {
            service: service.into(),
            check: check.into(),
            severity: Severity::Ok,
            message: success_message,
        },
        failure: Finding {
            service: service.into(),
            check: check.into(),
            severity: failure_severity,
            message: failure_message,
        },
        #[cfg(test)]
        before_run: None,
        #[cfg(test)]
        after_run: None,
    }
}

fn path_test_command(path: &str, writable: bool) -> std::process::Command {
    #[cfg(unix)]
    {
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            if writable {
                "test -w \"$1\""
            } else {
                "test -e \"$1\""
            },
            "sh",
            path,
        ]);
        command
    }
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("powershell.exe");
        let expression = if writable {
            "if (Test-Path -LiteralPath $args[0]) { try { [IO.File]::Open($args[0], 'Open', 'Read', 'ReadWrite').Dispose(); exit 0 } catch { exit 1 } } else { exit 1 }"
        } else {
            "if (Test-Path -LiteralPath $args[0]) { exit 0 } else { exit 1 }"
        };
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            expression,
            path,
        ]);
        command
    }
}

fn executable_test_command(program: &str, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.args(args);
    command
}

fn command_available_command(program: &str) -> std::process::Command {
    #[cfg(unix)]
    let mut command = std::process::Command::new("which");
    #[cfg(windows)]
    let mut command = std::process::Command::new("where.exe");
    command.arg(program);
    command
}

fn backup_retention_command(config_path: &str) -> std::process::Command {
    #[cfg(unix)]
    {
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            "dir=${1%/*}; base=${1##*/}; set -- \"$dir/$base\".bak.*; test ! -e \"$1\" && exit 0; count=$#; bytes=$(du -ck \"$@\" | awk 'END {print $1 * 1024}'); test \"$count\" -le 10 && test \"$bytes\" -le 67108864",
            "sh",
            config_path,
        ]);
        command
    }
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$p=[IO.FileInfo]$args[0]; $b=@(Get-ChildItem -LiteralPath $p.DirectoryName -Filter ($p.Name + '.bak.*') -File); $n=($b | Measure-Object).Count; $s=($b | Measure-Object Length -Sum).Sum; if ($n -le 10 -and $s -le 67108864) { exit 0 } else { exit 1 }",
            config_path,
        ]);
        command
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(dead_code)]
fn path_check(service: &str, label: &str, path: &str, severity_on_missing: Severity) -> Finding {
    let exists = std::path::Path::new(path).exists();
    Finding {
        service: service.to_string(),
        check: label.to_string(),
        severity: if exists {
            Severity::Ok
        } else {
            severity_on_missing
        },
        message: if exists {
            format!("{path} found")
        } else {
            format!("{path} not found")
        },
    }
}

#[cfg(test)]
fn writable_check(service: &str, label: &str, path: &str) -> Finding {
    let path_obj = std::path::Path::new(path);
    if !path_obj.exists() {
        return Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Warn,
            message: format!("{path} not found; cannot check writability"),
        };
    }

    let result = if path_obj.is_dir() {
        let test_path = path_obj.join(".doctor_write_test");
        std::fs::write(&test_path, "test").inspect(|_| {
            drop(std::fs::remove_file(test_path));
        })
    } else {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path_obj)
            .map(|_| ())
    };

    match result {
        Ok(()) => Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Ok,
            message: format!("{path} is writable"),
        },
        Err(e) => Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Fail,
            message: format!("{path} is NOT writable: {e}"),
        },
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn config_backup_check(config_path: &str) -> Finding {
    let path = std::path::Path::new(config_path);
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let prefix = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name}.bak."))
        .unwrap_or_default();
    let backups = std::fs::read_dir(parent)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let bytes = backups
        .iter()
        .filter_map(|entry| entry.metadata().ok())
        .fold(0_u64, |total, metadata| {
            total.saturating_add(metadata.len())
        });
    let healthy = backups.len() <= 10 && bytes <= 64 * 1024 * 1024;
    Finding {
        service: "system".into(),
        check: "config:backup-retention".into(),
        severity: if healthy {
            Severity::Ok
        } else {
            Severity::Warn
        },
        message: if healthy {
            format!(
                "{} retained config backup(s), {} bytes",
                backups.len(),
                bytes
            )
        } else {
            format!(
                "{} config backups remain after retention; preserve the newest recovery point and remove older files after verifying config.toml",
                backups.len()
            )
        },
    }
}

fn terminate_probe_process(child: &mut std::process::Child) {
    drop(child.kill());
    drop(child.wait());
}

#[cfg(unix)]
struct ProbeProcessGuard {
    process_group: i32,
}

#[cfg(unix)]
impl Drop for ProbeProcessGuard {
    fn drop(&mut self) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(self.process_group),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

#[cfg(unix)]
fn probe_process_guard(child: &mut std::process::Child) -> Option<ProbeProcessGuard> {
    i32::try_from(child.id())
        .ok()
        .map(|process_group| ProbeProcessGuard { process_group })
}

#[cfg(windows)]
struct ProbeProcessGuard {
    _job: labby_winjob::JobObject,
}

#[cfg(windows)]
fn probe_process_guard(child: &mut std::process::Child) -> Option<ProbeProcessGuard> {
    labby_winjob::JobObject::assign(child.id())
        .map(|job| ProbeProcessGuard { _job: job })
        .ok()
}

#[cfg(not(any(unix, windows)))]
struct ProbeProcessGuard;

#[cfg(not(any(unix, windows)))]
fn probe_process_guard(_child: &mut std::process::Child) -> Option<ProbeProcessGuard> {
    Some(ProbeProcessGuard)
}

fn cancellable_command_status(
    command: &mut std::process::Command,
    cancellation: &AtomicBool,
) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let Some(tree_guard) = probe_process_guard(&mut child) else {
        terminate_probe_process(&mut child);
        return false;
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if cancellation.load(Ordering::Acquire) => {
                drop(tree_guard);
                terminate_probe_process(&mut child);
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                drop(tree_guard);
                terminate_probe_process(&mut child);
                return false;
            }
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn command_check(service: &str, label: &str, cmd: &str, cancellation: &AtomicBool) -> Finding {
    let found =
        cancellable_command_status(std::process::Command::new("which").arg(cmd), cancellation);
    Finding {
        service: service.to_string(),
        check: label.to_string(),
        severity: if found { Severity::Ok } else { Severity::Warn },
        message: if found {
            format!("`{cmd}` is available")
        } else {
            format!("`{cmd}` not found on PATH")
        },
    }
}

/// Verify `docker compose` (the v2 CLI plugin) is actually wired up,
/// not just that the `docker` binary exists.
///
/// Runs `docker compose version` and treats a non-zero exit (or missing
/// binary) as the plugin being unavailable.
#[cfg(test)]
#[allow(dead_code)]
fn compose_plugin_check(cancellation: &AtomicBool) -> Finding {
    let found = cancellable_command_status(
        std::process::Command::new("docker").args(["compose", "version"]),
        cancellation,
    );
    Finding {
        service: "system".to_string(),
        check: "docker:compose-plugin".to_string(),
        severity: if found { Severity::Ok } else { Severity::Warn },
        message: if found {
            "`docker compose` plugin is available".to_string()
        } else {
            "`docker compose` plugin not available".to_string()
        },
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Run all local system probes: env-var checks, config files, Docker, disk.
///
/// Order: env-var checks first (preserves current `labby doctor` output), then
/// system-level checks.
pub async fn run_system_checks() -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();

    // --- Env var checks (current labby doctor behaviour; preserved for output parity) ---
    for (service_name, required_env) in service_env_checks() {
        for env in required_env {
            let present = std::env::var(env.name).is_ok_and(|v| !v.is_empty());
            findings.push(Finding {
                service: service_name.into(),
                check: format!("env:{}", env.name),
                severity: if present {
                    Severity::Ok
                } else {
                    Severity::Fail
                },
                message: if present {
                    format!("{} is set", env.name)
                } else {
                    format!("{} is missing ({})", env.name, env.description)
                },
            });
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let env_path = format!("{home}/.labby/.env");
    let lab_dir = format!("{home}/.labby");
    let config_path = format!("{home}/.labby/config.toml");
    let mut probes = Vec::new();
    probes.push(process_probe(
        "lab",
        "config:~/.labby/.env",
        path_test_command(&env_path, false),
        format!("{env_path} found"),
        Severity::Warn,
        format!("{env_path} not found"),
    ));
    probes.push(process_probe(
        "lab",
        "config:~/.labby/.env:writable",
        path_test_command(&env_path, true),
        format!("{env_path} is writable"),
        Severity::Fail,
        format!("{env_path} is NOT writable or is missing"),
    ));
    probes.push(process_probe(
        "lab",
        "config:~/.labby:writable",
        path_test_command(&lab_dir, true),
        format!("{lab_dir} is writable"),
        Severity::Fail,
        format!("{lab_dir} is NOT writable or is missing"),
    ));
    probes.push(process_probe(
        "lab",
        "config:~/.labby/config.toml",
        path_test_command(&config_path, false),
        format!("{config_path} found"),
        Severity::Warn,
        format!("{config_path} not found"),
    ));
    probes.push(process_probe(
        "system",
        "config:backup-retention",
        backup_retention_command(&config_path),
        "config backup retention is within count and byte budgets".into(),
        Severity::Warn,
        "config backups exceed the count or byte retention budget".into(),
    ));

    for (name, rel_path) in [
        (".claude", "claude"),
        (".codex", "codex"),
        (".gemini", "gemini"),
    ] {
        let full = format!("{home}/{name}");
        probes.push(process_probe(
            "lab",
            &format!("config:~/{name}"),
            path_test_command(&full, false),
            format!("~/{name} present ({rel_path} detected)"),
            Severity::Ok,
            format!("~/{name} not present"),
        ));
    }
    probes.push(process_probe(
        "system",
        "docker:socket",
        path_test_command("/var/run/docker.sock", false),
        "/var/run/docker.sock found".into(),
        Severity::Warn,
        "/var/run/docker.sock not found".into(),
    ));
    probes.push(process_probe(
        "system",
        "docker:cli",
        command_available_command("docker"),
        "`docker` is available".into(),
        Severity::Warn,
        "`docker` not found on PATH".into(),
    ));
    probes.push(process_probe(
        "system",
        "docker:compose-plugin",
        executable_test_command("docker", &["compose", "version"]),
        "`docker compose` plugin is available".into(),
        Severity::Warn,
        "`docker compose` plugin not available".into(),
    ));
    probes.push(process_probe(
        "system",
        "rust:cargo",
        command_available_command("cargo"),
        "`cargo` is available".into(),
        Severity::Warn,
        "`cargo` not found on PATH".into(),
    ));
    let mut disk_command = std::process::Command::new("sh");
    disk_command.args(["-c", "used=$(df -P / | awk 'NR==2 {gsub(/%/, \"\", $5); print $5}'); test -n \"$used\" && test \"$used\" -lt 90"]);
    probes.push(process_probe(
        "system",
        "disk:/",
        disk_command,
        "/ disk use is below 90%".into(),
        Severity::Warn,
        "/ disk use is at least 90% or could not be determined".into(),
    ));
    findings.extend(
        run_bounded_process_probes(
            probes,
            ProbeLimits {
                concurrency: DOCTOR_PROBE_CONCURRENCY,
                per_probe: DOCTOR_PROBE_TIMEOUT,
                aggregate: DOCTOR_AGGREGATE_TIMEOUT,
            },
        )
        .await,
    );
    findings
}

#[cfg(target_os = "linux")]
#[cfg(test)]
#[allow(dead_code)]
fn disk_check(findings: &mut Vec<Finding>, cancellation: &AtomicBool) {
    let below_warning_threshold = cancellable_command_status(
        std::process::Command::new("sh").args([
            "-c",
            "used=$(df -P / | awk 'NR==2 {gsub(/%/, \"\", $5); print $5}'); test -n \"$used\" && test \"$used\" -lt 90",
        ]),
        cancellation,
    );
    findings.push(Finding {
        service: "system".into(),
        check: "disk:/".into(),
        severity: if below_warning_threshold {
            Severity::Ok
        } else {
            Severity::Warn
        },
        message: if below_warning_threshold {
            "/ disk use is below 90%".into()
        } else {
            "/ disk use is at least 90% or could not be determined".into()
        },
    });
}

#[cfg(not(target_os = "linux"))]
#[cfg(test)]
#[allow(dead_code)]
fn disk_check(_findings: &mut Vec<Finding>, _cancellation: &AtomicBool) {}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use std::thread;

    #[cfg(unix)]
    static PROCESS_PROBE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(unix)]
    #[test]
    fn cancellable_command_kills_a_hanging_process_group() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            trigger.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        let status = cancellable_command_status(
            std::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 30 & wait"),
            &cancelled,
        );

        assert!(!status);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_probe_runner_cancels_and_reaps_a_noncooperative_child() {
        let _serial = PROCESS_PROBE_TEST_LOCK.lock().await;
        let probe = process_probe(
            "system",
            "hung-process",
            executable_test_command("sh", &["-c", "sleep 30 & wait"]),
            "unexpected success".into(),
            Severity::Fail,
            "cancelled".into(),
        );
        let started = std::time::Instant::now();
        let findings = run_bounded_process_probes(
            vec![probe],
            ProbeLimits {
                concurrency: 1,
                per_probe: Duration::from_millis(50),
                aggregate: Duration::from_millis(100),
            },
        )
        .await;

        assert!(started.elapsed() < Duration::from_millis(400));
        assert_eq!(findings.len(), 1);
        assert!(matches!(findings[0].severity, Severity::Fail));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_probe_runner_never_detaches_cleanup_after_timeout() {
        let _serial = PROCESS_PROBE_TEST_LOCK.lock().await;
        use std::sync::atomic::{AtomicBool, Ordering};

        let cleanup_finished = Arc::new(AtomicBool::new(false));
        let cleanup_finished_for_task = Arc::clone(&cleanup_finished);
        let mut probe = process_probe(
            "system",
            "mandatory-join",
            executable_test_command("sh", &["-c", "sleep 30 & wait"]),
            "unexpected success".into(),
            Severity::Fail,
            "cancelled".into(),
        );
        probe.after_run = Some(Box::new(move || {
            thread::sleep(Duration::from_millis(350));
            cleanup_finished_for_task.store(true, Ordering::Release);
        }));

        let findings = run_bounded_process_probes(
            vec![probe],
            ProbeLimits {
                concurrency: 1,
                per_probe: Duration::from_millis(20),
                aggregate: Duration::from_millis(40),
            },
        )
        .await;

        assert_eq!(findings.len(), 1);
        assert!(
            cleanup_finished.load(Ordering::Acquire),
            "the audit returned while its blocking probe cleanup was detached"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_audits_share_one_process_wide_probe_limit() {
        let _serial = PROCESS_PROBE_TEST_LOCK.lock().await;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const EXPECTED_GLOBAL_LIMIT: usize = 5;
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let budget = Arc::new(tokio::sync::Semaphore::new(EXPECTED_GLOBAL_LIMIT));
        let mut audits = Vec::new();
        for audit in 0..4 {
            let mut probes = Vec::new();
            for probe_index in 0..5 {
                let mut probe = process_probe(
                    "system",
                    &format!("audit-{audit}-probe-{probe_index}"),
                    executable_test_command("sh", &["-c", "sleep 0.1"]),
                    "ok".into(),
                    Severity::Fail,
                    "failed".into(),
                );
                let active_before = Arc::clone(&active);
                let peak_before = Arc::clone(&peak);
                probe.before_run = Some(Box::new(move || {
                    let current = active_before.fetch_add(1, Ordering::SeqCst) + 1;
                    peak_before.fetch_max(current, Ordering::SeqCst);
                }));
                let active_after = Arc::clone(&active);
                probe.after_run = Some(Box::new(move || {
                    active_after.fetch_sub(1, Ordering::SeqCst);
                }));
                probes.push(probe);
            }
            audits.push(tokio::spawn(run_bounded_process_probes_with_budget(
                probes,
                ProbeLimits {
                    concurrency: 5,
                    per_probe: Duration::from_secs(1),
                    aggregate: Duration::from_secs(2),
                },
                Arc::clone(&budget),
            )));
        }
        for audit in audits {
            assert_eq!(audit.await.unwrap().len(), 5);
        }
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(
            peak.load(Ordering::SeqCst) <= EXPECTED_GLOBAL_LIMIT,
            "concurrent audits launched {} process probes",
            peak.load(Ordering::SeqCst)
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_an_audit_cancels_and_joins_its_active_process_probe() {
        let _serial = PROCESS_PROBE_TEST_LOCK.lock().await;
        use std::sync::atomic::{AtomicBool, Ordering};

        let started = Arc::new(AtomicBool::new(false));
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let mut probe = process_probe(
            "system",
            "disconnect",
            executable_test_command("sh", &["-c", "sleep 0.5"]),
            "unexpected success".into(),
            Severity::Fail,
            "cancelled".into(),
        );
        let started_for_probe = Arc::clone(&started);
        probe.before_run = Some(Box::new(move || {
            started_for_probe.store(true, Ordering::Release);
        }));
        probe.after_run = Some(Box::new(move || {
            let _ = finished_tx.send(());
        }));

        let audit = tokio::spawn(run_bounded_process_probes(
            vec![probe],
            ProbeLimits {
                concurrency: 1,
                per_probe: Duration::from_secs(1),
                aggregate: Duration::from_secs(1),
            },
        ));
        tokio::time::timeout(Duration::from_millis(200), async {
            while !started.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        audit.abort();
        drop(audit.await);
        tokio::time::timeout(Duration::from_secs(1), finished_rx)
            .await
            .expect("dropping the audit detached its active process probe")
            .expect("probe cleanup dropped its completion signal");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probes_queued_past_deadline_return_without_starting() {
        let _serial = PROCESS_PROBE_TEST_LOCK.lock().await;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let budget = Arc::new(tokio::sync::Semaphore::new(
            DOCTOR_PROCESS_PROBE_GLOBAL_CONCURRENCY,
        ));
        let held = Arc::clone(&budget)
            .acquire_many_owned(DOCTOR_PROCESS_PROBE_GLOBAL_CONCURRENCY as u32)
            .await
            .unwrap();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            drop(held);
        });
        let started = Arc::new(AtomicUsize::new(0));
        let mut probe = process_probe(
            "system",
            "queued",
            executable_test_command("sh", &["-c", "true"]),
            "ok".into(),
            Severity::Fail,
            "failed".into(),
        );
        let started_for_probe = Arc::clone(&started);
        probe.before_run = Some(Box::new(move || {
            started_for_probe.fetch_add(1, Ordering::SeqCst);
        }));

        let began = tokio::time::Instant::now();
        let findings = run_bounded_process_probes_with_budget(
            vec![probe],
            ProbeLimits {
                concurrency: 1,
                per_probe: Duration::from_secs(1),
                aggregate: Duration::from_millis(50),
            },
            budget,
        )
        .await;

        assert!(began.elapsed() < Duration::from_millis(150));
        assert_eq!(started.load(Ordering::SeqCst), 0);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("aggregate timed out"));
        release.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_probe_runner_honors_single_worker_limit() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let probes = (0..10)
            .map(|index| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                BlockingProbe::new(format!("probe-{index}"), move |_| {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(2));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Finding {
                        service: "test".into(),
                        check: format!("probe-{index}"),
                        severity: Severity::Ok,
                        message: "ok".into(),
                    }
                })
            })
            .collect();

        let findings = run_bounded_probes(
            probes,
            ProbeLimits {
                concurrency: 1,
                per_probe: Duration::from_secs(1),
                aggregate: Duration::from_secs(1),
            },
        )
        .await;

        assert_eq!(findings.len(), 10);
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_probe_runner_returns_one_ordered_result_for_one_hundred_probes() {
        let probes = (0..100)
            .map(|index| {
                BlockingProbe::new(format!("probe-{index:03}"), move |_| Finding {
                    service: "test".into(),
                    check: format!("probe-{index:03}"),
                    severity: Severity::Ok,
                    message: "ok".into(),
                })
            })
            .collect();

        let findings = run_bounded_probes(
            probes,
            ProbeLimits {
                concurrency: 5,
                per_probe: Duration::from_secs(1),
                aggregate: Duration::from_secs(2),
            },
        )
        .await;

        assert_eq!(findings.len(), 100);
        assert_eq!(findings[0].check, "probe-000");
        assert_eq!(findings[99].check, "probe-099");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_probe_runner_surfaces_per_probe_timeout() {
        let probes = vec![BlockingProbe::new("hung", |cancelled| {
            while !cancelled.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            Finding {
                service: "test".into(),
                check: "hung".into(),
                severity: Severity::Ok,
                message: "late".into(),
            }
        })];

        let findings = run_bounded_probes(
            probes,
            ProbeLimits {
                concurrency: 1,
                per_probe: Duration::from_millis(10),
                aggregate: Duration::from_millis(50),
            },
        )
        .await;

        assert_eq!(findings.len(), 1);
        assert!(matches!(findings[0].severity, Severity::Fail));
        assert!(findings[0].message.contains("timed out"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bounded_probe_runner_cancels_and_drains_active_work_at_aggregate_deadline() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let active = Arc::new(AtomicUsize::new(0));
        let probes = (0..100)
            .map(|index| {
                let active = Arc::clone(&active);
                BlockingProbe::new(format!("hung-{index}"), move |cancelled| {
                    active.fetch_add(1, Ordering::SeqCst);
                    while !cancelled.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(1));
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    probe_failure(&format!("hung-{index}"), "cancelled".into())
                })
            })
            .collect();

        let started_at = std::time::Instant::now();
        let findings = run_bounded_probes(
            probes,
            ProbeLimits {
                concurrency: 5,
                per_probe: Duration::from_secs(1),
                aggregate: Duration::from_millis(20),
            },
        )
        .await;

        assert_eq!(findings.len(), 100);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert!(
            started_at.elapsed() >= Duration::from_millis(20),
            "aggregate must not return before cancellation and worker join"
        );
        assert!(
            findings
                .iter()
                .all(|finding| matches!(finding.severity, Severity::Fail))
        );
    }

    #[test]
    fn writable_check_warns_when_target_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let finding = writable_check(
            "lab",
            "config:missing:writable",
            dir.path().join("missing.env").to_str().expect("utf8"),
        );
        assert!(matches!(finding.severity, Severity::Warn));
    }

    #[test]
    fn writable_check_accepts_writable_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let finding = writable_check("lab", "config:dir:writable", dir.path().to_str().unwrap());
        assert!(matches!(finding.severity, Severity::Ok));
        assert!(!dir.path().join(".doctor_write_test").exists());
    }

    #[cfg(unix)]
    #[test]
    fn writable_check_tests_actual_file_not_sibling() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        std::fs::write(&path, "LAB=value\n").expect("write");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o444);
        std::fs::set_permissions(&path, permissions).expect("readonly");

        let finding = writable_check("lab", "config:file:writable", path.to_str().unwrap());

        let mut restore = std::fs::metadata(&path).expect("metadata").permissions();
        restore.set_mode(0o644);
        std::fs::set_permissions(&path, restore).expect("restore");

        assert!(matches!(finding.severity, Severity::Fail));
    }
}

// ---------------------------------------------------------------------------
// Auth / OAuth checks
// ---------------------------------------------------------------------------

/// Build an auth-namespace `Finding`. All `auth:*` checks share `service = "auth"`.
fn auth_finding(check: &str, severity: Severity, message: impl Into<String>) -> Finding {
    Finding {
        service: "auth".into(),
        check: check.into(),
        severity,
        message: message.into(),
    }
}

/// Severity + message for an env var that is required when OAuth is enabled.
///
/// - set + valid → Ok
/// - set + invalid → Fail (caller validates and supplies `invalid_message`)
/// - missing + oauth → Fail
/// - missing + non-oauth → Warn
fn oauth_required_env(
    value: &str,
    is_oauth: bool,
    ok_message: impl Into<String>,
    fail_when_oauth: &str,
    warn_otherwise: &str,
) -> (Severity, String) {
    if !value.is_empty() {
        (Severity::Ok, ok_message.into())
    } else if is_oauth {
        (Severity::Fail, fail_when_oauth.to_string())
    } else {
        (Severity::Warn, warn_otherwise.to_string())
    }
}

/// Run auth/OAuth configuration probes.
///
/// Checks env vars, file presence, and Unix file permissions.
/// No network I/O — all checks are local and synchronous.
pub fn run_auth_checks() -> Vec<Finding> {
    let resolved = crate::config::toml_candidates()
        .and_then(|candidates| crate::config::load_toml(&candidates))
        .and_then(|config| crate::config::resolve_auth_for_config(&config));
    run_auth_checks_with_config(resolved.as_ref().ok())
}

pub fn run_auth_checks_with_config(
    config: Option<&labby_auth::config::AuthConfig>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let home = std::env::var("HOME").unwrap_or_default();

    let mode = config.map_or_else(
        || {
            std::env::var("LABBY_AUTH_MODE")
                .unwrap_or_default()
                .to_lowercase()
        },
        |config| match config.mode {
            labby_auth::config::AuthMode::OAuth => "oauth".into(),
            labby_auth::config::AuthMode::Bearer => "bearer".into(),
        },
    );
    let is_oauth = mode == "oauth";
    let provider = config
        .and_then(|config| config.inbound_provider)
        .map_or_else(
            || {
                std::env::var("LABBY_AUTH_PROVIDER")
                    .unwrap_or_else(|_| "google".into())
                    .to_lowercase()
            },
            |provider| match provider {
                labby_auth::config::InboundProviderKind::Google => "google".into(),
                labby_auth::config::InboundProviderKind::Authelia => "authelia".into(),
            },
        );
    let uses_google = provider != "authelia";

    let bearer_token = std::env::var("LABBY_MCP_HTTP_TOKEN").unwrap_or_default();
    let google_id = config.map_or_else(
        || std::env::var("LABBY_GOOGLE_CLIENT_ID").unwrap_or_default(),
        |config| config.google.client_id.clone(),
    );
    let google_secret = config.map_or_else(
        || std::env::var("LABBY_GOOGLE_CLIENT_SECRET").unwrap_or_default(),
        |config| config.google.client_secret.clone(),
    );
    let has_google = !google_id.is_empty() && !google_secret.is_empty();
    let authelia_issuer = config
        .and_then(|config| config.authelia.as_ref())
        .map_or_else(
            || std::env::var("LABBY_AUTHELIA_ISSUER_URL").unwrap_or_default(),
            |config| config.issuer_url.to_string(),
        );
    let authelia_id = config
        .and_then(|config| config.authelia.as_ref())
        .map_or_else(
            || std::env::var("LABBY_AUTHELIA_CLIENT_ID").unwrap_or_default(),
            |config| config.client_id.clone(),
        );
    let authelia_secret = config
        .and_then(|config| config.authelia.as_ref())
        .map_or_else(
            || std::env::var("LABBY_AUTHELIA_CLIENT_SECRET").unwrap_or_default(),
            |config| config.client_secret.clone(),
        );

    let provider_valid = matches!(provider.as_str(), "google" | "authelia");
    findings.push(auth_finding(
        "auth:provider",
        if provider_valid {
            Severity::Ok
        } else {
            Severity::Fail
        },
        if provider_valid {
            format!("selected inbound provider: {provider}")
        } else {
            "LABBY_AUTH_PROVIDER is invalid; expected google or authelia".to_string()
        },
    ));
    let access_ttl = config
        .map(|config| config.access_token_ttl.as_secs())
        .or_else(|| {
            std::env::var("LABBY_AUTH_ACCESS_TOKEN_TTL_SECS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(3_600);
    findings.push(auth_finding(
        "auth:access-token-window",
        Severity::Ok,
        format!("issued access tokens can retain authority for at most {access_ttl} seconds after policy or provider changes"),
    ));
    let config_fingerprint = config
        .and_then(|config| config.inbound_provider_fingerprint().ok())
        .unwrap_or_else(|| "unresolved".to_string());
    findings.push(auth_finding(
        "auth:provider-config-fingerprint",
        Severity::Ok,
        format!(
            "provider config fingerprint: {}",
            config_fingerprint.chars().take(16).collect::<String>()
        ),
    ));

    // --- Auth mode ---
    let mode_label = match mode.as_str() {
        "oauth" => "oauth",
        "bearer" => "bearer",
        _ => "auto (defaulting to bearer)",
    };
    findings.push(auth_finding(
        "auth:mode",
        Severity::Ok,
        format!("LABBY_AUTH_MODE={mode_label}"),
    ));

    // --- Safety gate ---
    let web_ui_auth_disabled = match crate::config::resolve_web_ui_auth_disabled_env() {
        Ok(setting) => setting,
        Err(error) => {
            findings.push(auth_finding(
                "auth:web-ui-auth-disabled",
                Severity::Fail,
                format!("{error}"),
            ));
            None
        }
    };
    let web_ui_auth_disabled_source = web_ui_auth_disabled
        .map_or(crate::config::WEB_UI_AUTH_DISABLED_ENV, |setting| {
            setting.source
        });
    let web_ui_auth_disabled_value = web_ui_auth_disabled.is_some_and(|setting| setting.disabled);
    findings.push(auth_finding(
        "auth:web-ui-auth-disabled",
        if web_ui_auth_disabled_value {
            Severity::Fail
        } else {
            Severity::Ok
        },
        if web_ui_auth_disabled_value {
            format!(
                "{web_ui_auth_disabled_source}=true — /v1/* routes are unprotected (dev only, never in production)"
            )
        } else {
            format!(
                "{} not set (protected mode)",
                crate::config::WEB_UI_AUTH_DISABLED_ENV
            )
        },
    ));

    // --- Bearer token ---
    let (bearer_severity, bearer_message) = if !bearer_token.is_empty() {
        let len = bearer_token.len();
        if len < 32 {
            (
                Severity::Warn,
                format!(
                    "LABBY_MCP_HTTP_TOKEN is set ({len} chars) — too short; regenerate: openssl rand -hex 32"
                ),
            )
        } else {
            (
                Severity::Ok,
                format!("LABBY_MCP_HTTP_TOKEN is set ({len} chars)"),
            )
        }
    } else if is_oauth {
        (
            Severity::Ok,
            "LABBY_MCP_HTTP_TOKEN not set — OAuth-only mode (MCP clients must use the OAuth flow)"
                .into(),
        )
    } else {
        (
            Severity::Fail,
            "LABBY_MCP_HTTP_TOKEN not set — set it or enable OAuth: LABBY_AUTH_MODE=oauth".into(),
        )
    };
    findings.push(auth_finding(
        "auth:bearer-token",
        bearer_severity,
        bearer_message,
    ));

    // --- LABBY_PUBLIC_URL ---
    let public_url = config
        .and_then(|config| config.public_url.as_ref())
        .map_or_else(
            || std::env::var("LABBY_PUBLIC_URL").unwrap_or_default(),
            ToString::to_string,
        );
    let (url_severity, url_message) = if !public_url.is_empty() {
        if public_url.starts_with("http://") || public_url.starts_with("https://") {
            (Severity::Ok, format!("LABBY_PUBLIC_URL={public_url}"))
        } else {
            (
                Severity::Fail,
                format!(
                    "LABBY_PUBLIC_URL={public_url} — not a valid URL (must start with http:// or https://)"
                ),
            )
        }
    } else if is_oauth {
        (
            Severity::Fail,
            "LABBY_PUBLIC_URL not set — required for OAuth (JWT issuer, audience, metadata URLs)"
                .into(),
        )
    } else {
        (
            Severity::Warn,
            "LABBY_PUBLIC_URL not set — required if using LABBY_AUTH_MODE=oauth".into(),
        )
    };
    findings.push(auth_finding("auth:public-url", url_severity, url_message));

    // --- Google credentials ---
    let (gid_severity, gid_message) = oauth_required_env(
        &google_id,
        is_oauth && uses_google,
        "LABBY_GOOGLE_CLIENT_ID is set",
        "LABBY_GOOGLE_CLIENT_ID not set — required for LABBY_AUTH_MODE=oauth",
        "LABBY_GOOGLE_CLIENT_ID not set — required if using LABBY_AUTH_MODE=oauth",
    );
    findings.push(auth_finding(
        "auth:google-client-id",
        gid_severity,
        gid_message,
    ));

    let (gsec_severity, gsec_message) = oauth_required_env(
        &google_secret,
        is_oauth && uses_google,
        "LABBY_GOOGLE_CLIENT_SECRET is set",
        "LABBY_GOOGLE_CLIENT_SECRET not set — required for LABBY_AUTH_MODE=oauth",
        "LABBY_GOOGLE_CLIENT_SECRET not set — required if using LABBY_AUTH_MODE=oauth",
    );
    findings.push(auth_finding(
        "auth:google-client-secret",
        gsec_severity,
        gsec_message,
    ));

    for (id, value, ok, fail, warn) in [
        (
            "auth:authelia-issuer",
            &authelia_issuer,
            "LABBY_AUTHELIA_ISSUER_URL is set",
            "LABBY_AUTHELIA_ISSUER_URL not set — required for the Authelia provider",
            "LABBY_AUTHELIA_ISSUER_URL not set — required only for the Authelia provider",
        ),
        (
            "auth:authelia-client-id",
            &authelia_id,
            "LABBY_AUTHELIA_CLIENT_ID is set",
            "LABBY_AUTHELIA_CLIENT_ID not set — required for the Authelia provider",
            "LABBY_AUTHELIA_CLIENT_ID not set — required only for the Authelia provider",
        ),
        (
            "auth:authelia-client-secret",
            &authelia_secret,
            "LABBY_AUTHELIA_CLIENT_SECRET is set",
            "LABBY_AUTHELIA_CLIENT_SECRET not set — required for the Authelia provider",
            "LABBY_AUTHELIA_CLIENT_SECRET not set — required only for the Authelia provider",
        ),
    ] {
        let (severity, message) =
            oauth_required_env(value, is_oauth && provider == "authelia", ok, fail, warn);
        findings.push(auth_finding(id, severity, message));
    }

    // --- Provider credential and local refresh replay encryption ---
    let token_encryption_key = config
        .and_then(|config| config.token_encryption_key.as_ref())
        .map_or_else(
            || std::env::var("LABBY_TOKEN_ENCRYPTION_KEY").unwrap_or_default(),
            |_| "<resolved-valid-key>".to_string(),
        );
    let (encryption_severity, encryption_message) = if token_encryption_key.trim().is_empty() {
        if is_oauth {
            (
                Severity::Fail,
                "LABBY_TOKEN_ENCRYPTION_KEY not set — required for OAuth credential and refresh replay encryption at rest".to_string(),
            )
        } else {
            (
                Severity::Ok,
                "LABBY_TOKEN_ENCRYPTION_KEY not set — not required by the selected auth provider"
                    .to_string(),
            )
        }
    } else {
        if token_encryption_key == "<resolved-valid-key>" {
            (
                Severity::Ok,
                "LABBY_TOKEN_ENCRYPTION_KEY is resolved and valid".to_string(),
            )
        } else {
            match labby_auth::at_rest::TokenEncryptionKey::from_encoded(&token_encryption_key) {
            Ok(_) => (
                Severity::Ok,
                "LABBY_TOKEN_ENCRYPTION_KEY is set and valid".to_string(),
            ),
            Err(_) => (
                Severity::Fail,
                "LABBY_TOKEN_ENCRYPTION_KEY is invalid — expected 64 hex digits or 43 base64url characters".to_string(),
            ),
        }
        }
    };
    findings.push(auth_finding(
        "auth:token-encryption-key",
        encryption_severity,
        encryption_message,
    ));

    // --- Auth store files (only meaningful when OAuth is configured) ---
    if is_oauth || has_google {
        let sqlite_path = config.map_or_else(
            || {
                std::env::var("LABBY_AUTH_SQLITE_PATH")
                    .unwrap_or_else(|_| format!("{home}/.labby/auth.db"))
            },
            |config| config.sqlite_path.display().to_string(),
        );
        let key_path = config.map_or_else(
            || {
                std::env::var("LABBY_AUTH_KEY_PATH")
                    .unwrap_or_else(|_| format!("{home}/.labby/auth-jwt.pem"))
            },
            |config| config.key_path.display().to_string(),
        );

        let sqlite_exists = std::path::Path::new(&sqlite_path).exists();
        findings.push(auth_finding(
            "auth:sqlite-path",
            if sqlite_exists {
                Severity::Ok
            } else {
                Severity::Warn
            },
            if sqlite_exists {
                format!("{sqlite_path} found")
            } else {
                format!("{sqlite_path} not found — will be created at first login")
            },
        ));

        if sqlite_exists {
            let durable = rusqlite::Connection::open_with_flags(
                &sqlite_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .and_then(|conn| {
                conn.query_row(
                    "SELECT provider, config_fingerprint, generation FROM inbound_identity_provider WHERE singleton = 1",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
                )
            });
            match durable {
                Ok((durable_provider, durable_fingerprint, generation)) => findings.push(auth_finding(
                    "auth:provider-generation",
                    if durable_provider == provider && durable_fingerprint == config_fingerprint {
                        Severity::Ok
                    } else {
                        Severity::Fail
                    },
                    format!(
                        "durable provider generation {generation}; provider={durable_provider}; fingerprint={}",
                        durable_fingerprint.chars().take(16).collect::<String>()
                    ),
                )),
                Err(error) => findings.push(auth_finding(
                    "auth:provider-generation",
                    Severity::Fail,
                    format!("existing auth database is not ready ({})", error.sqlite_error_code().map_or("unavailable", |_| "sqlite error")),
                )),
            }
        }

        if provider == "authelia"
            && let Some(path) = config
                .and_then(|config| config.authelia.as_ref())
                .and_then(|authelia| authelia.ca_certificate_path.as_ref())
        {
            let ready = std::fs::read(path)
                .ok()
                .and_then(|pem| reqwest::Certificate::from_pem(&pem).ok())
                .is_some();
            findings.push(auth_finding(
                "auth:authelia-ca-certificate",
                if ready { Severity::Ok } else { Severity::Fail },
                if ready {
                    format!(
                        "{} is readable and contains a valid PEM certificate",
                        path.display()
                    )
                } else {
                    format!(
                        "{} is missing, unreadable, or not a valid PEM certificate",
                        path.display()
                    )
                },
            ));
        }

        let key_exists = std::path::Path::new(&key_path).exists();
        findings.push(auth_finding(
            "auth:key-path",
            if key_exists {
                Severity::Ok
            } else {
                Severity::Warn
            },
            if key_exists {
                format!("{key_path} found")
            } else {
                format!("{key_path} not found — will be generated at first startup")
            },
        ));

        // File permission checks (Unix only)
        #[cfg(unix)]
        {
            if sqlite_exists {
                findings.push(file_perms_check("auth", "auth:sqlite-perms", &sqlite_path));
            }
            if key_exists {
                findings.push(file_perms_check("auth", "auth:key-perms", &key_path));
            }
        }
    }

    findings
}

#[cfg(unix)]
fn file_perms_check(service: &str, label: &str, path: &str) -> Finding {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mode = meta.mode();
            let perms_ok = mode.trailing_zeros() >= 6;
            Finding {
                service: service.to_string(),
                check: label.to_string(),
                severity: if perms_ok {
                    Severity::Ok
                } else {
                    Severity::Fail
                },
                message: if perms_ok {
                    format!("{path}: permissions 0600 (owner-only)")
                } else {
                    format!(
                        "{path}: permissions {:04o} — must be 0600 (fix: chmod 600 {path})",
                        mode & 0o777
                    )
                },
            }
        }
        Err(e) => Finding {
            service: service.to_string(),
            check: label.to_string(),
            severity: Severity::Warn,
            message: format!("{path}: could not read permissions: {e}"),
        },
    }
}
