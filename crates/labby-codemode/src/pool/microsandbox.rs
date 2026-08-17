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
    pub(super) fn remove(&mut self) {
        if self.removed {
            return;
        }
        self.removed = true;
        let child = std::process::Command::new(&self.executable)
            .args(["remove", "--quiet", "--force", &self.name])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            tracing::warn!(sandbox = %self.name, "failed to start Microsandbox cleanup");
            return;
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return,
                Ok(Some(status)) => {
                    tracing::warn!(sandbox = %self.name, %status, "Microsandbox cleanup failed");
                    return;
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(None) => {
                    let _kill_result = child.kill();
                    tracing::warn!(sandbox = %self.name, "Microsandbox cleanup timed out");
                    return;
                }
                Err(error) => {
                    tracing::warn!(sandbox = %self.name, %error, "Microsandbox cleanup wait failed");
                    return;
                }
            }
        }
    }
}

impl Drop for MicrosandboxGuard {
    fn drop(&mut self) {
        self.remove();
    }
}

pub(super) async fn runner_command(
    spawn: &RunnerSpawn,
) -> Result<(Command, Option<MicrosandboxGuard>), ToolError> {
    let Some(config) = &spawn.microsandbox else {
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
    create(config, &name, &mount).await?;

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
    Ok((
        command,
        Some(MicrosandboxGuard {
            executable: config.executable.clone(),
            name,
            removed: false,
        }),
    ))
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
            "30m",
            "--idle-timeout",
            "5m",
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
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(512)
                .collect::<String>()
        ),
    })
}
