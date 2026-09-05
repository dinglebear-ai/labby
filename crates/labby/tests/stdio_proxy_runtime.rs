// These integration tests drive the labby stdio proxy (a trusted server)
// with the raw rmcp helpers on purpose.
#![allow(clippy::disallowed_methods)]
#![cfg(all(feature = "gateway", feature = "proxy-testkit"))]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use labby::proxy::command::ProxyCommand;
use labby::proxy::config::{ProxyAuthMode, ProxyExposure, ProxyPortPreference, ProxyPreferences};
use labby::proxy::runtime::{LocalProxy, LocalProxyAuthPolicy, LocalProxyOptions};
use labby_auth::config::{AuthConfig, AuthMode, GoogleConfig};
use labby_auth::jwt::AccessClaims;
use labby_auth::state::AuthState;
use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};

#[cfg(unix)]
const PROCESS_DEADLOCK_WATCHDOG: Duration = Duration::from_mins(1);

#[cfg(unix)]
async fn wait_for_readiness_or_exit(
    child: &mut tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    context: &str,
) -> Result<String, String> {
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    tokio::time::timeout(PROCESS_DEADLOCK_WATCHDOG, async {
        let mut lines = BufReader::new(stdout).lines();
        tokio::select! {
            line = lines.next_line() => line
                .map_err(|error| format!("failed to read {context}: {error}"))?
                .ok_or_else(|| format!("child closed stdout before {context}")),
            status = child.wait() => Err(format!(
                "child exited before {context}: {}",
                status.map_err(|error| format!("failed to read child exit status: {error}"))?
            )),
        }
    })
    .await
    .map_err(|_| format!("deadlock waiting for {context}"))?
}

#[cfg(unix)]
async fn wait_for_child_output(
    child: tokio::process::Child,
    context: &str,
) -> Result<std::process::Output, String> {
    tokio::time::timeout(PROCESS_DEADLOCK_WATCHDOG, child.wait_with_output())
        .await
        .map_err(|_| format!("deadlock waiting for {context}"))?
        .map_err(|error| format!("failed waiting for {context}: {error}"))
}

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
    connect_with_lifecycle(
        url,
        token,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
        },
    )
    .await
}

async fn connect_with_lifecycle(
    url: &url::Url,
    token: Option<&str>,
    lifecycle: ClientLifecycleMode,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    ensure_tls_provider();
    let mut config = StreamableHttpClientTransportConfig::with_uri(url.as_str().to_string());
    config.auth_header = token.map(str::to_string);
    let worker = StreamableHttpClientWorker::new(reqwest::Client::new(), config);
    ().serve_with_lifecycle(worker, lifecycle)
        .await
        .expect("HTTP MCP client connects")
}

