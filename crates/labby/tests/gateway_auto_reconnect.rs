//! Background reconnect through an isolated compiled server and its public CLI.
#![cfg(feature = "gateway")]
#![allow(dead_code, clippy::panic)]
#![cfg(feature = "proxy-testkit")]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use wiremock::{Mock, MockServer, ResponseTemplate};

// These cases each launch a complete Labby gateway and bootstrap its access
// owner. Running both bootstraps concurrently makes the process-level access
// admission deliberately fail closed, so serialize only the two public
// gateway lifecycle cases while leaving the support-unit tests parallel.
static PUBLIC_GATEWAY_LIFECYCLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn cli(server: &live_labby::LiveLabbyGuard, args: &[&str]) -> Value {
    let home = server.root().join("client-home");
    std::fs::create_dir_all(home.join("tmp")).expect("isolated client home");
    let mut command = tokio::process::Command::from(live_labby::isolated_command(&home));
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .args(args)
        .arg("--json");
    server.authorize_cli(&mut command);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("CLI deadline")
        .expect("CLI starts");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("public CLI JSON")
}

fn failure_diagnostics(server: &mut live_labby::LiveLabbyGuard, label: &str) -> String {
    use std::io::Read as _;

    // Only fixture-owned configuration/runtime files are captured. Log tails
    // use the harness sanitizer and stay bounded; credentials are never read.
    let snapshots: Vec<_> = ["config.toml", "config.runtime.json"]
        .into_iter()
        .map(|name| {
            let path = server.root().join("labby-home").join(name);
            let mut bytes = Vec::new();
            let result = std::fs::File::open(path)
                .and_then(|file| file.take(16 * 1024).read_to_end(&mut bytes));
            json!({"file": name, "content": evidence::sanitize(&String::from_utf8_lossy(&bytes)), "read_error": result.err().map(|error| error.to_string())})
        })
        .collect();
    let report =
        json!({"phase":label, "server":server.diagnostics(Some(label)), "snapshots":snapshots});
    // stderr is retained by the test runner even after the owned server root
    // is cleaned up, so failures carry their attempt-matched evidence.
    evidence::sanitize(&report.to_string())
}

async fn advances(
    server: &mut live_labby::LiveLabbyGuard,
    counter: &AtomicUsize,
    previous: usize,
    label: &str,
) {
    tokio::time::timeout(Duration::from_secs(100), async {
        while counter.load(Ordering::SeqCst) <= previous {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "background {label} did not advance from {previous}: {}",
            failure_diagnostics(server, label)
        )
    });
}

fn assert_recovered(state: &Value, name: &str, tool_count: usize) {
    let row = state
        .as_array()
        .expect("runtime rows")
        .iter()
        .find(|row| row["name"] == name)
        .expect("owned upstream listed");
    assert_eq!(row["connected"], true, "{row}");
    assert_eq!(row["discovered_tool_count"], tool_count, "{row}");
}

