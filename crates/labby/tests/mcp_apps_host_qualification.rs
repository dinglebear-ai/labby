//! Q4 deterministic browser-host emulation over MCP App resources obtained from a real Labby process.

#![cfg(all(feature = "gateway", feature = "proxy-testkit", unix))]
#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_tools_transport_qualification.rs"]
mod transport;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use rmcp::model::ResourceContents;
use serde_json::Value;
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;
use transport::{TransportKind, TransportQualification};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn text_resource(result: rmcp::model::ReadResourceResult) -> (String, String, Value) {
    let [
        ResourceContents::TextResourceContents {
            text,
            mime_type: Some(mime),
            meta: Some(meta),
            ..
        },
    ] = result.contents.as_slice()
    else {
        panic!("expected one text MCP App resource with MIME and metadata")
    };
    (
        text.clone(),
        mime.clone(),
        serde_json::to_value(meta).expect("resource metadata JSON"),
    )
}

async fn run_emulators(
    repository: &Path,
    openai_html: &Path,
    anthropic_html: &Path,
    endpoint: &str,
    token: &str,
) -> Value {
    let runner =
        repository.join("crates/labby/tests/support/mcp_apps_host_qualification/runner.mjs");
    let mut command = Command::new("node");
    command
        .arg(runner)
        .arg("--openai-html")
        .arg(openai_html)
        .arg("--anthropic-html")
        .arg(anthropic_html)
        .arg("--gateway-admin-dir")
        .arg(repository.join("apps/gateway-admin"))
        .arg("--mcp-endpoint")
        .arg(endpoint)
        .arg("--mcp-token")
        .arg(token)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command
        .spawn()
        .expect("start deterministic MCP App browser emulators");
    let process_group = Pid::from_raw(i32::try_from(child.id().expect("emulator pid")).unwrap());
    let stdout = child.stdout.take().expect("emulator stdout");
    let stderr = child.stderr.take().expect("emulator stderr");
    let mut stdout = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take(65_537)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let mut stderr = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(65_537)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let status = match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(status) => status.expect("wait for MCP App browser emulators"),
        Err(_) => {
            let _ = killpg(process_group, Signal::SIGTERM);
            drop(tokio::time::timeout(Duration::from_secs(2), child.wait()).await);
            // The leader may exit while a descendant retains the process group
            // and inherited pipes, so settle the whole owned group regardless.
            let _ = killpg(process_group, Signal::SIGKILL);
            drop(tokio::time::timeout(Duration::from_secs(2), child.wait()).await);
            stdout.abort();
            stderr.abort();
            panic!("MCP App browser emulators exceeded 30-second deadline");
        }
    };
    let readers = tokio::time::timeout(Duration::from_secs(3), async {
        let stdout = (&mut stdout)
            .await
            .expect("join emulator stdout")
            .expect("read stdout");
        let stderr = (&mut stderr)
            .await
            .expect("join emulator stderr")
            .expect("read stderr");
        (stdout, stderr)
    })
    .await;
    let (stdout, stderr) = match readers {
        Ok(output) => output,
        Err(_) => {
            let _ = killpg(process_group, Signal::SIGKILL);
            stdout.abort();
            stderr.abort();
            panic!("emulator descendants retained stdout/stderr after leader exit")
        }
    };
    assert!(stdout.len() <= 65_536, "emulator stdout exceeded 64 KiB");
    assert!(stderr.len() <= 65_536, "emulator stderr exceeded 64 KiB");
    assert!(
        status.success(),
        "host emulators failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    serde_json::from_slice(&stdout).expect("host emulator JSON summary")
}

