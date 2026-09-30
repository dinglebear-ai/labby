//! Required bearer credentials must fail before any upstream side effect.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use labby_runtime::gateway_config::{UpstreamConfig, UpstreamLifecycle};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::connect::connect_upstream;
use super::http_cancellation::build_http_cancellation_sender;
use super::testsupport::test_upstream_config;

fn assert_missing_bearer(error: anyhow::Error) {
    let typed = error
        .downcast_ref::<labby_runtime::error::ToolError>()
        .expect("connection failures must preserve the typed credential error");
    assert_eq!(typed.kind(), "upstream_credential_missing");
}

fn missing_bearer_config() -> UpstreamConfig {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "LABBY_TEST_ABSENT_BEARER_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    assert!(std::env::var_os(&name).is_none());
    UpstreamConfig {
        bearer_token_env: Some(name),
        ..test_upstream_config()
    }
}

#[tokio::test]
async fn missing_bearer_blocks_http_before_any_request() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    for lifecycle in [None, Some(UpstreamLifecycle::Initialize)] {
        let mut config = missing_bearer_config();
        config.url = Some(format!("{}/mcp", server.uri()));
        config.lifecycle = lifecycle;
        let result = connect_upstream(&config, None, None, None, None).await;
        assert_missing_bearer(result.expect_err("missing credentials must fail closed"));
        assert!(
            server
                .received_requests()
                .await
                .expect("request journal")
                .is_empty(),
            "neither discovery nor a lifecycle retry may be sent anonymously"
        );
    }
}

#[tokio::test]
async fn missing_bearer_blocks_websocket_before_connect() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let mut config = missing_bearer_config();
    config.url = Some(format!("ws://{}/mcp", listener.local_addr().unwrap()));
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = connect_upstream(&config, None, None, None, None) => {
                assert_missing_bearer(result.expect_err("missing credentials must fail closed"));
            }
            _ = listener.accept() => panic!("missing credentials opened a WebSocket connection"),
        }
    })
    .await
    .expect("credential check must not wait for network I/O");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn missing_bearer_blocks_unix_socket_before_connect() {
    use labby_runtime::gateway_config::UpstreamTransport;

    let dir = tempfile::tempdir_in("/tmp").expect("short socket directory");
    let socket = dir.path().join("mcp.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("bind test socket");
    let mut config = missing_bearer_config();
    config.transport = Some(UpstreamTransport::UnixSocket);
    config.url = Some("http://test.local/mcp".into());
    config.socket_path = Some(socket.to_string_lossy().into_owned());
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = connect_upstream(&config, None, None, None, None) => {
                assert_missing_bearer(result.expect_err("missing credentials must fail closed"));
            }
            _ = listener.accept() => panic!("missing credentials opened a Unix socket connection"),
        }
    })
    .await
    .expect("credential check must not wait for socket I/O");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn missing_bearer_blocks_stdio_before_spawn() {
    let dir = tempfile::tempdir().expect("fixture directory");
    let marker = dir.path().join("spawned");
    let mut config = missing_bearer_config();
    config.command = Some("/bin/sh".into());
    config.args = vec![
        "-c".into(),
        "printf spawned > \"$1\"".into(),
        "fixture".into(),
        marker.to_string_lossy().into_owned(),
    ];
    let result = connect_upstream(&config, None, None, None, None).await;
    assert_missing_bearer(result.expect_err("missing credentials must fail closed"));
    assert!(
        !marker.exists(),
        "missing credentials must prevent child creation"
    );
}

#[tokio::test]
async fn missing_bearer_blocks_http_cancellation_sender() {
    let mut config = missing_bearer_config();
    config.url = Some("http://127.0.0.1:1/mcp".into());
    assert!(
        build_http_cancellation_sender(&config, None, None, None)
            .await
            .is_err(),
        "a cancellation sender must not lose its configured authentication"
    );
}

#[tokio::test]
async fn missing_bearer_preserves_kind_through_code_mode_lazy_connection() {
    let server = MockServer::start().await;
    let directory = tempfile::tempdir().expect("manager fixture");
    let manager = crate::gateway::manager::GatewayManager::new(
        directory.path().join("gateway.toml"),
        crate::gateway::manager::GatewayRuntimeHandle::default(),
    );
    let mut upstream = missing_bearer_config();
    upstream.url = Some(format!("{}/mcp", server.uri()));
    let name = upstream.name.clone();
    let mut config = manager.current_config().await;
    config.upstream = vec![upstream];
    manager.seed_config_unchecked_for_tests(config).await;
    let error = manager
        .ensure_upstream_tool_runtime_ready(&name, None, None)
        .await
        .expect_err("missing credential");
    assert_eq!(error.kind(), "upstream_credential_missing");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn anonymous_cancellation_is_allowed_only_without_a_credential_reference() {
    let mut config = test_upstream_config();
    config.url = Some("http://127.0.0.1:1/mcp".into());
    assert!(
        build_http_cancellation_sender(&config, None, None, None)
            .await
            .expect("explicitly anonymous configuration remains supported")
            .is_some()
    );
}