async fn published_recovery(
    server: &live_labby::LiveLabbyGuard,
    name: &str,
    tool_count: usize,
) -> Value {
    let mut last = Value::Null;
    let settled = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            // This public action reads cached runtime state; it never starts
            // discovery. A fixture response precedes publication in the owner.
            last = cli(server, &["server", "status"]).await;
            if last.as_array().is_some_and(|rows| {
                rows.iter().any(|row| {
                    row["name"] == name
                        && row["connected"] == true
                        && row["discovered_tool_count"] == tool_count
                })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await;
    assert!(settled.is_ok(), "recovery was not published: {last}");
    assert_recovered(&last, name, tool_count);
    last
}

async fn call_recovered_tool(
    server: &live_labby::LiveLabbyGuard,
    calls: &AtomicUsize,
    phase: &str,
) {
    let before = calls.load(Ordering::SeqCst);
    let code = format!(
        "return await callTool('owned-recovery::recovered', {{probe: {}}});",
        serde_json::to_string(phase).expect("phase JSON")
    );
    let output = cli(server, &["code", "run", "--code", &code]).await;
    assert_eq!(output["result"], json!({"reply": phase}), "{output}");
    assert_eq!(calls.load(Ordering::SeqCst), before + 1);
}

#[tokio::test]
async fn public_gateway_recovers_without_requests_and_after_cleanup() {
    let _lifecycle = PUBLIC_GATEWAY_LIFECYCLE.lock().await;
    let upstream = MockServer::start().await;
    let online = Arc::new(AtomicBool::new(true));
    let failed_requests = Arc::new(AtomicUsize::new(0));
    let catalogs = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let available = Arc::clone(&online);
    let failures = Arc::clone(&failed_requests);
    let listings = Arc::clone(&catalogs);
    let invocations = Arc::clone(&calls);
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            if !available.load(Ordering::SeqCst) {
                failures.fetch_add(1, Ordering::SeqCst);
                return ResponseTemplate::new(503);
            }
            let body: Value = serde_json::from_slice(&request.body).expect("MCP JSON");
            let result = match body["method"].as_str().unwrap_or_default() {
                "server/discover" => json!({"resultType":"complete", "supportedVersions":["2026-07-28"], "capabilities":{"tools":{}}, "serverInfo":{"name":"owned-recovery-fixture","version":"1"}, "ttlMs":0, "cacheScope":"private"}),
                "initialize" => json!({"protocolVersion":"2025-06-18", "capabilities":{"tools":{}}, "serverInfo":{"name":"owned-recovery-fixture","version":"1"}}),
                "notifications/initialized" => return ResponseTemplate::new(202),
                "tools/list" => {
                    listings.fetch_add(1, Ordering::SeqCst);
                    json!({"tools":[{"name":"recovered","description":"owned recovered catalog", "inputSchema":{"type":"object", "properties":{"probe":{"type":"string"}}, "required":["probe"]}, "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true}}]})
                }
                "tools/call" => {
                    assert_eq!(body["params"]["name"], "recovered");
                    let phase = body["params"]["arguments"]["probe"].as_str().expect("correlated call input");
                    invocations.fetch_add(1, Ordering::SeqCst);
                    json!({"content":[{"type":"text","text":phase}], "structuredContent":{"reply":phase}, "isError":false})
                }
                _ => return ResponseTemplate::new(500),
            };
            ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        }).mount(&upstream).await;
    let mut server = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .config(format!("[gateway]\nauto_reconnect = true\n[code_mode]\nenabled = true\n[[upstream]]\nname = \"owned-recovery\"\nenabled = true\nurl = \"{}/mcp\"\n", upstream.uri()))
        .start().await.expect("isolated gateway starts");
    server
        .bind_team_gateway_credential("owned-recovery")
        .await
        .expect("owned team binding");
    // Startup seeds the catalog lazily; observe background discovery before
    // inducing a failure so this starts from a confirmed healthy transport.
    advances(&mut server, &catalogs, 0, "initial background catalog").await;
    published_recovery(&server, "owned-recovery", 1).await;
    let before_failure = failed_requests.load(Ordering::SeqCst);
    online.store(false, Ordering::SeqCst);
    // Only fixture counters are inspected here: no request to Labby can trigger recovery.
    advances(
        &mut server,
        &failed_requests,
        before_failure,
        "offline probe",
    )
    .await;
    let before_recovery = catalogs.load(Ordering::SeqCst);
    online.store(true, Ordering::SeqCst);
    advances(&mut server, &catalogs, before_recovery, "recovered catalog").await;
    published_recovery(&server, "owned-recovery", 1).await;
    call_recovered_tool(&server, &calls, "after-offline-recovery").await;
    // Linux cleanup intentionally scans host-wide local MCP processes, which
    // may belong to concurrent tests. Exercise that transition in the isolated
    // pool tests there; macOS/Windows have no host process scan and can safely
    // exercise the complete public cleanup request in this server fixture.
    #[cfg(not(target_os = "linux"))]
    {
        cli(&server, &["server", "cleanup", "owned-recovery"]).await;
        let after_cleanup = catalogs.load(Ordering::SeqCst);
        advances(
            &mut server,
            &catalogs,
            after_cleanup,
            "catalog after cleanup",
        )
        .await;
        published_recovery(&server, "owned-recovery", 1).await;
        call_recovered_tool(&server, &calls, "after-cleanup-recovery").await;
    }
    let cleanup = server.finish().await;
    assert!(cleanup.is_clean(), "owned server cleanup: {cleanup:?}");
}

#[cfg(all(unix, feature = "proxy-testkit"))]
#[tokio::test]
async fn public_gateway_replaces_dead_stdio_process_without_requests() {
    let _lifecycle = PUBLIC_GATEWAY_LIFECYCLE.lock().await;
    let root = tempfile::tempdir().expect("owned stdio fixture root");
    let pid_file = root.path().join("fixture.pid");
    let command = env!("CARGO_BIN_EXE_stdio-mcp-fixture");
    let config = format!(
        "[gateway]\nauto_reconnect = true\nextra_stdio_commands = [{}]\n[code_mode]\nenabled = true\n[[upstream]]\nname = \"owned-stdio\"\nenabled = true\ncommand = {}\nargs = [\"--pid-file\", {}, \"--forge\"]\n",
        serde_json::to_string(command).unwrap(),
        serde_json::to_string(command).unwrap(),
        serde_json::to_string(&pid_file).unwrap(),
    );
    let mut server = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .config(config)
        .start()
        .await
        .expect("isolated stdio gateway starts");
    server
        .bind_team_gateway_credential("owned-stdio")
        .await
        .expect("owned stdio team binding");
    // Initial lazy discovery can wait for the periodic probe. Observe the
    // child directly before checking the cached gateway publication.
    let original = tokio::time::timeout(Duration::from_secs(100), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .filter(|pid| *pid > 0)
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "initial background discovery must start the owned stdio child: {}",
            failure_diagnostics(&mut server, "initial stdio discovery")
        )
    });
    let initial = published_recovery(&server, "owned-stdio", 10).await;
    let row = initial
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "owned-stdio")
        .unwrap();
    assert_eq!(
        row["pid"], original,
        "only kill the child owned by this gateway"
    );
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(original),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("kill only the owned fixture process");
    // File observations cannot drive an on-demand reconnect. A different PID
    // proves the configured periodic recovery actually spawned a replacement.
    let replacement = tokio::time::timeout(Duration::from_secs(100), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .filter(|pid| *pid != original)
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "periodic recovery must replace the dead stdio child: {}",
            failure_diagnostics(&mut server, "stdio replacement")
        )
    });
    let recovered = published_recovery(&server, "owned-stdio", 10).await;
    let row = recovered
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "owned-stdio")
        .unwrap();
    assert_eq!(row["pid"], replacement);
    let response = cli(
        &server,
        &[
            "code",
            "run",
            "--code",
            "return await callTool('owned-stdio::forge.safe', {query:'after-process-recovery'});",
        ],
    )
    .await;
    assert_eq!(response["result"]["tool"], "forge.safe", "{response}");
    assert_eq!(
        response["result"]["arguments"]["query"], "after-process-recovery",
        "{response}"
    );
    let cleanup = server.finish().await;
    assert!(cleanup.is_clean(), "owned server cleanup: {cleanup:?}");
}
