//! Process-boundary tests for target selection and operator workflows.

use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Output, Stdio},
    time::Duration,
};
use tokio::process::Command;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn command(home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_labby"));
    cmd.args(args)
        .env_clear()
        .env("HOME", home)
        .env("LABBY_HOME", home.join(".labby"))
        .env("XDG_CONFIG_HOME", home)
        .env("NO_COLOR", "1")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

async fn run(home: &Path, args: &[&str]) -> Output {
    tokio::time::timeout(Duration::from_secs(30), command(home, args).output())
        .await
        .expect("command must terminate without an interactive prompt")
        .unwrap()
}

fn success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("one JSON result")
}

#[tokio::test]
async fn configuration_inspection_and_context_reads_are_nonmutating_and_redacted() {
    let home = tempfile::tempdir().unwrap();
    let empty = success(&run(home.path(), &["--json", "context", "list"]).await);
    assert!(empty["contexts"].as_object().unwrap().is_empty());
    assert!(
        !home.path().join(".labby").exists(),
        "metadata read created an installation"
    );
    let root = home.path().join(".labby");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("config.toml");
    let content = "[custom]\ntoken = \"do-not-print-config-secret\"\n";
    std::fs::write(&path, content).unwrap();
    let before = std::fs::read(&path).unwrap();
    let shown = run(home.path(), &["--json", "config", "show"]).await;
    success(&shown);
    assert!(!String::from_utf8_lossy(&shown.stdout).contains("do-not-print-config-secret"));
    assert_eq!(
        success(&run(home.path(), &["--json", "config", "check"]).await)["valid"],
        true
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        1,
        "metadata inspection created locks or logs"
    );
    std::fs::write(&path, "[broken").unwrap();
    let invalid = run(home.path(), &["--json", "config", "check"]).await;
    assert!(!invalid.status.success());
    let error: Value = serde_json::from_slice(&invalid.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_param");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[broken");
}

#[tokio::test]
async fn auth_status_reports_saved_presence_without_claiming_online_authentication() {
    let home = tempfile::tempdir().unwrap();
    let value = success(
        &run(
            home.path(),
            &[
                "--json",
                "--server",
                "https://unreachable.example.invalid",
                "auth",
                "status",
            ],
        )
        .await,
    );
    assert_eq!(value["saved_session"], false);
    assert_eq!(value["verified_online"], false);
    let logout = success(
        &run(
            home.path(),
            &[
                "--json",
                "--server",
                "https://unreachable.example.invalid",
                "auth",
                "logout",
            ],
        )
        .await,
    );
    assert_eq!(logout["changed"], false);
    assert_eq!(logout["provider_revoked"], false);
    assert!(!home.path().join(".labby/cli-sessions").exists());
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn guided_inputs_are_never_requested_by_json_or_noninteractive_commands() {
    let home = tempfile::tempdir().unwrap();
    let gateway = MockServer::start().await;
    for args in [
        vec!["--json", "server", "add"],
        vec!["--json", "--no-input", "server", "add", "missing-transport"],
    ] {
        let output = run(home.path(), &args).await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["kind"], "invalid_param");
        assert_eq!(error["error"]["side_effects"], "none_expected");
    }
    let output = command(
        home.path(),
        &[
            "--json",
            "--server",
            &gateway.uri(),
            "server",
            "add",
            "preview",
            "--url",
            "https://example.com/mcp",
            "--dry-run",
        ],
    )
    .output()
    .await
    .unwrap();
    let value = success(&output);
    assert_eq!(value["executed"], false);
    assert_eq!(value["action"], "gateway.add");
    assert!(gateway.received_requests().await.unwrap().is_empty());
}

