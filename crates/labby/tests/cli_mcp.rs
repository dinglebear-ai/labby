#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result};
use rmcp::ClientServiceExt as _;
use rmcp::model::{CallToolRequestParams, ProtocolVersion};
use rmcp::service::ClientLifecycleMode;
use rmcp::transport::TokioChildProcess;
use serde_json::{Map, Value, json};
use tokio::process::Command;

const HELLO_MANIFEST: &str = r#"
[[tools]]
name = "hello"
description = "Say hello with an approved CLI"
executable = "/usr/bin/printf"
args = ["%s"]
read_only = true

[tools.choices]
world = ["Hello world"]
"#;

fn manifest_fixture() -> Result<(tempfile::TempDir, PathBuf)> {
    // Keep the test fixture below a project-owned directory; a production
    // manifest does not belong in an untrusted temporary directory.
    let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(&manifest, HELLO_MANIFEST)?;
    Ok((dir, manifest))
}

fn hello_arguments(value: &str) -> Map<String, Value> {
    Map::from_iter([("choice".to_owned(), json!(value))])
}

#[tokio::test]
async fn direct_cli_mcp_negotiates_lists_and_calls_without_gateway() -> Result<()> {
    let (_dir, manifest) = manifest_fixture()?;
    let mut process = Command::new(env!("CARGO_BIN_EXE_labby"));
    process
        .env_clear()
        .env("LABBY_HOME", "/unavailable-should-not-be-read")
        .args(["cli", "serve", "--manifest"])
        .arg(manifest);
    let transport = TokioChildProcess::new(process)?;
    tokio::time::timeout(Duration::from_secs(20), async {
        let client = ().serve_with_lifecycle(transport, ClientLifecycleMode::Initialize).await?;
        let info = client
            .peer_info()
            .context("server identity was not negotiated")?;
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_none());
        assert!(info.capabilities.prompts.is_none());
        assert!(info.protocol_version >= ProtocolVersion::V_2025_11_25);
        let listing = client.list_tools(None).await?;
        assert_eq!(listing.tools.len(), 1);
        assert_eq!(listing.tools[0].name, "hello");
        assert_eq!(
            listing.tools[0].input_schema["properties"]["choice"]["enum"],
            json!(["world"])
        );
        let passed = client
            .call_tool(CallToolRequestParams::new("hello").with_arguments(hello_arguments("world")))
            .await?;
        assert_eq!(
            serde_json::to_value(&passed)?["content"][0]["text"],
            "Hello world"
        );
        assert!(
            client
                .call_tool(
                    CallToolRequestParams::new("hello")
                        .with_arguments(hello_arguments("--unapproved"))
                )
                .await
                .is_err()
        );
        client.cancel().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    Ok(())
}

#[cfg(feature = "gateway")]
#[path = "support/evidence.rs"]
#[allow(dead_code)]
mod evidence;
#[cfg(feature = "gateway")]
#[path = "support/live_labby.rs"]
#[allow(dead_code)]
mod live_labby;

#[cfg(feature = "gateway")]
#[tokio::test]
async fn cli_mcp_is_an_ordinary_gateway_upstream() -> Result<()> {
    const TOKEN: &str = "cli-mcp-qualification-disposable-token";
    let (manifest_root, manifest) = manifest_fixture()?;
    let outside_manifest = manifest_fixture()?;
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_labby")).canonicalize()?;
    let config = format!(
        "[code_mode]\nenabled = true\n\n[gateway]\nextra_stdio_commands = [{}]\n",
        serde_json::to_string(&binary)?,
    );
    drop(rustls::crypto::ring::default_provider().install_default());
    let parent = std::env::temp_dir().join("labby-live-e2e");
    std::fs::create_dir_all(&parent)?;
    let installation = tempfile::tempdir_in(parent)?;
    let labby_home = installation.path().join("labby-home");
    std::fs::create_dir_all(&labby_home)?;
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        std::fs::set_permissions(&labby_home, std::fs::Permissions::from_mode(0o700))?;
        let mut env = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(labby_home.join(".env"))?;
        write!(
            env,
            "LABBY_AUTH_MODE=bearer\nLABBY_MCP_HTTP_TOKEN={TOKEN}\n"
        )?;
    }
    let mut bootstrap = Command::from(live_labby::isolated_command(installation.path()));
    let bootstrapped = bootstrap
        .env("LABBY_HOME", &labby_home)
        .args(["setup", "--bootstrap-static-owner"])
        .kill_on_drop(true)
        .output()
        .await?;
    assert!(
        bootstrapped.status.success(),
        "{}",
        String::from_utf8_lossy(&bootstrapped.stderr)
    );
    // The normal bootstrap reads a private .env. The daemon receives the
    // same disposable credential only in its process environment. Remove the
    // owned bootstrap inputs/backups before the harness checks for leaks.
    for entry in std::fs::read_dir(&labby_home)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".env" || name.to_string_lossy().starts_with(".env.bak.") {
            std::fs::remove_file(entry.path())?;
        }
    }
    let gateway = live_labby::LiveLabbyBuilder::new()
        .existing_root(installation.path())
        .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
        .env("LABBY_CLI_MCP_MANIFEST_ROOT", manifest_root.path())
        .config(config)
        .start()
        .await
        .map_err(anyhow::Error::msg)?;
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        let url = &gateway.connection().base_url;
        let cli_child = |args: &[&str]| {
            let mut command = Command::from(live_labby::isolated_command(gateway.root()));
            command
                .env("LABBY_SERVER_URL", url)
                .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
                .args(["--team-id", live_labby::LiveLabbyGuard::HARNESS_TEAM_ID])
                .args(args)
                .kill_on_drop(true);
            command
        };
        let http = reqwest::Client::builder().no_proxy().build()?;
        let registration = |params: Value| {
            http.post(format!("{url}/v1/gateway")).json(&json!({
                "action": "gateway.add",
                "params": params
            }))
        };
        let params = json!({
            "spec": {"name": "cli-presets", "display_name": "Approved CLI"},
            "adapter": {"kind": "cli", "manifest_path": manifest},
            "confirm": true
        });
        let unauthenticated = registration(params.clone()).send().await?;
        assert!(matches!(unauthenticated.status().as_u16(), 401 | 403));
        let authorized = |params: Value| {
            registration(params).bearer_auth(TOKEN).header(
                "x-labby-team-id",
                live_labby::LiveLabbyGuard::HARNESS_TEAM_ID,
            )
        };
        for (pointer, replacement) in [
            ("/confirm", json!(false)),
            ("/adapter/manifest_path", json!(outside_manifest.1)),
            (
                "/spec",
                json!({"name": "injected", "command": "/usr/bin/printf"}),
            ),
        ] {
            let mut bad = params.clone();
            *bad.pointer_mut(pointer)
                .context("bad fixture JSON pointer")? = replacement;
            let refused = authorized(bad).send().await?;
            assert_eq!(refused.status().as_u16(), 400, "{}", refused.text().await?);
        }
        let created = authorized(params).send().await?;
        let status = created.status();
        let body = created.text().await?;
        assert!(status.is_success(), "HTTP {status}: {body}");
        assert!(body.contains("Approved CLI"), "{body}");
        let response = http
            .post(format!("{url}/v1/gateway"))
            .bearer_auth(TOKEN)
            .header(
                "x-labby-team-id",
                live_labby::LiveLabbyGuard::HARNESS_TEAM_ID,
            )
            .json(&json!({"action": "gateway.list", "params": {}}))
            .send()
            .await?;
        let status = response.status();
        let servers: Value = response.json().await?;
        assert!(status.is_success(), "HTTP {status}: {servers}");
        assert!(servers.to_string().contains("cli-presets"), "{servers}");
        let test = cli_child(&["server", "test", "cli-presets", "--json"])
            .output()
            .await?;
        assert!(
            test.status.success(),
            "{}",
            String::from_utf8_lossy(&test.stderr)
        );
        let search = cli_child(&["code", "search", "cli-presets hello", "--json"])
            .output()
            .await?;
        assert!(
            search.status.success(),
            "{}",
            String::from_utf8_lossy(&search.stderr)
        );
        assert!(
            String::from_utf8_lossy(&search.stdout).contains("hello"),
            "{}",
            String::from_utf8_lossy(&search.stdout),
        );

        // The ordinary CLI path invokes the gateway's Streamable HTTP MCP
        // surface, rather than importing or invoking the preset itself.
        let tool_call = cli_child(&[
            "code",
            "run",
            "--code",
            "async () => await callTool('cli-presets::hello', { choice: 'world' })",
            "--json",
        ])
        .output()
        .await?;
        assert!(
            tool_call.status.success(),
            "{}",
            String::from_utf8_lossy(&tool_call.stderr)
        );
        assert!(
            String::from_utf8_lossy(&tool_call.stdout).contains("Hello world"),
            "{}",
            String::from_utf8_lossy(&tool_call.stdout),
        );
        let palette_id = "mcp:cli-presets::hello";
        let team = live_labby::LiveLabbyGuard::HARNESS_TEAM_ID;
        let descriptor = http
            .get(format!("{url}/v1/palette/descriptor"))
            .bearer_auth(TOKEN)
            .header("x-labby-team-id", team)
            .query(&[("id", palette_id)])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let contract = descriptor["contractHash"]
            .as_str()
            .context("palette returned no CLI tool contract")?;
        let executed = http
            .post(format!("{url}/v1/palette/execute"))
            .bearer_auth(TOKEN)
            .header("x-labby-team-id", team)
            .json(&json!({
                "id": palette_id,
                "params": hello_arguments("world"),
                "expectedContractHash": contract,
            }))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        assert!(
            executed["result"].to_string().contains("Hello world"),
            "{executed}"
        );

        // The root gateway deliberately hides raw upstream tools with Code
        // Mode enabled. Validate direct MCP against its ordinary raw setting.
        let disable = cli_child(&["code", "disable", "--json"]).output().await?;
        assert!(
            disable.status.success(),
            "{}",
            String::from_utf8_lossy(&disable.stderr)
        );
        use reqwest::header::{HeaderName, HeaderValue};
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
        };
        let config = StreamableHttpClientTransportConfig::with_uri(format!("{url}/mcp"))
            .auth_header(TOKEN)
            .custom_headers(std::collections::HashMap::from([(
                HeaderName::from_static("x-labby-team-id"),
                HeaderValue::from_static(team),
            )]));
        let worker = StreamableHttpClientWorker::new(http, config);
        let peer = ().serve_with_lifecycle(worker, ClientLifecycleMode::Initialize).await?;
        let listed = peer.list_tools(None).await?;
        let tool = listed
            .tools
            .iter()
            .find(|tool| tool.name.ends_with("hello"))
            .context("CLI tool not discoverable on the ordinary gateway MCP surface")?;
        let called = peer
            .call_tool(
                CallToolRequestParams::new(tool.name.clone())
                    .with_arguments(hello_arguments("world")),
            )
            .await?;
        assert_eq!(
            serde_json::to_value(&called)?["content"][0]["text"],
            "Hello world"
        );
        peer.cancel().await?;
        Ok::<(), anyhow::Error>(())
    })
    .await;
    let cleanup = gateway.finish().await;
    result??;
    assert!(cleanup.is_clean(), "{cleanup:?}");
    Ok(())
}
