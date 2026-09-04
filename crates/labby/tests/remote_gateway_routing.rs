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
    let mut command = small_stack_command(home);
    command
        .env("LABBY_HOME", home)
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_SERVER_URL", "")
        .env("LABBY_MCP_HTTP_TOKEN", "")
        .env("LABBY_MCP_HTTP_HOST", url.host_str().unwrap())
        .env("LABBY_MCP_HTTP_PORT", url.port().unwrap().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn assert_no_local_gateway_state(home: &std::path::Path) {
    assert!(!home.join("config.toml").exists());
    assert!(!home.join("auth.db").exists());
}

fn small_stack_command(home: &std::path::Path) -> Command {
    // Match Windows' default main-thread stack on Unix as well. The command
    // dispatcher previously overflowed this budget before entering any arm.
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("sh");
        command
            .args(["-c", "ulimit -s 1024 && exec \"$@\"", "labby-small-stack"])
            .arg(env!("CARGO_BIN_EXE_labby"));
        command
    };
    #[cfg(not(unix))]
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .kill_on_drop(true)
        .env("LABBY_HOME", home)
        .env("LABBY_LOG_DIR", home.join("logs"))
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_SERVER_URL", "")
        .env("LABBY_MCP_HTTP_TOKEN", "")
        .env("LABBY_MCP_HTTP_HOST", "127.0.0.1")
        .env("LABBY_MCP_HTTP_PORT", "9");
    command
}

#[tokio::test]
async fn local_gateway_list_fits_a_one_mebibyte_main_stack() {
    let home = tempfile::tempdir().unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        small_stack_command(home.path())
            .args(["gateway", "list", "--json"])
            .output(),
    )
    .await
    .expect("local CLI dispatch must finish promptly")
    .unwrap();
    assert!(
        output.status.success(),
        "small-stack CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value.is_array(),
        "gateway list must retain its JSON contract"
    );
}

#[tokio::test]
async fn local_code_mode_execution_fits_a_one_mebibyte_main_stack() {
    let home = tempfile::tempdir().unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        small_stack_command(home.path())
            .args(["gateway", "code", "exec", "--code", "return 7;", "--json"])
            .output(),
    )
    .await
    .expect("local Code Mode execution must finish promptly")
    .unwrap();
    assert!(
        output.status.success(),
        "small-stack Code Mode failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["result"], 7,
        "Code Mode must return the executed result"
    );
}

#[tokio::test]
async fn local_stdio_discovery_fits_a_one_mebibyte_main_stack() {
    let home = tempfile::tempdir().unwrap();
    assert_stdio_discovery(small_stack_command(home.path())).await;
}

#[cfg(unix)]
#[tokio::test]
async fn local_stdio_discovery_retains_headroom_below_one_mebibyte() {
    let home = tempfile::tempdir().unwrap();
    // The native Windows test remains authoritative for its ABI. This
    // stricter Unix budget catches growth before it consumes that headroom.
    let mut command = Command::new("sh");
    command
        .args(["-c", "ulimit -s 768 && exec \"$@\"", "labby-stack-headroom"])
        .arg(env!("CARGO_BIN_EXE_labby"))
        .kill_on_drop(true)
        .env("LABBY_HOME", home.path())
        .env("LABBY_LOG_DIR", home.path().join("logs"))
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_SERVER_URL", "")
        .env("LABBY_MCP_HTTP_TOKEN", "")
        .env("LABBY_MCP_HTTP_HOST", "127.0.0.1")
        .env("LABBY_MCP_HTTP_PORT", "9");
    assert_stdio_discovery(command).await;
}

async fn assert_stdio_discovery(mut command: Command) {
    use rmcp::model::ProtocolVersion;
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};

    command.arg("mcp");
    let transport = rmcp::transport::TokioChildProcess::new(command).unwrap();
    let client = tokio::time::timeout(
        Duration::from_secs(30),
        ().serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ),
    )
    .await
    .expect("small-stack stdio discovery must finish promptly")
    .expect("small-stack stdio discovery must succeed");
    let page = tokio::time::timeout(Duration::from_secs(10), client.list_tools(None))
        .await
        .expect("tools/list must finish promptly")
        .unwrap();
    let exposes_gateway = page.tools.iter().any(|tool| tool.name == "gateway");
    let resource = tokio::time::timeout(
        Duration::from_secs(10),
        client.read_resource(rmcp::model::ReadResourceRequestParams::new("lab://catalog")),
    )
    .await
    .expect("resources/read must finish promptly")
    .expect("catalog resource must remain readable");
    assert!(
        serde_json::to_value(resource).unwrap()["contents"]
            .as_array()
            .is_some_and(|contents| !contents.is_empty())
    );
    let prompt = tokio::time::timeout(
        Duration::from_secs(10),
        client.get_prompt(
            rmcp::model::GetPromptRequestParams::new("service-discover").with_arguments(
                serde_json::json!({"service": "gateway"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ),
    )
    .await
    .expect("prompts/get must finish promptly")
    .expect("built-in prompt must remain readable");
    assert!(
        serde_json::to_value(prompt).unwrap()["messages"]
            .as_array()
            .is_some_and(|messages| !messages.is_empty())
    );
    let completion = tokio::time::timeout(
        Duration::from_secs(10),
        client.complete(rmcp::model::CompleteRequestParams::new(
            rmcp::model::Reference::for_prompt("service-discover"),
            rmcp::model::ArgumentInfo::new("service", "gate"),
        )),
    )
    .await
    .expect("completion must finish promptly")
    .expect("prompt service completion must succeed");
    assert!(
        completion
            .completion
            .values
            .iter()
            .any(|value| value == "gateway")
    );
    tokio::time::timeout(Duration::from_secs(10), client.cancel())
        .await
        .expect("stdio shutdown must finish promptly")
        .unwrap();
    assert!(
        exposes_gateway,
        "stdio discovery must retain the gateway tool"
    );
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

    let output = tokio::time::timeout(
        Duration::from_secs(30),
        opportunistic_command(home.path(), &server)
            .args(["gateway", "code", "exec", "--code", "return 7;"])
            .output(),
    )
    .await
    .expect("opportunistic Code Mode fallback must finish promptly")
    .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains('7'));
}