#[tokio::test]
async fn q4_real_resources_render_in_distinct_openai_and_anthropic_emulators() {
    let runner =
        TransportQualification::start(TransportKind::StreamableHttp, "q4-mcp-app-secret-canary")
            .await
            .expect("real Labby MCP process");

    let initially_hidden = runner.read_resource("ui://lab/apps/manage").await;
    assert!(
        initially_hidden.is_err(),
        "disabled app resource must not bypass policy"
    );

    let enabled = runner
        .call_raw(
            "mcp_app",
            serde_json::json!({"action":"enable", "params":{"target":"manager"}}),
        )
        .await
        .expect("enable manager through real tools/call");
    assert_ne!(enabled.is_error, Some(true), "enable failed: {enabled:?}");

    let tools = runner
        .discover_tools()
        .await
        .expect("discover real MCP tool metadata");
    let manager = tools.get("mcp_app").expect("manager tool after enable");
    let tool_meta = serde_json::to_value(manager.meta.as_ref().expect("manager metadata"))
        .expect("tool metadata JSON");
    let mcp_uri = tool_meta["ui"]["resourceUri"]
        .as_str()
        .expect("SEP-1724 UI binding");
    let openai_uri = tool_meta["openai/outputTemplate"]
        .as_str()
        .expect("OpenAI output template binding");

    let (anthropic_html, anthropic_mime, anthropic_meta) = text_resource(
        runner
            .read_resource(mcp_uri)
            .await
            .expect("MCP Apps resource"),
    );
    let (openai_html, openai_mime, openai_meta) = text_resource(
        runner
            .read_resource(openai_uri)
            .await
            .expect("OpenAI skybridge resource"),
    );
    assert_eq!(anthropic_mime, "text/html;profile=mcp-app");
    assert_eq!(openai_mime, "text/html+skybridge");
    for meta in [&anthropic_meta, &openai_meta] {
        assert_eq!(meta["ui"]["csp"]["connectDomains"], serde_json::json!([]));
        assert_eq!(meta["ui"]["csp"]["resourceDomains"], serde_json::json!([]));
        assert_eq!(meta["ui"]["csp"]["frameDomains"], serde_json::json!([]));
    }
    assert!(anthropic_html.contains("window.__LABBY_MCP_RESOURCE=true"));
    assert!(!openai_html.contains("window.__LABBY_MCP_RESOURCE=true"));

    let html_dir = tempfile::tempdir().expect("host-emulation HTML directory");
    let openai_path = html_dir.path().join("openai.html");
    let anthropic_path = html_dir.path().join("anthropic.html");
    std::fs::write(&openai_path, openai_html).expect("write OpenAI resource");
    std::fs::write(&anthropic_path, anthropic_html).expect("write MCP Apps resource");
    let endpoint = runner.http_endpoint().expect("HTTP qualification endpoint");
    let summary = run_emulators(
        &repository_root(),
        &openai_path,
        &anthropic_path,
        &endpoint,
        runner.http_token(),
    )
    .await;
    assert_eq!(summary["status"], "PASS");
    assert_eq!(summary["actual_vendor_host"], false);
    assert_eq!(summary["emulators"].as_array().map(Vec::len), Some(2));
    assert_eq!(summary["teardown"], "measured-browser-and-server-closed");
    assert_eq!(summary["cleanup"]["browser_closed"], true);
    assert_eq!(summary["cleanup"]["server_closed"], true);
    assert_eq!(summary["forbidden_requests"], 0);
    assert_eq!(
        summary["cancellation"]["status"],
        "unsupported-by-pinned-rmcp-stateless-notification-delivery"
    );
    for emulator in summary["emulators"].as_array().expect("emulators") {
        assert_eq!(emulator["live_callback"], true);
        assert!(
            emulator["negative_auth"]
                .as_str()
                .is_some_and(|message| message.starts_with("HTTP 401:"))
        );
        assert_eq!(emulator["csp"]["fetchBlocked"], true);
        assert_eq!(emulator["csp"]["imageBlocked"], true);
        assert_eq!(emulator["csp"]["frameBlocked"], true);
    }
    let openai = summary["emulators"]
        .as_array()
        .expect("emulators")
        .iter()
        .find(|emulator| emulator["host"] == "openai-emulator")
        .expect("OpenAI emulator evidence");
    assert_eq!(openai["type_error_calls"], 1);
    let disabled = runner
        .call_raw(
            "mcp_app",
            serde_json::json!({"action":"disable", "params":{"target":"manager"}}),
        )
        .await
        .expect("disable manager through real tools/call");
    assert_ne!(
        disabled.is_error,
        Some(true),
        "disable failed: {disabled:?}"
    );
    assert!(
        runner.read_resource(mcp_uri).await.is_err(),
        "revoked resource remained readable on the same authenticated session"
    );

    let cleanup = runner.finish().await;
    assert!(
        cleanup.is_clean(),
        "unclean process teardown: {:?}",
        cleanup.failures
    );
}

#[tokio::test]
async fn q4_stateless_http_cancellation_reaches_admitted_upstream() {
    let runner = TransportQualification::start(TransportKind::StreamableHttp, "q4-cancel-canary")
        .await
        .expect("real Labby MCP process");
    runner
        .call_raw("forge.safe", serde_json::json!({}))
        .await
        .expect("initialize ledger");
    let before = runner.effect_counts().expect("initial ledger");
    let endpoint = runner.http_endpoint().expect("HTTP endpoint");
    let token = runner.http_token();
    let request_id = 91_004_u64;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("bounded reproduction client");
    let spawn_call = |request_id| {
        let client = client.clone();
        let endpoint = endpoint.clone();
        tokio::spawn(async move {
            client
                .post(endpoint)
                .bearer_auth(token)
                .header("accept", "application/json, text/event-stream")
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/call")
                .header("mcp-name", "forge.pending")
                .header("x-labby-project-id", "disposable")
                .header("x-labby-team-id", "bootstrap-initial-team")
                .json(&serde_json::json!({
                    "jsonrpc":"2.0", "id":request_id, "method":"tools/call",
                    "params":{"name":"forge.pending","arguments":{},"_meta":{
                        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                        "io.modelcontextprotocol/clientInfo":{"name":"q4-cancel-repro","version":"1"},
                        "io.modelcontextprotocol/clientCapabilities":{}
                    }}
                }))
                .send()
                .await?
                .bytes()
                .await
        })
    };
    let call = spawn_call(request_id);
    let survivor = spawn_call(request_id + 1);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while runner.effect_counts().expect("ledger during admission") != (before.0 + 2, 2) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "call was not admitted"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // MCP 2026-07-28 HTTP cancellation closes this request's response stream.
    // Keep consuming the body above so this also covers an early SSE response:
    // dropping a completed send future alone would not close a retained body.
    call.abort();
    let aborted = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .expect("request abort deadline");
    assert!(
        aborted.is_err_and(|error| error.is_cancelled()),
        "pending request finished before cancellation"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let settled = loop {
        if runner.effect_counts().expect("ledger during settlement") == (before.0 + 2, 1) {
            break true;
        }
        if tokio::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        !survivor.is_finished(),
        "cancellation affected a different request"
    );
    survivor.abort();
    let survivor_result = tokio::time::timeout(Duration::from_secs(2), survivor)
        .await
        .expect("survivor cleanup deadline");
    assert!(survivor_result.is_err_and(|error| error.is_cancelled()));
    let observed = runner.effect_counts().expect("final reproduction ledger");
    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
    assert!(
        settled,
        "closing the admitted HTTP response did not settle upstream work; final ledger={observed:?}"
    );
}
