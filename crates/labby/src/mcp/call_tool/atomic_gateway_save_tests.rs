use std::sync::Arc;

use axum::http::Request;
use rmcp::model::{CallToolRequestParams, NumberOrString};
use rmcp::service::{RequestContext, serve_directly};
use serde_json::json;

use crate::api::oauth::AuthContext;
use crate::mcp::logging::{LoggingLevel, logging_level_rank};
use crate::mcp::server::LabMcpServer;

#[tokio::test]
async fn atomic_gateway_save_mcp_rejects_selected_team_before_effects() {
    #[cfg(target_os = "macos")]
    let directory = tempfile::Builder::new()
        .prefix("labby-atomic-")
        .tempdir_in("/private/tmp")
        .unwrap();
    #[cfg(not(target_os = "macos"))]
    let directory = tempfile::Builder::new()
        .prefix("labby-atomic-")
        .tempdir()
        .unwrap();
    let directory_path = directory.path().canonicalize().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory_path.join("config.toml");
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            path.clone(),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ),
    );
    let identity = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "atomic-save-owner",
    )
    .unwrap();
    let access_runtime =
        Arc::new(crate::access::AccessRuntime::initialize(directory_path.join("access.db")).await);
    access_runtime
        .bootstrap_owner(
            crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
        )
        .await
        .unwrap();
    let server = LabMcpServer {
        installation_id: None,
        registry: Arc::new(crate::registry::build_default_registry()),
        access_runtime,
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        gateway_manager: Some(manager.clone()),
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        client_registry: Default::default(),
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            LoggingLevel::Emergency,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running =
        serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
    let mut context = RequestContext::new(NumberOrString::Number(1), running.peer().clone());
    let mut parts = Request::builder()
        .uri("https://example.test/mcp")
        .body(())
        .unwrap()
        .into_parts()
        .0;
    parts.extensions.insert(identity);
    parts.extensions.insert(AuthContext {
        sub: "atomic-save-owner".into(),
        actor_key: None,
        scopes: vec!["lab:admin".into()],
        issuer: "test".into(),
        via_session: false,
        csrf_token: None,
        email: None,
    });
    context.extensions.insert(parts);
    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("gateway").with_arguments(serde_json::Map::from_iter([
            ("action".into(), json!("gateway.add")),
            ("params".into(), json!({"team_id":"alpha", "spec":{"name":"must-not-install", "url":"https://example.test/mcp"},
                "protected_route":{"operation":"upsert", "route":{
                    "name":"route","enabled":true,"public_host":"mcp.example.test","public_path":"/route",
                    "backend_url":"https://example.test/mcp","backend_mcp_path":"/mcp","scopes":["mcp:read"]
                }}})),
        ])), context,
    )).await.unwrap();
    assert!(result.is_error.unwrap_or(false));
    let error = result.content[0].as_text().unwrap().text.as_str();
    assert!(error.contains("invalid_param"), "{error}");
    assert!(error.contains("Team"), "{error}");
    assert!(manager.current_config().await.upstream.is_empty());
    assert!(!path.exists());
}
