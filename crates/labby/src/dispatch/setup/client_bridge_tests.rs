use super::*;
fn root() -> tempfile::TempDir {
    #[cfg(target_os = "macos")]
    let parent = PathBuf::from("/private/tmp");
    #[cfg(not(target_os = "macos"))]
    let parent = std::env::temp_dir().canonicalize().unwrap();
    tempfile::Builder::new()
        .prefix("lab-bridge-")
        .tempdir_in(parent)
        .unwrap()
}
#[cfg(unix)]
fn saved(root: &Path, server: &str) {
    labby_runtime::secure_atomic_file::write_secure_atomic(
        &root.join(".env"),
        format!("LABBY_SERVER_URL=\"{server}\"\nLABBY_MCP_HTTP_TOKEN=\"fixture-credential\"\n")
            .as_bytes(),
    )
    .unwrap();
}
#[cfg(unix)]
#[test]
fn saved_credential_is_bound_to_explicit_target_and_not_in_descriptor() {
    let directory = root();
    saved(directory.path(), "http://127.0.0.1:8765");
    assert!(credentials(directory.path(), "https://other.example.com").is_err());
    assert_eq!(
        credentials(directory.path(), "http://127.0.0.1:8765/mcp")
            .unwrap()
            .1,
        "fixture-credential"
    );
    let descriptor = descriptor(
        directory.path(),
        "http://127.0.0.1:8765",
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    assert!(
        !serde_json::to_string(&descriptor)
            .unwrap()
            .contains("fixture-credential")
    );
}
#[cfg(unix)]
#[test]
fn bridge_refuses_publicly_readable_credentials_and_symlinks() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = root();
    saved(directory.path(), "http://127.0.0.1:8765");
    std::fs::set_permissions(
        directory.path().join(".env"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(credentials(directory.path(), "http://127.0.0.1:8765").is_err());
    std::fs::remove_file(directory.path().join(".env")).unwrap();
    let outside = root();
    saved(outside.path(), "http://127.0.0.1:8765");
    std::os::unix::fs::symlink(outside.path().join(".env"), directory.path().join(".env")).unwrap();
    assert!(credentials(directory.path(), "http://127.0.0.1:8765").is_err());
}

#[cfg(all(unix, feature = "gateway"))]
#[tokio::test]
async fn actual_stdio_http_bridge_lists_and_calls_fixture_tool_with_saved_auth() {
    drop(rustls::crypto::ring::default_provider().install_default());
    use axum::{Json, Router, routing::get};
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    };
    use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt as _};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    #[derive(Clone)]
    struct Fixture;
    impl ServerHandler for Fixture {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }
        async fn list_tools(
            &self,
            _: Option<PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(serde_json::from_value(
                serde_json::json!({"tools":[{"name":"fixture","inputSchema":{"type":"object"}}]}),
            )
            .unwrap())
        }
        async fn call_tool(
            &self,
            params: CallToolRequestParams,
            _: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            assert_eq!(params.name, "fixture");
            Ok(CallToolResponse::Complete(serde_json::from_value(serde_json::json!({"content":[{"type":"text","text":"round-trip-confirmed"}],"isError":false})).unwrap()))
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = format!("http://{address}");
    let stop = CancellationToken::new();
    let service = StreamableHttpService::new(
        || Ok(Fixture),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(vec![address.to_string()])
            .with_legacy_session_mode(false),
    );
    let protected = Router::new()
        .nest_service("/mcp", service)
        .route(
            "/v1/gateway/actions",
            get(|| async { Json(serde_json::json!([{"name":"gateway.reload"}])) }),
        )
        .layer(
            labby_auth::AuthLayer::new().with_static_token(Some(Arc::from("fixture-credential"))),
        );
    let router = Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok"})) }),
        )
        .merge(protected);
    let cancel = stop.clone();
    let http = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .unwrap();
    });
    let directory = root();
    saved(directory.path(), &server);
    let (client_io, bridge_io) = tokio::io::duplex(256 * 1024);
    let (read, write) = tokio::io::split(bridge_io);
    let bridge = tokio::spawn(run_io(
        directory.path().to_path_buf(),
        server.clone(),
        read,
        write,
    ));
    let client = ().serve(client_io).await.unwrap();
    let listed = client.peer().list_tools(None).await.unwrap();
    assert_eq!(listed.tools[0].name, "fixture");
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("fixture"))
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&result)
            .unwrap()
            .contains("round-trip-confirmed")
    );
    client.cancel().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), bridge)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        verify(directory.path().to_path_buf(), server)
            .await
            .unwrap(),
        1
    );
    stop.cancel();
    http.await.unwrap();
}

#[cfg(unix)]
#[test]
fn saved_identity_reads_actual_target_and_rejects_duplicate_assignments() {
    let directory = root();
    saved(directory.path(), "http://127.0.0.1:9876");
    let (server, bearer) = saved_connection_identity(directory.path()).unwrap();
    assert_eq!(server, "http://127.0.0.1:9876/");
    assert!(bearer);
    labby_runtime::secure_atomic_file::write_secure_atomic(
        &directory.path().join(".env"),
        b"LABBY_SERVER_URL=https://example.com\nLABBY_MCP_HTTP_TOKEN=\n",
    )
    .unwrap();
    assert_eq!(
        saved_connection_identity(directory.path()).unwrap(),
        ("https://example.com/".to_owned(), false)
    );
    labby_runtime::secure_atomic_file::write_secure_atomic(
        &directory.path().join(".env"),
        b"LABBY_SERVER_URL=https://example.com\nLABBY_SERVER_URL=https://other.example.com\n",
    )
    .unwrap();
    assert!(saved_connection_identity(directory.path()).is_err());
}
