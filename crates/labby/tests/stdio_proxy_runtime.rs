#![cfg(all(feature = "gateway", feature = "proxy-testkit"))]

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use labby::proxy::command::ProxyCommand;
use labby::proxy::config::{ProxyAuthMode, ProxyExposure, ProxyPortPreference, ProxyPreferences};
use labby::proxy::runtime::{LocalProxy, LocalProxyOptions};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};

fn ensure_tls_provider() {
    drop(rustls::crypto::ring::default_provider().install_default());
}

fn fixture_command(cwd: PathBuf, pid_file: &std::path::Path) -> ProxyCommand {
    let program = OsString::from(env!("CARGO_BIN_EXE_stdio-mcp-fixture"));
    let args = vec![
        OsString::from("--pid-file"),
        pid_file.as_os_str().to_owned(),
    ];
    ProxyCommand {
        display: format!(
            "{} --pid-file {}",
            program.to_string_lossy(),
            pid_file.display()
        ),
        program,
        args,
        cwd,
    }
}

fn local_preferences(auth: ProxyAuthMode) -> ProxyPreferences {
    ProxyPreferences {
        exposure: ProxyExposure::Local,
        auth,
        path: "/custom-mcp".to_string(),
        ..ProxyPreferences::default()
    }
}

async fn connect(
    url: &url::Url,
    token: Option<&str>,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    ensure_tls_provider();
    let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str().to_string());
    config.auth_header = token.map(str::to_string);
    let worker = StreamableHttpClientWorker::new(reqwest::Client::new(), config);
    ().serve_with_lifecycle(
        worker,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
        },
    )
    .await
    .expect("HTTP MCP client connects")
}

#[tokio::test]
async fn local_proxy_forwards_tools_list_after_child_discovery_and_bind() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("child.pid");
    let mut command = fixture_command(temp.path().to_path_buf(), &pid_file);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        command.args.push(OsString::from_vec(vec![b'x', 0xff]));
    }
    let proxy = LocalProxy::start(LocalProxyOptions {
        command,
        preferences: local_preferences(ProxyAuthMode::None),
        bearer_token: None,
        explicit_env: vec![(OsString::from("PROXY_EXPLICIT"), OsString::from("present"))],
        inherit_env: vec![OsString::from("PATH")],
    })
    .await
    .expect("proxy starts only after child discovery and listener bind");

    assert_eq!(proxy.url().path(), "/custom-mcp");
    let service = connect(proxy.url(), None).await;
    let tools = service.peer().list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_ref(), "fixture.echo");

    let sse_response = reqwest::Client::new()
        .post(proxy.url().clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 77,
            "method": "tools/list",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "proxy-sse-test",
                        "version": "1.0.0"
                    },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(sse_response.status(), reqwest::StatusCode::OK);
    assert!(
        sse_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"))
    );
    let sse_body = sse_response.text().await.unwrap();
    assert!(
        sse_body.contains("\"id\":77"),
        "unexpected SSE body: {sse_body}"
    );
    assert!(
        sse_body.contains("\"tools\""),
        "unexpected SSE body: {sse_body}"
    );

    let result = service
        .peer()
        .call_tool(rmcp::model::CallToolRequestParams::new("fixture.echo"))
        .await
        .unwrap();
    let context: serde_json::Value = serde_json::from_str(
        &result.content[0]
            .as_text()
            .expect("fixture returns text")
            .text,
    )
    .unwrap();
    assert_eq!(context["cwd"], temp.path().to_string_lossy().as_ref());
    assert_eq!(context["explicit_env"], "present");
    assert!(
        context["inherited_path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
    );
    #[cfg(unix)]
    assert_eq!(context["saw_non_utf8_argument"], true);

    let bad_host = reqwest::Client::new()
        .post(proxy.url().clone())
        .header(reqwest::header::HOST, "attacker.example")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_host.status(), reqwest::StatusCode::FORBIDDEN);
    let bad_origin = reqwest::Client::new()
        .post(proxy.url().clone())
        .header(reqwest::header::ORIGIN, "https://attacker.example")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_origin.status(), reqwest::StatusCode::FORBIDDEN);

    proxy.shutdown().await.expect("clean proxy shutdown");
}

#[tokio::test]
async fn local_proxy_honors_a_fixed_port() {
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let fixed_port = reservation.local_addr().unwrap().port();
    drop(reservation);

    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("fixed-port-child.pid");
    let mut preferences = local_preferences(ProxyAuthMode::None);
    preferences.port = ProxyPortPreference::Fixed(fixed_port);
    let proxy = LocalProxy::start(LocalProxyOptions {
        command: fixture_command(temp.path().to_path_buf(), &pid_file),
        preferences,
        bearer_token: None,
        explicit_env: Vec::new(),
        inherit_env: vec![OsString::from("PATH")],
    })
    .await
    .unwrap();

    assert_eq!(proxy.url().port(), Some(fixed_port));
    proxy.shutdown().await.unwrap();
}

