//! Real HTTPS/MCP requests backed by the same encrypted session as CLI login/logout.

// The Linux platform verifier honors SSL_CERT_FILE; other platforms require
// OS trust-store provisioning rather than this isolated environment fixture.
#![cfg(target_os = "linux")]

use super::*;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use http_body_util::{BodyExt as _, Full};
use hyper::{Request, Response, StatusCode, body::Bytes};
use oauth2::TokenResponse as _;
use rmcp::transport::auth::CredentialStore as _;
use serde_json::json;

const CHILD_ENV: &str = "LABBY_BRIDGE_OAUTH_FIXTURE";
const TEST_NAME: &str = "live_gateway::oauth_session_tests::saved_cli_session_refreshes_real_mcp_and_logout_fails_closed";

#[derive(Default)]
struct Observed {
    protected_calls: Vec<String>,
    refreshes: Vec<String>,
}

#[tokio::test]
async fn saved_cli_session_refreshes_real_mcp_and_logout_fails_closed() {
    run_fixture(TEST_NAME, false).await;
}

#[tokio::test]
async fn saved_oauth_setup_bridge_registers_refreshes_and_logout_fails_closed() {
    run_fixture("live_gateway::oauth_session_tests::saved_oauth_setup_bridge_registers_refreshes_and_logout_fails_closed", true).await;
}

async fn run_fixture(test_name: &str, setup: bool) {
    if std::env::var_os(CHILD_ENV).is_some() {
        tokio::time::timeout(Duration::from_secs(30), exercise_session(setup))
            .await
            .expect("HTTPS/MCP session fixture timed out");
        return;
    }

    // Native TLS roots and installation selection are process-wide. Set them on
    // an isolated child before any HTTP client exists, preserving other tests
    // and the operator's real CLI session. Production HTTPS checks stay enabled.
    let dir = tempfile::tempdir().unwrap();
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let pem = dir.path().join("fixture-ca.pem");
    let encoded = base64::engine::general_purpose::STANDARD.encode(cert.der());
    let mut certificate_pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in encoded.as_bytes().chunks(64) {
        certificate_pem.push_str(std::str::from_utf8(line).unwrap());
        certificate_pem.push('\n');
    }
    certificate_pem.push_str("-----END CERTIFICATE-----\n");
    std::fs::write(&pem, certificate_pem).unwrap();
    std::fs::write(dir.path().join("certificate.der"), cert.der()).unwrap();
    std::fs::write(dir.path().join("key.der"), signing_key.serialize_der()).unwrap();
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .kill_on_drop(true)
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_ENV, dir.path())
        .env("LABBY_HOME", dir.path().join("installation"))
        .env("SSL_CERT_FILE", pem)
        .env_remove("SSL_CERT_DIR")
        .env_remove("CLAUDE_PLUGIN_OPTION_API_TOKEN")
        .env_remove("LABBY_MCP_HTTP_TOKEN")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "isolated HTTPS bridge regression failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed;"),
        "isolated child must execute exactly the selected regression"
    );
}

