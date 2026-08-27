#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Output};

#[cfg(unix)]
use serde_json::Value;
use serde_json::json;
#[cfg(unix)]
#[path = "support/lib.rs"]
mod support;
#[cfg(all(unix, feature = "gateway"))]
use wiremock::matchers::{method, path};
#[cfg(all(unix, feature = "gateway"))]
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg(unix)]
fn command(home: &Path) -> Command {
    support::isolated_command(home)
}

#[cfg(unix)]
fn write_config(home: &Path, body: &str) {
    let lab_home = home.join(".labby");
    std::fs::create_dir_all(&lab_home).unwrap();
    std::fs::write(lab_home.join("config.toml"), body).unwrap();
}

#[cfg(unix)]
fn run_json(command: &mut Command) -> (Output, Value) {
    let output = command.output().expect("run labby doctor proxy");
    let report = serde_json::from_slice(&output.stdout);
    assert!(
        report.is_ok(),
        "doctor output was not JSON: {:?}; stdout={}; stderr={}",
        report.as_ref().err(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = match report {
        Ok(report) => report,
        Err(_) => Value::Null,
    };
    (output, report)
}

#[cfg(unix)]
fn finding<'a>(report: &'a Value, check: &str) -> &'a Value {
    let finding = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["check"] == check);
    assert!(finding.is_some(), "missing finding {check}: {report}");
    finding.unwrap_or(report)
}

#[cfg(unix)]
fn assert_severity(report: &Value, check: &str, severity: &str) {
    assert_eq!(finding(report, check)["severity"], severity, "{report}");
}

#[tokio::test]
async fn doctor_catalog_advertises_preflight_without_changing_proxy_check_schema() {
    let catalog = labby::dispatch::doctor::dispatch("help", json!({}))
        .await
        .unwrap();
    assert!(catalog["actions"].as_array().unwrap().iter().any(|action| {
        action["name"] == "proxy.preflight"
            && action["destructive"] == false
            && action["params"].as_array().unwrap().is_empty()
    }));

    let schema = labby::dispatch::doctor::dispatch("schema", json!({"action": "proxy.check"}))
        .await
        .unwrap();
    assert!(
        schema["params"]
            .as_array()
            .unwrap()
            .iter()
            .any(|param| { param["name"] == "route" && param["required"] == true })
    );
}

#[cfg(unix)]
fn executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn launcher_path(root: &Path, include_python: bool) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    executable(&bin.join("node"), "#!/bin/sh\nexit 0\n");
    if include_python {
        executable(&bin.join("python3"), "#!/bin/sh\nexit 0\n");
    }
    bin
}

#[cfg(all(unix, feature = "gateway"))]
fn fake_tailscale(root: &Path, version: &str, status: &str, serve_status: &str) -> PathBuf {
    let executable_path = root.join("tailscale");
    let calls = root.join("tailscale.calls");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> '{calls}'
case "$*" in
  version) printf '%s\n' '{version}'; exit 0 ;;
  'status --json') printf '%s\n' '{status}'; exit 0 ;;
  'serve status --json') printf '%s\n' '{serve_status}'; exit 0 ;;
  *) printf '%s\n' 'mutation attempted' >&2; exit 97 ;;
esac
"#,
        calls = calls.display(),
    );
    executable(&executable_path, &script);
    executable_path
}

#[cfg(unix)]
#[test]
fn doctor_proxy_without_route_runs_local_preflight_and_skips_tailscale_for_local_exposure() {
    let home = tempfile::tempdir().expect("temp home");
    write_config(
        home.path(),
        "[proxy]\nexposure = \"local\"\nauth = \"none\"\npath = \"/mcp\"\nport = \"random\"\nport_range_start = 50000\nport_range_end = 51000\n",
    );

    let (output, report) = run_json(
        command(home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(home.path(), true))
            .env("LABBY_TAILSCALE_BIN", "/definitely/must/not/run/tailscale"),
    );

    #[cfg(feature = "gateway")]
    assert_eq!(output.status.code(), Some(1));
    #[cfg(not(feature = "gateway"))]
    assert_eq!(output.status.code(), Some(2));
    assert_severity(&report, "proxy:config", "ok");
    assert_severity(&report, "proxy:launcher-node", "ok");
    assert_severity(&report, "proxy:launcher-python3", "ok");
    assert_severity(&report, "proxy:auth-none", "warn");
    assert_severity(&report, "proxy:tailscale-skipped", "ok");
    #[cfg(not(feature = "gateway"))]
    assert_severity(&report, "proxy:gateway-feature", "fail");
    assert!(
        !report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["check"]
                    .as_str()
                    .is_some_and(|check| check.starts_with("proxy:tailscale-"))
                    && finding["check"] != "proxy:tailscale-skipped"
            })
    );
}