#[tokio::test]
async fn bearer_proxy_rejects_missing_token_and_accepts_configured_token() {
    ensure_tls_provider();
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("child.pid");
    let proxy = LocalProxy::start(LocalProxyOptions {
        command: fixture_command(temp.path().to_path_buf(), &pid_file),
        preferences: local_preferences(ProxyAuthMode::Bearer),
        bearer_token: Some("proxy-secret".to_string()),
        explicit_env: Vec::new(),
        inherit_env: vec![OsString::from("PATH")],
    })
    .await
    .unwrap();

    let rejected = reqwest::Client::new()
        .post(proxy.url().clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(
        rejected
            .headers()
            .contains_key(reqwest::header::WWW_AUTHENTICATE)
    );
    let wrong_token = reqwest::Client::new()
        .post(proxy.url().clone())
        .bearer_auth("wrong-secret")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_token.status(), reqwest::StatusCode::UNAUTHORIZED);

    let service = connect(proxy.url(), Some("proxy-secret")).await;
    assert_eq!(service.peer().list_all_tools().await.unwrap().len(), 1);
    proxy.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_reaps_the_owned_stdio_child() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("child.pid");
    let proxy = LocalProxy::start(LocalProxyOptions {
        command: fixture_command(temp.path().to_path_buf(), &pid_file),
        preferences: local_preferences(ProxyAuthMode::None),
        bearer_token: None,
        explicit_env: Vec::new(),
        inherit_env: vec![OsString::from("PATH")],
    })
    .await
    .unwrap();
    let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();

    proxy.shutdown().await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "child PID {pid} survived shutdown"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cli_prints_real_url_serves_tools_and_stops_cleanly_on_sigint() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("cli-child.pid");
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["--json", "proxy", "--local", "--auth", "none"])
        .arg(env!("CARGO_BIN_EXE_stdio-mcp-fixture"))
        .arg("--pid-file")
        .arg(&pid_file)
        .env("LABBY_HOME", temp.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let line = tokio::time::timeout(
        Duration::from_secs(10),
        BufReader::new(stdout).lines().next_line(),
    )
    .await
    .expect("CLI readiness output timed out")
    .unwrap()
    .expect("CLI exited before readiness");
    let ready: serde_json::Value =
        serde_json::from_str(&line).expect("readiness is one JSON object");
    let url = url::Url::parse(ready["url"].as_str().unwrap()).unwrap();
    let service = connect(&url, None).await;
    assert_eq!(service.peer().list_all_tools().await.unwrap().len(), 1);

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("CLI did not stop after Ctrl+C")
        .unwrap();
    assert!(status.success(), "CLI exited with {status}");
}

#[cfg(unix)]
#[tokio::test]
async fn zero_flag_cli_publishes_verified_tailscale_url_with_fake_cli() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("tailscale-child.pid");
    let mapping = temp.path().join("mapping");
    let calls = temp.path().join("tailscale-calls");
    let fake_tailscale = temp.path().join("tailscale");
    let script = format!(
        r#"#!/usr/bin/env bash
set -u
mapping='{mapping}'
calls='{calls}'
printf '%s\n' "$*" >> "$calls"
if [[ "${{1:-}} ${{2:-}}" == "status --json" ]]; then
  printf '%s\n' '{{"BackendState":"Running","Self":{{"Online":true,"DNSName":"proxy-test.example.ts.net."}}}}'
  exit 0
fi
if [[ "${{1:-}}" == "version" ]]; then printf '%s\n' '1.98.10'; exit 0; fi
if [[ "${{1:-}} ${{2:-}} ${{3:-}}" == "serve status --json" ]]; then
  if [[ -f "$mapping" ]]; then
    IFS='|' read -r port backend < "$mapping"
    printf '{{"TCP":{{}},"Web":{{"proxy-test.example.ts.net:%s":{{"Handlers":{{"/":{{"Proxy":"%s"}}}}}}}}}}\n' "$port" "$backend"
  else
    printf '%s\n' '{{"TCP":{{}},"Web":{{}}}}'
  fi
  exit 0
fi
if [[ "${{1:-}}" == "serve" ]]; then
  port="${{3#--https=}}"; backend="${{4:-}}"
  if [[ "$backend" == "off" ]]; then rm -f "$mapping"; exit 0; fi
  printf '%s|%s\n' "$port" "$backend" > "$mapping"
  trap 'rm -f "$mapping"; exit 0' TERM INT
  while :; do sleep 0.05; done
fi
exit 2
"#,
        mapping = mapping.display(),
        calls = calls.display(),
    );
    std::fs::write(&fake_tailscale, script).unwrap();
    std::fs::set_permissions(&fake_tailscale, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(temp.path().join("config.toml"), "[proxy]\nport = 54000\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["--json", "proxy"])
        .arg(env!("CARGO_BIN_EXE_stdio-mcp-fixture"))
        .arg("--pid-file")
        .arg(&pid_file)
        .env("LABBY_HOME", temp.path())
        .env("LABBY_TAILSCALE_BIN", &fake_tailscale)
        .current_dir(temp.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let line = tokio::time::timeout(
        Duration::from_secs(10),
        BufReader::new(child.stdout.take().unwrap())
            .lines()
            .next_line(),
    )
    .await
    .expect("CLI readiness output timed out")
    .unwrap()
    .expect("CLI exited before readiness");
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["url"], "https://proxy-test.example.ts.net:54000/mcp");
    assert_eq!(ready["exposure"], "tailscale");
    assert_eq!(ready["auth"], "tailnet");
    assert_eq!(ready["external_port"], 54_000);
    let local_url = url::Url::parse(&format!(
        "http://{}/mcp",
        ready["local_addr"].as_str().unwrap()
    ))
    .unwrap();
    let service = connect(&local_url, None).await;
    assert_eq!(service.peer().list_all_tools().await.unwrap().len(), 1);

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("CLI did not stop after Ctrl+C")
        .unwrap();
    assert!(status.success(), "CLI exited with {status}");
    assert!(!mapping.exists());
    let calls = std::fs::read_to_string(calls).unwrap();
    assert!(!calls.contains("reset"));
}