#[tokio::test]
async fn local_proxy_forwards_tools_list_after_child_discovery_and_bind() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("child.pid");
    let command = fixture_command(temp.path().to_path_buf(), &pid_file);
    #[cfg(unix)]
    let mut command = command;
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
    let resources = service.peer().list_all_resources().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, "fixture://status");
    let resource = service
        .peer()
        .read_resource(rmcp::model::ReadResourceRequestParams::new(
            "fixture://status",
        ))
        .await
        .unwrap();
    assert!(
        matches!(
            &resource.contents[0],
            rmcp::model::ResourceContents::TextResourceContents { .. }
        ),
        "expected text resource contents, got {:?}",
        resource.contents[0]
    );
    let text = match &resource.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text,
        _ => return,
    };
    assert_eq!(text, "fixture-ready");
    let prompts = service.peer().list_all_prompts().await.unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "fixture.prompt");
    let prompt = service
        .peer()
        .get_prompt(rmcp::model::GetPromptRequestParams::new("fixture.prompt"))
        .await
        .unwrap();
    assert_eq!(
        prompt.messages[0].content.as_text().unwrap().text,
        "fixture prompt result"
    );

    for lifecycle in [
        ClientLifecycleMode::Initialize,
        ClientLifecycleMode::Auto {
            preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
            legacy_version: Some(rmcp::model::ProtocolVersion::V_2025_11_25),
        },
    ] {
        let lifecycle_service = connect_with_lifecycle(proxy.url(), None, lifecycle).await;
        assert_eq!(
            lifecycle_service
                .peer()
                .list_all_tools()
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            lifecycle_service
                .peer()
                .list_all_resources()
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            lifecycle_service
                .peer()
                .list_all_prompts()
                .await
                .unwrap()
                .len(),
            1
        );
        lifecycle_service.cancel().await.unwrap();
    }

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
    let child_cwd = PathBuf::from(context["cwd"].as_str().expect("fixture cwd"))
        .canonicalize()
        .expect("fixture cwd exists");
    let expected_cwd = temp.path().canonicalize().expect("temp cwd exists");
    assert_eq!(child_cwd, expected_cwd);
    assert_eq!(context["explicit_env"], "present");
    assert!(
        context["inherited_path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
    );
    assert_eq!(context["scrub_canary"], serde_json::Value::Null);
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
async fn prepared_listener_accepts_no_http_requests_before_router_start() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("prepared-child.pid");
    let prepared = LocalProxy::prepare(LocalProxyOptions {
        command: fixture_command(temp.path().to_path_buf(), &pid_file),
        preferences: local_preferences(ProxyAuthMode::None),
        bearer_token: None,
        explicit_env: Vec::new(),
        inherit_env: vec![OsString::from("PATH")],
    })
    .await
    .expect("child discovery and loopback bind complete");

    let mut stream = tokio::net::TcpStream::connect(prepared.local_addr())
        .await
        .expect("kernel listener is bound");
    stream
        .write_all(b"GET /custom-mcp HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut byte = [0_u8; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), stream.read(&mut byte))
            .await
            .is_err(),
        "prepared listener must not accept or answer HTTP before auth is finalized"
    );
    drop(stream);

    let proxy = prepared
        .start(LocalProxyAuthPolicy::None)
        .expect("router starts from the already-bound listener");
    let service = connect(proxy.url(), None).await;
    assert_eq!(service.peer().list_all_tools().await.unwrap().len(), 1);
    proxy.shutdown().await.unwrap();
}

async fn oauth_state(temp: &tempfile::TempDir) -> Arc<AuthState> {
    let config = AuthConfig {
        mode: AuthMode::OAuth,
        public_url: Some(url::Url::parse("https://issuer.example.com").unwrap()),
        sqlite_path: temp.path().join("auth.db"),
        key_path: temp.path().join("auth-jwt.pem"),
        scopes_supported: vec!["mcp:read".to_string(), "mcp:write".to_string()],
        disable_static_token_with_oauth: true,
        google: GoogleConfig {
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            ..GoogleConfig::default()
        },
        token_encryption_key: Some(
            labby_auth::at_rest::TokenEncryptionKey::from_encoded(
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            )
            .unwrap(),
        ),
        ..AuthConfig::default()
    };
    Arc::new(AuthState::new(config).await.unwrap())
}

fn oauth_claims(resource: &str, issuer: &str, scope: &str) -> AccessClaims {
    let now = usize::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    AccessClaims {
        iss: issuer.to_string(),
        sub: "subject-must-not-be-logged".to_string(),
        aud: resource.to_string(),
        exp: now + 300,
        nbf: None,
        iat: now,
        jti: "jwt-id-must-not-be-logged".to_string(),
        scope: scope.to_string(),
        azp: "proxy-test".to_string(),
        identity_issuer: Some("https://accounts.google.com".to_string()),
        identity_credential_id: None,
    }
}

async fn oauth_request(
    proxy: &LocalProxy,
    resource: &url::Url,
    token: Option<&str>,
) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(proxy.url().clone())
        .header(reqwest::header::HOST, resource.authority())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 91,
            "method": "tools/list",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": {"name":"oauth-test","version":"1"},
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        }));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.unwrap()
}