#[cfg(feature = "gateway")]
#[test]
fn duration_flags_use_explicit_units_and_preserve_backend_units() {
    use clap::Parser;
    use labby::cli::{
        Cli, Command,
        gateway::{GatewayCommand, GatewayEnrichArgs},
    };
    let cli =
        Cli::try_parse_from(["labby", "code", "hints", "preview", "--timeout", "2m"]).unwrap();
    let Command::Gateway(args) = cli.command.into_operation() else {
        unreachable!()
    };
    let GatewayCommand::Enrich(GatewayEnrichArgs { timeout_ms, .. }) = args.command else {
        unreachable!()
    };
    assert_eq!(timeout_ms, Some(120_000));
    for raw in ["30", "0s", "-1s", "999999999999999999999s", "25h"] {
        assert!(
            Cli::try_parse_from(["labby", "code", "hints", "preview", "--timeout", raw]).is_err()
        );
    }
    assert!(
        Cli::try_parse_from([
            "labby",
            "server",
            "auth",
            "login",
            "one",
            "--timeout",
            "500ms"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from(["labby", "server", "auth", "login", "one", "--timeout", "2m"]).is_ok()
    );
}

#[tokio::test]
async fn contexts_persist_in_existing_config_and_preserve_unrelated_content() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".labby");
    std::fs::create_dir(&root).unwrap();
    let config = root.join("config.toml");
    std::fs::write(&config, "# keep this comment\n[log]\ncolor = \"plain\"\n").unwrap();
    success(
        &run(
            home.path(),
            &[
                "--json",
                "context",
                "add",
                "homelab",
                "--server",
                "https://EXAMPLE.com:443/mcp",
                "--team-id",
                "team-one",
            ],
        )
        .await,
    );
    let first = std::fs::read_to_string(&config).unwrap();
    assert!(first.contains("# keep this comment"));
    assert!(first.contains("[log]"));
    let value: toml::Value = toml::from_str(&first).unwrap();
    assert_eq!(
        value["cli"]["contexts"]["homelab"]["server"].as_str(),
        Some("https://example.com/")
    );
    assert_eq!(
        value["cli"]["contexts"]["homelab"]["team_id"].as_str(),
        Some("team-one")
    );
    let duplicate = run(
        home.path(),
        &[
            "--json",
            "context",
            "add",
            "homelab",
            "--server",
            "https://other.example",
        ],
    )
    .await;
    assert!(!duplicate.status.success());
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        first,
        "duplicate add must not replace an existing context"
    );
    success(&run(home.path(), &["--json", "context", "use", "homelab"]).await);
    let selected = success(&run(home.path(), &["--json", "context", "get"]).await);
    assert_eq!(selected["name"], "homelab");
    assert_eq!(selected["server"], "https://example.com/");
    let blocked = run(home.path(), &["--json", "context", "remove", "homelab"]).await;
    assert!(
        !blocked.status.success(),
        "removing the active context must require an explicit selection change"
    );
    success(&run(home.path(), &["--json", "context", "clear"]).await);
    success(&run(home.path(), &["--json", "context", "remove", "homelab"]).await);
    let listed = success(&run(home.path(), &["--json", "context", "list"]).await);
    assert!(listed["contexts"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_context_urls_do_not_persist_credentials_or_select_a_target() {
    let home = tempfile::tempdir().unwrap();
    for raw in [
        "https://user:super-secret@example.com",
        "https://example.com?token=super-secret",
        "http://example.com",
        "file:///tmp/example",
    ] {
        let output = run(
            home.path(),
            &["--json", "context", "add", "unsafe", "--server", raw],
        )
        .await;
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("super-secret"));
        assert!(!home.path().join(".labby/config.toml").exists());
    }
}

#[cfg(feature = "gateway")]
async fn daemon() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/gateway/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"name":"gateway.reload"}])))
        .mount(&server)
        .await;
    server
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn context_pins_daemon_and_team_without_forwarding_another_targets_token() {
    let home = tempfile::tempdir().unwrap();
    let chosen = daemon().await;
    let other = daemon().await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&chosen)
        .await;
    success(
        &run(
            home.path(),
            &[
                "--json",
                "context",
                "add",
                "chosen",
                "--server",
                &chosen.uri(),
                "--team-id",
                "team-chosen",
            ],
        )
        .await,
    );
    let output = command(
        home.path(),
        &["--json", "--context", "chosen", "server", "list"],
    )
    .env("LABBY_SERVER_URL", other.uri())
    .env("LABBY_MCP_HTTP_TOKEN", "other-authoritys-token")
    .output()
    .await
    .unwrap();
    success(&output);
    assert!(other.received_requests().await.unwrap().is_empty());
    let requests = chosen.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| !request.headers.contains_key("authorization"))
    );
    let action = requests
        .iter()
        .find(|request| request.method == "POST")
        .expect("selected daemon received the operation");
    assert_eq!(
        action.headers.get("x-labby-team-id").unwrap(),
        "team-chosen"
    );
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn explicitly_selected_context_never_falls_back_to_local_execution() {
    let home = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    success(
        &run(
            home.path(),
            &["--json", "context", "add", "offline", "--server", &url],
        )
        .await,
    );
    let output = run(
        home.path(),
        &[
            "--json",
            "--context",
            "offline",
            "code",
            "run",
            "--code",
            "async () => 777",
        ],
    )
    .await;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let failure: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(failure["ok"], false);
    assert!(!home.path().join(".labby/auth.db").exists());
}

#[tokio::test]
async fn explicit_remote_target_is_rejected_for_local_host_operations() {
    let home = tempfile::tempdir().unwrap();
    let output = run(
        home.path(),
        &[
            "--json",
            "--context",
            "anything",
            "host",
            "service",
            "restart",
        ],
    )
    .await;
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_param");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("local")
    );
    assert_eq!(error["error"]["side_effects"], "none_expected");
}