async fn exercise_session(setup: bool) {
    drop(rustls::crypto::ring::default_provider().install_default());
    let dir = std::path::PathBuf::from(std::env::var_os(CHILD_ENV).unwrap());
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                std::fs::read(dir.join("certificate.der")).unwrap(),
            )],
            rustls::pki_types::PrivatePkcs8KeyDer::from(
                std::fs::read(dir.join("key.der")).unwrap(),
            )
            .into(),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}/", listener.local_addr().unwrap());
    let server_url = crate::oauth::cli_session::server_url(&origin).unwrap();
    let observed = Arc::new(Mutex::new(Observed::default()));
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let fixture_observed = observed.clone();
    let fixture_origin = origin.clone();
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let observed = fixture_observed.clone();
            let origin = fixture_origin.clone();
            tokio::spawn(async move {
                let stream = acceptor.accept(socket).await.unwrap();
                let service = hyper::service::service_fn(move |request| {
                    respond(request, origin.clone(), observed.clone())
                });
                drop(
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await,
                );
            });
        }
    });

    let root = crate::installation::InstallationPaths::resolve().unwrap();
    let credentials =
        crate::oauth::cli_session::test_support::seed_bridge_test_profile(root.root(), &server_url)
            .await
            .unwrap();
    assert_eq!(
        crate::oauth::cli_session::token_for_root(root.root(), &server_url)
            .await
            .unwrap()
            .as_deref(),
        Some("bridge-initial-token")
    );
    // An explicit root with no grant must not borrow the valid ambient profile.
    let unrelated = tempfile::tempdir().unwrap();
    assert!(
        detect_bound_saved_cli_target(unrelated.path(), &origin)
            .await
            .is_err()
    );
    labby_runtime::secure_atomic_file::write_secure_atomic(
        &root.root().join(".env"),
        format!("LABBY_SERVER_URL={origin}\nLABBY_MCP_HTTP_TOKEN=\n").as_bytes(),
    )
    .unwrap();
    let mut bridge = None;
    let service = if setup {
        // Registration must write a token-free stdio descriptor for this same
        // encrypted OAuth profile, retaining unrelated client settings.
        use crate::dispatch::setup::client_registration::{self, ExternalClient};
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        let config_path = home.path().join(".codex/config.toml");
        std::fs::write(&config_path, "# retained\nmodel = 'fixture'\n").unwrap();
        let binary = std::env::current_exe().unwrap();
        let plan = client_registration::saved_plan(
            home.path().to_owned(),
            root.root().to_owned(),
            ExternalClient::Codex,
            origin.clone(),
            binary.clone(),
        )
        .await
        .unwrap();
        assert_eq!(plan.authentication, "protected_saved_cli_bridge");
        let result = client_registration::saved_register(
            home.path().to_owned(),
            root.root().to_owned(),
            ExternalClient::Codex,
            origin.clone(),
            binary,
            plan.config_version,
        )
        .await
        .unwrap();
        assert!(result.changed && result.bridge_verified && !result.connected);
        let registered: toml::Value =
            toml::from_str(&std::fs::read_to_string(config_path).unwrap()).unwrap();
        assert_eq!(registered["model"].as_str(), Some("fixture"));
        let entry = &registered["mcp_servers"]["lab"];
        assert!(entry.get("url").is_none());
        assert!(entry.get("command").is_some());
        assert!(!entry.to_string().contains("bridge-initial-token"));
        let args = entry["args"].as_array().unwrap();
        assert!(
            args.windows(2)
                .any(|pair| pair[0].as_str() == Some("--state-root")
                    && pair[1].as_str() == root.root().to_str())
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0].as_str() == Some("--client")
                    && pair[1].as_str() == Some("codex"))
        );
        observed.lock().unwrap().protected_calls.clear();
        use rmcp::ServiceExt as _;
        let (client_io, bridge_io) = tokio::io::duplex(256 * 1024);
        let (read, write) = tokio::io::split(bridge_io);
        bridge = Some(tokio::spawn(crate::dispatch::setup::client_bridge::run_io(
            root.root().to_path_buf(),
            origin.clone(),
            read,
            write,
        )));
        ().serve(client_io).await.unwrap()
    } else {
        let gateway = LiveGateway {
            base_url: server_url.clone(),
            explicit: true,
            source: "operator",
            token: Some("bridge-initial-token".into()),
            saved_cli_session: Some(root.root().to_path_buf()),
            team_id: None,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            dispatch_timeout: DEFAULT_DISPATCH_TIMEOUT,
            actions: None,
        };
        gateway.connect_service(()).await.unwrap()
    };
    service.peer().list_tools(None).await.unwrap();
    assert_eq!(
        observed.lock().unwrap().protected_calls,
        ["bridge-initial-token"]
    );

    // Simulate time passing in the shared durable profile, without sleeping or
    // modifying the bridge's private auth manager. Both refresh rounds must be
    // discovered by the already running MCP transport.
    for (access, refresh) in [
        ("bridge-refreshed-1", "bridge-refresh-2"),
        ("bridge-refreshed-2", "bridge-refresh-3"),
    ] {
        let mut saved = credentials.load().await.unwrap().unwrap();
        saved.token_received_at = Some(1);
        credentials.save(saved).await.unwrap();
        service.peer().list_tools(None).await.unwrap();
        let saved = credentials.load().await.unwrap().unwrap();
        let token = saved.token_response.unwrap();
        assert_eq!(token.access_token().secret(), access);
        assert_eq!(token.refresh_token().unwrap().secret(), refresh);
    }
    assert_eq!(
        observed.lock().unwrap().refreshes,
        ["bridge-refresh-1", "bridge-refresh-2"]
    );
    assert_eq!(
        observed.lock().unwrap().protected_calls,
        [
            "bridge-initial-token",
            "bridge-refreshed-1",
            "bridge-refreshed-2"
        ]
    );

    assert!(
        crate::oauth::cli_session::logout(&server_url)
            .await
            .unwrap()
    );
    assert!(credentials.load().await.unwrap().is_none());
    assert!(service.peer().list_tools(None).await.is_err());
    assert_eq!(observed.lock().unwrap().protected_calls.len(), 3);
    assert_eq!(observed.lock().unwrap().refreshes.len(), 2);
    drop(service.cancel().await);
    if let Some(bridge) = bridge {
        tokio::time::timeout(Duration::from_secs(3), bridge)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    server.abort();
}