#[cfg(all(unix, not(feature = "gateway")))]
#[test]
fn no_gateway_proxy_preflight_fails_even_when_local_bearer_dependencies_are_healthy() {
    let home = tempfile::tempdir().expect("temp home");
    write_config(
        home.path(),
        "[proxy]\nexposure = \"local\"\nauth = \"bearer\"\n",
    );

    let (output, report) = run_json(
        command(home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(home.path(), true))
            .env("LABBY_PROXY_BEARER_TOKEN", "preflight-test-token"),
    );

    assert_eq!(output.status.code(), Some(2));
    assert_severity(&report, "proxy:gateway-feature", "fail");
    assert!(
        finding(&report, "proxy:gateway-feature")["message"]
            .as_str()
            .is_some_and(|message| message.contains("gateway feature"))
    );
}

#[cfg(unix)]
#[test]
fn proxy_preflight_reports_invalid_persisted_config_before_dependencies() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        "[proxy]\nexposure = \"local\"\nauth = \"none\"\npath = \"relative\"\nport_range_start = 51000\nport_range_end = 50000\n",
    );

    let output = command(home.path())
        .args(["--json", "doctor", "proxy"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid config"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("tailscale"));
}

#[cfg(unix)]
#[test]
fn proxy_preflight_reports_each_child_launcher_deterministically() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        "[proxy]\nexposure = \"local\"\nauth = \"bearer\"\n",
    );
    let secret = "doctor-secret-must-not-be-rendered";
    let (output, report) = run_json(
        command(home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(home.path(), false))
            .env("LABBY_PROXY_BEARER_TOKEN", secret),
    );

    assert_eq!(output.status.code(), Some(2));
    assert_severity(&report, "proxy:launcher-node", "ok");
    assert_severity(&report, "proxy:launcher-python3", "fail");
    assert_severity(&report, "proxy:bearer-secret", "ok");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn bearer_preflight_loads_secret_from_persisted_dotenv_without_rendering_it() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        "[proxy]\nexposure = \"local\"\nauth = \"bearer\"\nbearer_token_env = \"CUSTOM_PROXY_TOKEN\"\n",
    );
    let secret = "persisted-secret-must-not-be-rendered";
    std::fs::write(
        home.path().join(".labby/.env"),
        format!("CUSTOM_PROXY_TOKEN={secret}\n"),
    )
    .unwrap();

    let (output, report) = run_json(
        command(home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(home.path(), true)),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_severity(&report, "proxy:bearer-secret", "ok");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));

    let (missing_output, missing_report) = run_json(
        command(home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(home.path(), true))
            .env_remove("CUSTOM_PROXY_TOKEN"),
    );
    assert!(missing_output.status.success());
    assert_severity(&missing_report, "proxy:bearer-secret", "ok");

    let absent_home = tempfile::tempdir().unwrap();
    write_config(
        absent_home.path(),
        "[proxy]\nexposure = \"local\"\nauth = \"bearer\"\nbearer_token_env = \"ABSENT_PROXY_TOKEN\"\n",
    );
    let (absent_output, absent_report) = run_json(
        command(absent_home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(absent_home.path(), true))
            .env_remove("ABSENT_PROXY_TOKEN"),
    );
    assert_eq!(absent_output.status.code(), Some(2));
    assert_severity(&absent_report, "proxy:bearer-secret", "fail");
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn tailscale_preflight_checks_version_identity_dns_and_https_without_mutation() {
    let home = tempfile::tempdir().unwrap();
    write_config(home.path(), "[proxy]\nport = 54000\n");
    let tailscale = fake_tailscale(
        home.path(),
        "1.98.10",
        r#"{"BackendState":"Running","Self":{"Online":true,"DNSName":"node.example.ts.net.","TailscaleIPs":["100.64.0.1"]},"Peer":{}}"#,
        r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"node.example.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:8765"}}}},"AllowFunnel":{}}"#,
    );

    let (output, report) = run_json(
        command(home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(home.path(), true))
            .env("LABBY_TAILSCALE_BIN", &tailscale),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for check in [
        "proxy:tailscale-version",
        "proxy:tailscale-running",
        "proxy:tailscale-online",
        "proxy:tailscale-dns",
        "proxy:tailscale-https-serve",
    ] {
        assert_severity(&report, check, "ok");
    }
    let calls = std::fs::read_to_string(home.path().join("tailscale.calls")).unwrap();
    assert_eq!(calls, "version\nstatus --json\nserve status --json\n");
    assert!(!calls.contains("--yes"));
    assert!(!calls.contains("off"));
    assert!(!calls.contains("reset"));
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn tailscale_preflight_attributes_status_failures_to_the_exact_category() {
    let cases = [
        (
            "stopped",
            r#"{"BackendState":"Stopped","Self":{"Online":true,"DNSName":"node.example.ts.net."}}"#,
            "proxy:tailscale-running",
        ),
        (
            "offline",
            r#"{"BackendState":"Running","Self":{"Online":false,"DNSName":"node.example.ts.net."}}"#,
            "proxy:tailscale-online",
        ),
        (
            "no-dns",
            r#"{"BackendState":"Running","Self":{"Online":true,"DNSName":""}}"#,
            "proxy:tailscale-dns",
        ),
    ];
    for (name, status, expected_check) in cases {
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "[proxy]\n");
        let tailscale = fake_tailscale(home.path(), "1.98.10", status, r#"{"TCP":{},"Web":{}}"#);
        let (output, report) = run_json(
            command(home.path())
                .args(["--json", "doctor", "proxy"])
                .env("PATH", launcher_path(home.path(), true))
                .env("LABBY_TAILSCALE_BIN", tailscale),
        );
        assert_eq!(output.status.code(), Some(2), "case {name}");
        assert_severity(&report, expected_check, "fail");
    }
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn tailscale_preflight_reports_executable_version_and_https_serve_failures() {
    let missing_home = tempfile::tempdir().unwrap();
    write_config(missing_home.path(), "[proxy]\n");
    let (missing_output, missing_report) = run_json(
        command(missing_home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(missing_home.path(), true))
            .env(
                "LABBY_TAILSCALE_BIN",
                missing_home.path().join("missing-tailscale"),
            ),
    );
    assert_eq!(missing_output.status.code(), Some(2));
    assert_severity(&missing_report, "proxy:tailscale-version", "fail");
    assert!(
        finding(&missing_report, "proxy:tailscale-running")["message"]
            .as_str()
            .unwrap()
            .contains("executable/version")
    );

    let empty_version_home = tempfile::tempdir().unwrap();
    write_config(empty_version_home.path(), "[proxy]\n");
    let tailscale = fake_tailscale(
        empty_version_home.path(),
        "",
        r#"{"BackendState":"Running","Self":{"Online":true,"DNSName":"node.example.ts.net."}}"#,
        r#"{"TCP":{},"Web":{}}"#,
    );
    let (empty_output, empty_report) = run_json(
        command(empty_version_home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(empty_version_home.path(), true))
            .env("LABBY_TAILSCALE_BIN", tailscale),
    );
    assert_eq!(empty_output.status.code(), Some(2));
    assert_severity(&empty_report, "proxy:tailscale-version", "fail");

    let bad_serve_home = tempfile::tempdir().unwrap();
    write_config(bad_serve_home.path(), "[proxy]\n");
    let tailscale = fake_tailscale(
        bad_serve_home.path(),
        "1.98.10",
        r#"{"BackendState":"Running","Self":{"Online":true,"DNSName":"node.example.ts.net."}}"#,
        "not-json",
    );
    let (serve_output, serve_report) = run_json(
        command(bad_serve_home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(bad_serve_home.path(), true))
            .env("LABBY_TAILSCALE_BIN", tailscale),
    );
    assert_eq!(serve_output.status.code(), Some(2));
    assert_severity(&serve_report, "proxy:tailscale-running", "ok");
    assert_severity(&serve_report, "proxy:tailscale-online", "ok");
    assert_severity(&serve_report, "proxy:tailscale-dns", "ok");
    assert_severity(&serve_report, "proxy:tailscale-https-serve", "fail");
}

#[cfg(all(unix, feature = "gateway"))]
async fn mount_daemon(server: &MockServer, actions: Value, metadata: Value, jwks: Value) {
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ok"})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/labby.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "paletteCatalogUrl": "catalog",
            "paletteExecuteUrl": "execute"
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/gateway/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(actions))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(metadata))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
        .mount(server)
        .await;
}

