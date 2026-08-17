//! Microsandbox lifecycle and byte-faithful stdio attachment for a runner.

use std::process::Stdio;

use tokio::process::Command;

use crate::error::ToolError;
use crate::pool::RunnerSpawn;

static NEXT_SANDBOX_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(super) struct MicrosandboxGuard {
    executable: std::path::PathBuf,
    name: String,
    removed: bool,
}

impl MicrosandboxGuard {
    pub(super) async fn remove(&mut self) {
        if self.removed {
            return;
        }
        let mut remove = Command::new(&self.executable);
        remove
            .args(["remove", "--quiet", "--force", &self.name])
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        match tokio::time::timeout(std::time::Duration::from_secs(2), remove.output()).await {
            Ok(Ok(output)) if output.status.success() => self.removed = true,
            Ok(Ok(output)) => {
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
            }
            Ok(Err(error)) => tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "microsandbox.remove",
                kind = "cleanup_spawn_failed",
                sandbox = %self.name,
                executable = %self.executable.display(),
                %error,
                "failed to start Microsandbox cleanup"
            ),
            Err(_) => tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "microsandbox.remove",
                kind = "cleanup_timeout",
                sandbox = %self.name,
                "Microsandbox cleanup timed out"
            ),
        }
    }

    fn remove_in_background(&mut self) {
        if self.removed {
            return;
        }
        self.removed = true;
        let executable = self.executable.clone();
        let name = self.name.clone();
        let spawn = std::thread::Builder::new()
            .name("labby-msb-cleanup".into())
            .spawn(move || {
                let child = std::process::Command::new(&executable)
                    .args(["remove", "--quiet", "--force", &name])
                    .env_clear()
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                let result = child.and_then(|mut child| {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                    loop {
                        match child.try_wait()? {
                            Some(status) => return Ok(status),
                            None if std::time::Instant::now() < deadline => {
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                            None => {
                                child.kill()?;
                                return child.wait();
                            }
                        }
                    }
                });
                if !matches!(&result, Ok(status) if status.success()) {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "microsandbox.remove_fallback",
                        kind = "cleanup_failed",
                        sandbox = %name,
                        ?result,
                        "background Microsandbox cleanup failed"
                    );
                }
            });
        if let Err(error) = spawn {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "microsandbox.remove_fallback",
                kind = "cleanup_spawn_failed",
                sandbox = %self.name,
                %error,
                "failed to start background Microsandbox cleanup"
            );
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
) -> Result<(Command, Option<MicrosandboxGuard>), ToolError> {
    let Some(config) = config else {
        let mut command = Command::new(&spawn.program);
        command.args(&spawn.args);
        return Ok((command, None));
    };

    let ordinal = NEXT_SANDBOX_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("labby-codemode-{}-{ordinal}", std::process::id());
    let mount = format!(
        "{}:/opt/labby/labby:ro,nodev,nosuid",
        spawn.program.display()
    );
    let mut guard = MicrosandboxGuard {
        executable: config.executable.clone(),
        name: name.clone(),
        removed: false,
    };
    if let Err(error) = create(config, &name, &mount).await {
        guard.remove().await;
        return Err(error);
    }

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
            "--mount-file",
            mount,
            &config.image,
        ])
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), create.output())
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "timeout".into(),
            message: "Microsandbox Code Mode runner creation timed out after 15 seconds".into(),
        })?
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("failed to start Microsandbox Code Mode runner: {error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!(
            "failed to create Microsandbox Code Mode runner: {}",
            sanitize_stderr(&output.stderr)
        ),
    })
}

fn sanitize_stderr(stderr: &[u8]) -> String {
    labby_runtime::redact::sanitize_error_text(&String::from_utf8_lossy(stderr), 512)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

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
        let error = runner_command(&spawn(), Some(&config))
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
    async fn successful_create_returns_stream_command_and_async_guard() {
        let (_dir, executable, calls) = fake_msb(0);
        let config = config(executable);
        let (command, guard) = runner_command(&spawn(), Some(&config))
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
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = remove ] && [ ! -e '{}' ]; then touch '{}'; exit 9; fi\nexit 0\n",
            calls.display(),
            first_remove.display(),
            first_remove.display()
        );
        std::fs::write(&executable, script).expect("write fake msb");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("make executable");
        let config = config(executable);
        let (_command, guard) = runner_command(&spawn(), Some(&config))
            .await
            .expect("create succeeds");
        let mut guard = guard.expect("guard");
        guard.remove().await;
        drop(guard);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
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
                std::time::Instant::now() < deadline,
                "fallback did not retry: {recorded}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