async fn respond(
    request: Request<hyper::body::Incoming>,
    origin: String,
    observed: Arc<Mutex<Observed>>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let bearer = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let (status, body) = match path.as_str() {
        "/.well-known/oauth-protected-resource/mcp" | "/.well-known/oauth-protected-resource" => (
            StatusCode::OK,
            json!({"resource":format!("{origin}mcp"),"authorization_servers":[origin]}),
        ),
        "/.well-known/oauth-authorization-server" => (
            StatusCode::OK,
            json!({"issuer":origin.trim_end_matches('/'),"authorization_endpoint":format!("{origin}authorize"),"token_endpoint":format!("{origin}token"),"code_challenge_methods_supported":["S256"]}),
        ),
        "/health" => (StatusCode::OK, json!({"status":"ok"})),
        "/v1/gateway/actions" if bearer.as_deref() == Some("bridge-initial-token") => {
            (StatusCode::OK, json!([{"name":"gateway.reload"}]))
        }
        "/token" => {
            let form: std::collections::HashMap<_, _> =
                url::form_urlencoded::parse(&body).into_owned().collect();
            assert_eq!(
                form.get("grant_type").map(String::as_str),
                Some("refresh_token")
            );
            let refresh = form.get("refresh_token").unwrap().clone();
            let round = ["bridge-refresh-1", "bridge-refresh-2"]
                .iter()
                .position(|expected| *expected == refresh)
                .expect("fixture must receive a supported refresh generation")
                + 1;
            observed.lock().unwrap().refreshes.push(refresh);
            (
                StatusCode::OK,
                json!({"access_token":format!("bridge-refreshed-{round}"),"refresh_token":format!("bridge-refresh-{}",round+1),"token_type":"Bearer","expires_in":3600}),
            )
        }
        "/mcp" if method == hyper::Method::POST => {
            let request: Value = serde_json::from_slice(&body).unwrap();
            let expected = match observed.lock().unwrap().refreshes.len() {
                0 => "bridge-initial-token",
                1 => "bridge-refreshed-1",
                _ => "bridge-refreshed-2",
            };
            if bearer.as_deref() != Some(expected) {
                (StatusCode::UNAUTHORIZED, json!({"error":"invalid_token"}))
            } else {
                let method = request["method"].as_str().unwrap();
                assert!(
                    matches!(
                        method,
                        "server/discover"
                            | "initialize"
                            | "notifications/initialized"
                            | "tools/list"
                    ),
                    "unexpected fixture MCP method {method}"
                );
                match method {
                    "server/discover" => {
                        let result = rmcp::model::DiscoverResult::new(
                            vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                            rmcp::model::ServerCapabilities::builder()
                                .enable_tools()
                                .build(),
                        );
                        (
                            StatusCode::OK,
                            json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                        )
                    }
                    "initialize" => (
                        StatusCode::OK,
                        json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"saved-cli-session-fixture","version":"1"}}}),
                    ),
                    "notifications/initialized" => (StatusCode::ACCEPTED, Value::Null),
                    "tools/list" => {
                        observed
                            .lock()
                            .unwrap()
                            .protected_calls
                            .push(bearer.unwrap());
                        (
                            StatusCode::OK,
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"resultType":"complete","ttlMs":0,"cacheScope":"private","tools":[]}}),
                        )
                    }
                    _ => (StatusCode::BAD_REQUEST, Value::Null),
                }
            }
        }
        _ => (StatusCode::METHOD_NOT_ALLOWED, Value::Null),
    };
    Ok(Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(serde_json::to_vec(&body).unwrap())))
        .unwrap())
}
