#![cfg(all(feature = "gateway", unix))]
#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::Command;

const OUTPUT_LIMIT: usize = 32 * 1024;
const CASE_DEADLINE: Duration = Duration::from_secs(90);
const PROCESS_CLEANUP_DEADLINE: Duration = Duration::from_secs(5);

async fn read_capped(
    mut input: impl AsyncRead + Unpin,
    label: &'static str,
) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = input
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > OUTPUT_LIMIT {
            return Err(format!("{label} exceeded {OUTPUT_LIMIT} byte cap"));
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

async fn terminate_process_group(
    child: &mut tokio::process::Child,
    process_group: Pid,
) -> Result<(), String> {
    let _ = killpg(process_group, Signal::SIGTERM);
    let deadline = Instant::now() + PROCESS_CLEANUP_DEADLINE;
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(_) => return Ok(()),
            None if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            None => break,
        }
    }
    let _ = killpg(process_group, Signal::SIGKILL);
    tokio::time::timeout(PROCESS_CLEANUP_DEADLINE, child.wait())
        .await
        .map_err(|_| "Chromium process-group cleanup deadline exceeded".to_string())?
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn evidence_root(repository: &std::path::Path) -> PathBuf {
    if let Some(run_root) = std::env::var_os("LABBY_E2E_RUN_ROOT") {
        return PathBuf::from(run_root).join("artifacts/q4-webmcp");
    }
    repository
        .join("target/q4-webmcp")
        .join(format!("run-{}", uuid::Uuid::new_v4()))
}

#[tokio::test]
async fn installed_chromium_extension_qualifies_webmcp_lifecycle() {
    if std::env::var_os("LABBY_LIVE_WEBMCP_RUN").is_none() {
        return;
    }

    let token = uuid::Uuid::new_v4().to_string();
    let mut gateway = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", &token)
        .start()
        .await
        .expect("isolated production gateway");
    let repository = repository_root();
    let evidence = evidence_root(&repository);
    std::fs::create_dir_all(&evidence).expect("Q4 evidence directory");

    let runner =
        repository.join("crates/labby/tests/support/webmcp_browser_qualification/runner.mjs");
    let extension = repository.join("apps/browser-extension");
    let gateway_admin = repository.join("apps/gateway-admin");
    let node = std::env::var_os("LABBY_NODE_BIN").unwrap_or_else(|| "node".into());
    let mut command = Command::new(node);
    command
        .arg(&runner)
        .arg("--base-url")
        .arg(&gateway.connection().base_url)
        .arg("--extension-dir")
        .arg(&extension)
        .arg("--gateway-admin-dir")
        .arg(&gateway_admin)
        .arg("--evidence-dir")
        .arg(&evidence)
        .env("LABBY_Q4_TOKEN", &token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);

    let mut child = command
        .spawn()
        .expect("spawn installed-browser qualification");
    let process_group = Pid::from_raw(i32::try_from(child.id().expect("runner pid")).unwrap());
    let stdout = child.stdout.take().expect("runner stdout");
    let stderr = child.stderr.take().expect("runner stderr");
    let stdout_reader = tokio::spawn(read_capped(stdout, "browser qualification stdout"));
    let stderr_reader = tokio::spawn(read_capped(stderr, "browser qualification stderr"));

    let runner_status = match tokio::time::timeout(CASE_DEADLINE, child.wait()).await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => {
            let cleanup = terminate_process_group(&mut child, process_group).await;
            Err(format!(
                "installed-browser qualification exceeded {} seconds; cleanup={cleanup:?}",
                CASE_DEADLINE.as_secs()
            ))
        }
    };
    let stdout = stdout_reader
        .await
        .map_err(|error| error.to_string())
        .and_then(|output| output);
    let stderr = stderr_reader
        .await
        .map_err(|error| error.to_string())
        .and_then(|output| output);

    let cleanup = gateway.finish_with_deadline(Duration::from_secs(10)).await;
    assert!(
        cleanup.is_clean(),
        "gateway cleanup failed: {:?}",
        cleanup.failures
    );

    let stdout = stdout.expect("bounded browser qualification stdout");
    let stderr = stderr.expect("bounded browser qualification stderr");
    let summary = stdout
        .split(|byte| *byte == b'\n')
        .rev()
        .find(|line| !line.is_empty())
        .map(serde_json::from_slice::<Value>)
        .transpose()
        .expect("runner JSON summary")
        .unwrap_or(Value::Null);
    let runner_status = runner_status.unwrap_or_else(|error| {
        panic!(
            "browser runner lifecycle failed: {error}; summary={summary}; stderr={}",
            String::from_utf8_lossy(&stderr)
        )
    });
    assert!(
        runner_status.success(),
        "installed-browser qualification failed: summary={summary}; stderr={}",
        String::from_utf8_lossy(&stderr)
    );
    assert_eq!(summary["status"], "PASS", "unexpected runner summary");
    assert_eq!(summary["installed_extension"], true);
    assert_eq!(summary["socket_simulator"], false);
    assert!(
        summary["chromium_version"]
            .as_str()
            .is_some_and(|version| !version.is_empty()),
        "Chromium version evidence missing"
    );
    assert!(
        summary["extension_sha256"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64),
        "extension digest evidence missing"
    );
    for required in ["report.md", "result.json", "meta.json"] {
        assert!(evidence.join(required).is_file(), "missing {required}");
    }
}