#[tokio::test]
async fn oauth_proxy_serves_exact_root_metadata_and_enforces_token_contract() {
    let temp = tempfile::tempdir().unwrap();
    let state = oauth_state(&temp).await;
    let resource = url::Url::parse("https://node.example.ts.net:53147/custom-mcp").unwrap();
    let issuer = url::Url::parse("https://issuer.example.com").unwrap();
    let stable_issuer = issuer.as_str().trim_end_matches('/');
    let prepared = LocalProxy::prepare(LocalProxyOptions {
        command: fixture_command(
            temp.path().to_path_buf(),
            &temp.path().join("oauth-child.pid"),
        ),
        preferences: local_preferences(ProxyAuthMode::Oauth),
        bearer_token: None,
        explicit_env: Vec::new(),
        inherit_env: vec![OsString::from("PATH")],
    })
    .await
    .unwrap();
    let proxy = prepared
        .start(LocalProxyAuthPolicy::Oauth {
            auth_state: Arc::clone(&state),
            resource: resource.clone(),
            issuer: issuer.clone(),
            required_scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
        })
        .unwrap();

    let metadata_url = proxy
        .url()
        .join("/.well-known/oauth-protected-resource")
        .unwrap();
    let metadata: serde_json::Value = reqwest::Client::new()
        .get(metadata_url)
        .header(reqwest::header::HOST, resource.authority())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        metadata,
        serde_json::json!({
            "resource": "https://node.example.ts.net:53147/custom-mcp",
            "authorization_servers": ["https://issuer.example.com"],
            "scopes_supported": ["mcp:read", "mcp:write"],
            "bearer_methods_supported": ["header"]
        })
    );

    let missing = oauth_request(&proxy, &resource, None).await;
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing.headers()[reqwest::header::WWW_AUTHENTICATE],
        "Bearer resource_metadata=\"https://node.example.ts.net:53147/.well-known/oauth-protected-resource\", scope=\"mcp:read mcp:write\""
    );

    let accepted = state
        .signing_keys
        .issue_access_token(&oauth_claims(
            resource.as_str(),
            stable_issuer,
            "mcp:read mcp:write",
        ))
        .unwrap();
    assert_eq!(
        oauth_request(&proxy, &resource, Some(&accepted))
            .await
            .status(),
        reqwest::StatusCode::OK
    );

    let now = usize::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    let mut cases = vec![
        (
            "wrong port",
            oauth_claims(
                "https://node.example.ts.net:53148/custom-mcp",
                stable_issuer,
                "mcp:read mcp:write",
            ),
            reqwest::StatusCode::UNAUTHORIZED,
        ),
        (
            "wrong path",
            oauth_claims(
                "https://node.example.ts.net:53147/other",
                stable_issuer,
                "mcp:read mcp:write",
            ),
            reqwest::StatusCode::UNAUTHORIZED,
        ),
        (
            "wrong issuer",
            oauth_claims(
                resource.as_str(),
                "https://other-issuer.example.com",
                "mcp:read mcp:write",
            ),
            reqwest::StatusCode::UNAUTHORIZED,
        ),
        (
            "wrong scope",
            oauth_claims(resource.as_str(), stable_issuer, "mcp:read"),
            reqwest::StatusCode::FORBIDDEN,
        ),
        (
            "expired",
            {
                let mut c = oauth_claims(resource.as_str(), stable_issuer, "mcp:read mcp:write");
                c.exp = now - 300;
                c
            },
            reqwest::StatusCode::UNAUTHORIZED,
        ),
        (
            "not before",
            {
                let mut c = oauth_claims(resource.as_str(), stable_issuer, "mcp:read mcp:write");
                c.nbf = Some(now + 300);
                c
            },
            reqwest::StatusCode::UNAUTHORIZED,
        ),
    ];
    for (name, claims, expected) in cases.drain(..) {
        let token = state.signing_keys.issue_access_token(&claims).unwrap();
        assert_eq!(
            oauth_request(&proxy, &resource, Some(&token))
                .await
                .status(),
            expected,
            "{name}"
        );
    }

    proxy.shutdown().await.unwrap();
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
async fn cli_local_oauth_fails_clearly_when_loopback_leases_are_not_enabled() {
    use tokio::process::Command;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    #[derive(Clone)]
    struct LeaseResponder;
    impl Respond for LeaseResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            match body["action"].as_str().unwrap() {
                "gateway.oauth.resource_lease.create" => {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "id": "lease-id-must-not-appear",
                        "resource": body["params"]["resource"],
                        "scopes": body["params"]["scopes"],
                        "expires_at_unix": 4_000_000_000_u64
                    }))
                }
                "gateway.oauth.resource_lease.release" => {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"released": true}))
                }
                other => ResponseTemplate::new(500).set_body_string(other.to_string()),
            }
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let key_path = temp.path().join("auth-jwt.pem");
    let keys = labby_auth::jwt::SigningKeys::load_or_create(&key_path).unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/labby.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "paletteCatalogUrl": format!("{}/catalog", server.uri()),
            "paletteExecuteUrl": format!("{}/execute", server.uri())
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/gateway/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"name":"gateway.oauth.resource_lease.create"},
            {"name":"gateway.oauth.resource_lease.renew"},
            {"name":"gateway.oauth.resource_lease.release"}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "issuer": server.uri(),
            "jwks_uri": format!("{}/jwks", server.uri())
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(keys.jwks()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway"))
        .respond_with(LeaseResponder)
        .mount(&server)
        .await;

    let pid_file = temp.path().join("oauth-cli-child.pid");
    let child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["--json", "proxy", "--local", "--auth", "oauth"])
        .arg(env!("CARGO_BIN_EXE_stdio-mcp-fixture"))
        .arg("--pid-file")
        .arg(&pid_file)
        .env("LABBY_HOME", temp.path())
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_SERVER_URL", server.uri())
        .env("LABBY_MCP_HTTP_TOKEN", "")
        .env("LABBY_PUBLIC_URL", server.uri())
        .env("LABBY_AUTH_MODE", "oauth")
        .env("LABBY_AUTH_SQLITE_PATH", temp.path().join("auth.db"))
        .env("LABBY_AUTH_KEY_PATH", &key_path)
        .env("LABBY_AUTH_ADMIN_EMAIL", "admin@example.com")
        .env("LABBY_GOOGLE_CLIENT_ID", "test-client")
        .env("LABBY_GOOGLE_CLIENT_SECRET", "test-secret")
        .env(
            "LABBY_TOKEN_ENCRYPTION_KEY",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let output = wait_for_child_output(child, "local OAuth rejection")
        .await
        .expect("local OAuth rejection completes");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("local OAuth exposure is not enabled"),
        "unexpected stderr: {stderr}"
    );
    assert!(!stderr.contains("lease-id-must-not-appear"));
    assert!(!stderr.contains("subject-must-not-be-logged"));

    let requests = server.received_requests().await.unwrap();
    assert!(!requests.iter().any(|request| {
        serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .is_some_and(|body| body["action"] == "gateway.oauth.resource_lease.create")
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn cli_tailscale_oauth_collision_releases_old_lease_then_renewal_failure_cleans_all() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::process::Command;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    #[derive(Clone)]
    struct LeaseResponder;
    impl Respond for LeaseResponder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            match body["action"].as_str().unwrap() {
                "gateway.oauth.resource_lease.create" => {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "id": format!("lease-for-{}", body["params"]["resource"].as_str().unwrap()),
                        "resource": body["params"]["resource"],
                        "scopes": body["params"]["scopes"],
                        "expires_at_unix": 4_000_000_000_u64
                    }))
                }
                "gateway.oauth.resource_lease.release" => {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"released": true}))
                }
                other => ResponseTemplate::new(500).set_body_string(other.to_string()),
            }
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let tailscale = root.join("tailscale");
    let tailscale_events = root.join("tailscale-events");
    std::fs::write(
        &tailscale,
        format!(
            r#"#!/usr/bin/env bash
set -u
root='{root}'
mapping="$root/mapping"
events="$root/tailscale-events"
if [[ "${{1:-}}" == "version" ]]; then echo 1.98.10; exit 0; fi
if [[ "${{1:-}} ${{2:-}}" == "status --json" ]]; then
  echo '{{"BackendState":"Running","Self":{{"Online":true,"DNSName":"node.example.ts.net."}}}}'; exit 0
fi
if [[ "${{1:-}} ${{2:-}} ${{3:-}}" == "serve status --json" ]]; then
  if [[ -f "$mapping" ]]; then IFS='|' read -r port backend < "$mapping"; printf '{{"Web":{{"node.example.ts.net:%s":{{"Handlers":{{"/":{{"Proxy":"%s"}}}}}}}}}}\n' "$port" "$backend"; else echo '{{}}'; fi
  exit 0
fi
if [[ "${{1:-}}" == "serve" ]]; then
  port="${{3#--https=}}"
  if [[ "${{4:-}}" == "off" ]]; then rm -f "$mapping"; exit 0; fi
  count=0; [[ -f "$root/claims" ]] && count=$(<"$root/claims"); count=$((count+1)); echo "$count" > "$root/claims"
  if [[ "$count" == 1 ]]; then printf 'collision:%s\n' "$port" >> "$events"; echo 'port already configured' >&2; exit 1; fi
  printf '%s|%s\n' "$port" "${{4:-}}" > "$mapping"
  printf 'claimed:%s\n' "$port" >> "$events"
  trap 'printf "released:%s\n" "$port" >> "$events"; rm -f "$mapping"; exit 0' TERM INT
  while :; do sleep 0.02; done
fi
exit 2
"#,
            root = root.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tailscale, std::fs::Permissions::from_mode(0o755)).unwrap();

    let key_path = root.join("auth-jwt.pem");
    let keys = labby_auth::jwt::SigningKeys::load_or_create(&key_path).unwrap();
    let server = MockServer::start().await;
    for endpoint in ["/health", "/.well-known/labby.json"] {
        let body = if endpoint == "/health" {
            serde_json::json!({"status":"ok"})
        } else {
            serde_json::json!({"paletteCatalogUrl":"catalog","paletteExecuteUrl":"execute"})
        };
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET")).and(path("/v1/gateway/actions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
        {"name":"gateway.oauth.resource_lease.create"},{"name":"gateway.oauth.resource_lease.renew"},{"name":"gateway.oauth.resource_lease.release"}
    ]))).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"issuer":server.uri(),"jwks_uri":format!("{}/jwks",server.uri())}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(keys.jwks()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/gateway"))
        .respond_with(LeaseResponder)
        .mount(&server)
        .await;

    let child_pid_file = root.join("renewal-child.pid");
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["--json", "proxy", "--auth", "oauth"])
        .arg(env!("CARGO_BIN_EXE_stdio-mcp-fixture"))
        .arg("--pid-file")
        .arg(&child_pid_file)
        .env("LABBY_HOME", root)
        .env("CLAUDE_PLUGIN_OPTION_SERVER_URL", "")
        .env("LABBY_SERVER_URL", server.uri())
        .env("LABBY_MCP_HTTP_TOKEN", "")
        .env("LABBY_PUBLIC_URL", server.uri())
        .env("LABBY_AUTH_MODE", "oauth")
        .env("LABBY_AUTH_SQLITE_PATH", root.join("auth.db"))
        .env("LABBY_AUTH_KEY_PATH", &key_path)
        .env("LABBY_AUTH_ADMIN_EMAIL", "admin@example.com")
        .env("LABBY_GOOGLE_CLIENT_ID", "test-client")
        .env("LABBY_GOOGLE_CLIENT_SECRET", "test-secret")
        .env(
            "LABBY_TOKEN_ENCRYPTION_KEY",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .env("LABBY_TAILSCALE_BIN", &tailscale)
        .env("LABBY_PROXY_TEST_RENEW_MS", "100")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let line = wait_for_readiness_or_exit(&mut child, stdout, "OAuth proxy readiness")
        .await
        .expect("OAuth proxy reaches readiness");
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["auth"], "oauth");
    assert_eq!(ready["exposure"], "tailscale");
    let output = wait_for_child_output(child, "OAuth lease-renewal failure")
        .await
        .expect("OAuth lease-renewal failure completes");
    assert!(!output.status.success());
    let stderr: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(stderr["ok"], false);
    assert_eq!(stderr["command"], "proxy");
    assert_eq!(stderr["error"]["contract_version"], 1);
    assert_eq!(stderr["error"]["kind"], "internal_error");
    assert_eq!(stderr["error"]["origin"], "runtime");
    assert!(
        stderr["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("OAuth resource lease renewal failed"))
    );
    assert_eq!(
        stderr["error"]["cause"],
        "live gateway daemon returned HTTP 500 Internal Server Error"
    );

    let requests = server.received_requests().await.unwrap();
    let actions = requests
        .iter()
        .filter_map(|request| serde_json::from_slice::<serde_json::Value>(&request.body).ok())
        .filter(|body| body.get("action").is_some())
        .collect::<Vec<_>>();
    let created = actions
        .iter()
        .filter(|body| body["action"] == "gateway.oauth.resource_lease.create")
        .map(|body| body["params"]["resource"].as_str().unwrap())
        .collect::<Vec<_>>();
    let released = actions
        .iter()
        .filter(|body| body["action"] == "gateway.oauth.resource_lease.release")
        .count();
    assert_eq!(created.len(), 2);
    assert_ne!(created[0], created[1]);
    assert_eq!(released, 2);
    assert!(!root.join("mapping").exists());
    let tailscale_events = std::fs::read_to_string(tailscale_events).unwrap();
    assert!(tailscale_events.contains("collision:"));
    assert!(tailscale_events.contains("claimed:"));
    assert!(tailscale_events.contains("released:"));
    let child_pid: i32 = std::fs::read_to_string(child_pid_file)
        .unwrap()
        .parse()
        .unwrap();
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(child_pid), None).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn zero_flag_cli_publishes_verified_tailscale_url_with_fake_cli() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::process::Command;

    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("tailscale-child.pid");
    let mapping = temp.path().join("mapping");
    let calls = temp.path().join("tailscale-calls");
    let events = temp.path().join("tailscale-events");
    let fake_tailscale = temp.path().join("tailscale");
    let script = format!(
        r#"#!/usr/bin/env bash
set -u
mapping='{mapping}'
calls='{calls}'
events='{events}'
printf '%s\n' "$*" >> "$calls"
if [[ "${{1:-}} ${{2:-}}" == "status --json" ]]; then
  printf '%s\n' '{{"BackendState":"Running","Self":{{"Online":true,"DNSName":"proxy-test.example.ts.net."}}}}'
  exit 0
fi
if [[ "${{1:-}}" == "version" ]]; then printf '%s\n' '1.98.10'; exit 0; fi
if [[ "${{1:-}} ${{2:-}} ${{3:-}}" == "serve status --json" ]]; then
  if [[ -f "$mapping" ]]; then
    IFS='|' read -r port backend < "$mapping"
    printf 'verified:%s\n' "$port" >> "$events"
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
  printf 'claimed:%s\n' "$port" >> "$events"
  trap 'printf "released:%s\n" "$port" >> "$events"; rm -f "$mapping"; exit 0' TERM INT
  while :; do sleep 0.05; done
fi
exit 2
"#,
        mapping = mapping.display(),
        calls = calls.display(),
        events = events.display(),
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
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let line = wait_for_readiness_or_exit(&mut child, stdout, "Tailscale proxy readiness")
        .await
        .expect("Tailscale proxy reaches readiness");
    let ready: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["url"], "https://proxy-test.example.ts.net:54000/mcp");
    assert_eq!(ready["exposure"], "tailscale");
    assert_eq!(ready["auth"], "tailnet");
    assert_eq!(ready["external_port"], 54_000);
    let readiness_events = std::fs::read_to_string(&events).unwrap();
    assert!(readiness_events.contains("claimed:54000"));
    assert!(readiness_events.contains("verified:54000"));
    let local_url = url::Url::parse(&format!(
        "http://{}/mcp",
        ready["local_addr"].as_str().unwrap()
    ))
    .unwrap();
    let public_url = url::Url::parse(ready["url"].as_str().unwrap()).unwrap();
    let public_authority = format!(
        "{}:{}",
        public_url.host_str().unwrap(),
        public_url.port().unwrap()
    );
    let public_origin = public_url.origin().ascii_serialization();
    ensure_tls_provider();
    let response = reqwest::Client::new()
        .post(local_url.clone())
        .header(reqwest::header::HOST, public_authority)
        .header(reqwest::header::ORIGIN, public_origin)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 91,
            "method": "tools/list",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "public-authority-test",
                        "version": "1"
                    },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.text().await.unwrap().contains("fixture.echo"));

    let service = connect(&local_url, None).await;
    assert_eq!(service.peer().list_all_tools().await.unwrap().len(), 1);

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()),
        nix::sys::signal::Signal::SIGINT,
    )
    .unwrap();
    let status = tokio::time::timeout(PROCESS_DEADLOCK_WATCHDOG, child.wait())
        .await
        .expect("deadlock waiting for CLI shutdown")
        .unwrap();
    assert!(status.success(), "CLI exited with {status}");
    assert!(!mapping.exists());
    let calls = std::fs::read_to_string(calls).unwrap();
    assert!(!calls.contains("reset"));
    assert!(
        std::fs::read_to_string(events)
            .unwrap()
            .contains("released:54000")
    );
}