#[cfg(all(unix, feature = "gateway"))]
async fn oauth_doctor(home: &Path, server: &MockServer) -> (Output, Value) {
    let url = url::Url::parse(&server.uri()).unwrap();
    write_config(
        home,
        &format!(
            "[proxy]\nexposure = \"local\"\nauth = \"oauth\"\n\n[auth]\nmode = \"oauth\"\npublic_url = \"{}\"\nsqlite_path = \"{}\"\nkey_path = \"{}\"\nadmin_email = \"admin@example.com\"\ngoogle_client_id = \"test-client\"\ngoogle_client_secret = \"test-secret\"\n",
            server.uri(),
            home.join("auth.db").display(),
            home.join("auth-jwt.pem").display(),
        ),
    );
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["--json", "doctor", "proxy"])
        .env("HOME", home)
        .env("LABBY_HOME", home.join(".labby"))
        .env("LABBY_LOG_DIR", home.join("logs"))
        .env("PATH", launcher_path(home, true))
        .env("LABBY_MCP_HTTP_HOST", url.host_str().unwrap())
        .env("LABBY_MCP_HTTP_PORT", url.port().unwrap().to_string())
        .env("LABBY_TOKEN_ENCRYPTION_KEY", "11".repeat(32))
        .output()
        .await
        .unwrap();
    let report = serde_json::from_slice(&output.stdout);
    assert!(
        report.is_ok(),
        "oauth doctor output was not JSON: {:?}; stdout={}; stderr={}",
        report.as_ref().err(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = match report {
        Ok(report) => report,
        Err(_) => Value::Null,
    };
    (output, report)
}

#[cfg(all(unix, feature = "gateway"))]
#[tokio::test(flavor = "multi_thread")]
async fn oauth_preflight_checks_stable_issuer_daemon_lease_actions_metadata_and_jwks() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mount_daemon(
        &server,
        json!([
            {"name": "gateway.oauth.resource_lease.create"},
            {"name": "gateway.oauth.resource_lease.renew"},
            {"name": "gateway.oauth.resource_lease.release"}
        ]),
        json!({"issuer": server.uri(), "jwks_uri": format!("{}/jwks", server.uri())}),
        json!({"keys": []}),
    )
    .await;

    let (output, report) = oauth_doctor(home.path(), &server).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for check in [
        "proxy:oauth-stable-issuer",
        "proxy:oauth-daemon",
        "proxy:oauth-lease-create",
        "proxy:oauth-lease-renew",
        "proxy:oauth-lease-release",
        "proxy:oauth-issuer-metadata",
        "proxy:oauth-jwks",
    ] {
        assert_severity(&report, check, "ok");
    }
}

