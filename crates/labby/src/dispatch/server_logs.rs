//! Shared dispatch layer for the `server_logs` operator tool.
//!
//! This is intentionally narrow: it reads Labby's own rolling process logs.
//! It does not reintroduce syslog ingestion, fleet log storage, or external
//! host log collection.

mod catalog;
mod client;
mod dispatch;
mod params;

pub use catalog::ACTIONS;
pub use dispatch::dispatch;

/// Run the deployment-aware systemd journal tail used by the CLI adapter.
pub fn tail_service_journal(
    lines: usize,
    follow: bool,
    container: Option<String>,
) -> anyhow::Result<()> {
    use std::process::{Command, Stdio};
    let container = container.or_else(detect_incus_container);
    let mut command = if let Some(container) = container {
        let mut command = Command::new("incus");
        command.args(["exec", &container, "--", "journalctl", "-u", "labby"]);
        command
    } else {
        let mut command = Command::new("journalctl");
        command.args(["-u", "labby"]);
        command
    };
    command.args(["-n", &lines.to_string(), "-o", "cat"]);
    if follow {
        command.arg("-f");
    }
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Labby journal command exited with {status}")
    }
}

fn detect_incus_container() -> Option<String> {
    let configured = std::env::var("LABBY_INCUS_CONTAINER")
        .ok()
        .filter(|name| !name.trim().is_empty());
    if configured.is_some() {
        return configured;
    }
    let output = std::process::Command::new("incus")
        .args(["list", "--format", "csv", "-c", "ns"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let running: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once(','))
        .filter(|(name, status)| {
            *status == "RUNNING" && (*name == "labby" || name.starts_with("labby-"))
        })
        .map(|(name, _)| name.to_owned())
        .collect();
    (running.len() == 1).then(|| running.into_iter().next().expect("one running container"))
}

use labby_primitives::plugin::{Category, PluginMeta};

/// Compile-time metadata for the server log viewer.
pub const META: PluginMeta = PluginMeta {
    name: "server_logs",
    display_name: "Server Logs",
    description: "View and filter Labby's own rolling server process logs",
    category: Category::Bootstrap,
    docs_url: "https://github.com/jmagar/lab",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
