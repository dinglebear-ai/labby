use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mock_detectable_daemon() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/gateway/actions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!([{ "name": "gateway.reload" }])),
        )
        .mount(&server)
        .await;
    server
}

fn isolated_command(home: &std::path::Path, server: &MockServer) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env("LABBY_HOME", home)
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_SERVER_URL", server.uri())
        .env("LABBY_MCP_HTTP_TOKEN", "")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn opportunistic_command(home: &std::path::Path, server: &MockServer) -> Command {
    let url = url::Url::parse(&server.uri()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env("LABBY_HOME", home)
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_SERVER_URL", "")
        .env("LABBY_MCP_HTTP_TOKEN", "")
        .env("LABBY_MCP_HOST", url.host_str().unwrap())
        .env("LABBY_MCP_PORT", url.port().unwrap().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn assert_no_local_gateway_state(home: &std::path::Path) {
    assert!(!home.join("config.toml").exists());
    assert!(!home.join("auth.db").exists());
}

#[tokio::test]
async fn explicit_malformed_gateway_list_never_falls_back_locally() {
    let home = tempfile::tempdir().unwrap();
    let server = mock_detectable_daemon().await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let output = isolated_command(home.path(), &server)
        .args(["gateway", "list", "--json"])
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("decode_error"));
    assert_no_local_gateway_state(home.path());
}

#[tokio::test]
async fn explicit_code_mode_mcp_failure_never_executes_locally() {
    let home = tempfile::tempdir().unwrap();
    let server = mock_detectable_daemon().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let output = isolated_command(home.path(), &server)
        .args([
            "gateway",
            "code",
            "exec",
            "--code",
            "return await codemode.search({ query: 'tidewave' });",
        ])
        .output()
        .await
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Code Mode execution through configured Labby server")
    );
    assert_no_local_gateway_state(home.path());
}

#[tokio::test]
async fn explicit_stdio_bridge_failure_never_starts_standalone() {
    let home = tempfile::tempdir().unwrap();
    let server = mock_detectable_daemon().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let output = tokio::time::timeout(
        Duration::from_secs(3),
        isolated_command(home.path(), &server).arg("mcp").output(),
    )
    .await
    .expect("stdio bridge failure must be bounded")
    .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("bridge_transport_error"));
    assert_no_local_gateway_state(home.path());
}

#[tokio::test]
async fn opportunistic_malformed_gateway_list_preserves_local_fallback() {
    let home = tempfile::tempdir().unwrap();
    let server = mock_detectable_daemon().await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let output = opportunistic_command(home.path(), &server)
        .args(["gateway", "list", "--json"])
        .output()
        .await
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!([])
    );
}

#[tokio::test]
async fn opportunistic_code_mode_failure_preserves_trusted_local_fallback() {
    let home = tempfile::tempdir().unwrap();
    let server = mock_detectable_daemon().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let output = opportunistic_command(home.path(), &server)
        .args(["gateway", "code", "exec", "--code", "return 7;"])
        .output()
        .await
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains('7'));
}