#[cfg(all(unix, feature = "gateway"))]
#[tokio::test(flavor = "multi_thread")]
async fn oauth_preflight_reports_missing_lease_action_and_bad_issuer_metadata_deterministically() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mount_daemon(
        &server,
        json!([
            {"name": "gateway.oauth.resource_lease.create"},
            {"name": "gateway.oauth.resource_lease.renew"}
        ]),
        json!({"issuer": "https://wrong.example", "jwks_uri": format!("{}/jwks", server.uri())}),
        json!({"keys": []}),
    )
    .await;

    let (output, report) = oauth_doctor(home.path(), &server).await;
    assert_eq!(output.status.code(), Some(2));
    assert_severity(&report, "proxy:oauth-lease-create", "ok");
    assert_severity(&report, "proxy:oauth-lease-renew", "ok");
    assert_severity(&report, "proxy:oauth-lease-release", "fail");
    assert_severity(&report, "proxy:oauth-issuer-metadata", "fail");
    assert_severity(&report, "proxy:oauth-jwks", "fail");
    assert!(
        finding(&report, "proxy:oauth-jwks")["message"]
            .as_str()
            .unwrap()
            .contains("issuer metadata")
    );
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn oauth_preflight_reports_missing_stable_issuer_and_unreachable_daemon_dependencies() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        "[proxy]\nexposure = \"local\"\nauth = \"oauth\"\n\n[auth]\nmode = \"oauth\"\n",
    );
    let (output, report) = run_json(
        command(home.path())
            .args(["--json", "doctor", "proxy"])
            .env("PATH", launcher_path(home.path(), true))
            .env("LABBY_MCP_HTTP_HOST", "127.0.0.1")
            .env("LABBY_MCP_HTTP_PORT", "9")
            .env("LABBY_TOKEN_ENCRYPTION_KEY", "11".repeat(32)),
    );
    assert_eq!(output.status.code(), Some(2));
    assert_severity(&report, "proxy:oauth-stable-issuer", "fail");
    assert_severity(&report, "proxy:oauth-daemon", "fail");
    for check in [
        "proxy:oauth-lease-create",
        "proxy:oauth-lease-renew",
        "proxy:oauth-lease-release",
        "proxy:oauth-issuer-metadata",
        "proxy:oauth-jwks",
    ] {
        assert_severity(&report, check, "fail");
        assert!(
            finding(&report, check)["message"]
                .as_str()
                .unwrap()
                .contains("live daemon discovery")
        );
    }
}

