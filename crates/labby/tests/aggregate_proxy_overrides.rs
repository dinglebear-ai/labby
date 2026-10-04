// Exercise selected environment propagation through both proxy process hops.
#![allow(clippy::disallowed_methods)]
#![cfg(all(unix, feature = "gateway", feature = "proxy-testkit"))]

use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, BufReader};

#[tokio::test]
async fn aggregate_overlaps_startup_and_honors_environment_and_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let barrier = temp.path().join("startup-barrier");
    std::fs::create_dir(&barrier).unwrap();
    let config = serde_json::json!({"mcpServers": {
        "alpha": {"command": env!("CARGO_BIN_EXE_stdio-mcp-fixture"), "args":["--startup-barrier", barrier, "--pid-file", temp.path().join("alpha.pid")], "env":{"PROXY_EXPLICIT":"json-value"}},
        "beta": {"command": env!("CARGO_BIN_EXE_stdio-mcp-fixture"), "args":["--startup-barrier", barrier, "--pid-file", temp.path().join("beta.pid")]}
    }});
    std::fs::write(temp.path().join(".mcp.json"), config.to_string()).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
        .args([
            "--json",
            "proxy",
            "--local",
            "--auth",
            "none",
            "--env",
            "PROXY_EXPLICIT=cli-value",
            "--inherit-env",
            "PROXY_INHERITED",
            "--cwd",
        ])
        .arg(&cwd)
        .env("LABBY_HOME", temp.path())
        .env("PROXY_INHERITED", "selected-value")
        .env("PROXY_SCRUB_CANARY", "unselected-secret")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let ready = tokio::time::timeout(Duration::from_mins(1), async {
        let mut lines = BufReader::new(stdout).lines();
        tokio::select! {
            line = lines.next_line() => line.unwrap().expect("readiness"),
            status = child.wait() => {
                assert!(status.is_err(), "proxy exited before readiness: {status:?}");
                String::new()
            },
        }
    })
    .await
    .unwrap();
    let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
    drop(rustls::crypto::ring::default_provider().install_default());
    let worker = StreamableHttpClientWorker::new(
        reqwest::Client::new(),
        StreamableHttpClientTransportConfig::with_uri(ready["url"].as_str().unwrap().to_owned()),
    );
    let service = ()
        .serve_with_lifecycle(
            worker,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .unwrap();
    for tool in service.peer().list_all_tools().await.unwrap() {
        assert_eq!(
            tool.meta.unwrap().0["securitySchemes"],
            serde_json::json!([{"type":"noauth"}])
        );
    }
    for name in ["alpha::fixture.echo", "beta::fixture.echo"] {
        let result = service
            .peer()
            .call_tool(rmcp::model::CallToolRequestParams::new(name))
            .await
            .unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(payload["explicit_env"], "cli-value");
        assert_eq!(payload["inherited_custom"], "selected-value");
        assert!(payload["scrub_canary"].is_null());
        assert_eq!(
            std::path::PathBuf::from(payload["cwd"].as_str().unwrap()),
            cwd.canonicalize().unwrap()
        );
    }
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn aggregate_rejects_unroutable_catalog_and_failed_server_before_readiness() {
    for namespace in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let pid = temp.path().join("upstream.pid");
        let mut servers = serde_json::json!({
            "alpha": {"command": env!("CARGO_BIN_EXE_stdio-mcp-fixture"),
                "args":["--pid-file", pid],
                "env": if namespace { serde_json::json!({"PROXY_NAMESPACE_FIXTURE":"1"}) } else { serde_json::json!({}) }
            }
        });
        if !namespace {
            servers["broken"] = serde_json::json!({"command":"/usr/bin/false"});
        }
        std::fs::write(
            temp.path().join(".mcp.json"),
            serde_json::json!({"mcpServers":servers}).to_string(),
        )
        .unwrap();
        let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
            .args(["--json", "proxy", "--local", "--auth", "none"])
            .env("LABBY_HOME", temp.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("\"url\""));
        if namespace {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("unsupported namespace separator"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if let Ok(pid) = std::fs::read_to_string(pid) {
            let pid = nix::unistd::Pid::from_raw(pid.parse().unwrap());
            tokio::time::timeout(Duration::from_secs(3), async {
                while nix::sys::signal::kill(pid, None).is_ok() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("failed startup must reap connected upstream");
        }
    }
}