#[cfg(all(unix, feature = "gateway"))]
#[tokio::test(flavor = "multi_thread")]
async fn oauth_preflight_attributes_invalid_jwks_after_valid_issuer_metadata() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mount_daemon(
        &server,
        json!([
            {"name": "gateway.oauth.resource_lease.create"},
            {"name": "gateway.oauth.resource_lease.renew"},
            {"name": "gateway.oauth.resource_lease.release"}
        ]),
        json!({"issuer": server.uri(), "jwks_uri": format!("{}/jwks", server.uri())}),
        json!({"not_keys": []}),
    )
    .await;

    let (output, report) = oauth_doctor(home.path(), &server).await;
    assert_eq!(output.status.code(), Some(2));
    assert_severity(&report, "proxy:oauth-issuer-metadata", "ok");
    assert_severity(&report, "proxy:oauth-jwks", "fail");
    assert!(
        finding(&report, "proxy:oauth-jwks")["message"]
            .as_str()
            .unwrap()
            .contains("JWKS")
    );
}

#[cfg(unix)]
#[test]
fn routed_proxy_probe_remains_compatible_and_does_not_run_local_preflight() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args([
            "--json",
            "doctor",
            "proxy",
            "--app-url",
            "http://doctor-app.invalid",
            "--mcp-url",
            "http://doctor-mcp.invalid",
            "--route",
            "/telemetry",
        ])
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path().join(".labby"))
        .env("LABBY_LOG_DIR", home.path().join("logs"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_severity(&report, "proxy:app-health", "fail");
    assert_eq!(
        finding(&report, "proxy:oauth-challenge")["severity"],
        "fail"
    );
    assert!(
        !report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["check"] == "proxy:config" || finding["check"] == "proxy:tailscale-skipped"
            })
    );
}
