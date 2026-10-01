use super::*;
use crate::access::test_support::{browser, fixture};
use serde_json::json;

#[tokio::test]
async fn sessions_are_principal_bound_and_all_selected_clients_need_observed_success() {
    let (_root, store, identity) = fixture().await;
    let issued: Issued = serde_json::from_value(
        start(
            store.clone(),
            identity.clone(),
            json!({"clients":["codex","claude-code"]}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(validate(&store, "unknown-user", &issued.sessions["codex"]).is_err());
    assert!(validate(&store, "bootstrap-owner", "fake-client-success").is_err());
    assert!(
        start(
            store.clone(),
            browser("unknown-user"),
            json!({"clients":["codex"]})
        )
        .await
        .is_err()
    );
    assert!(
        start(
            store.clone(),
            identity.clone(),
            json!({"clients":["codex"],"principal_id":"other"})
        )
        .await
        .is_err()
    );
    let codex = validate(&store, "bootstrap-owner", &issued.sessions["codex"]).unwrap();
    observed_success(store.clone(), codex.clone())
        .await
        .unwrap();
    let state = readiness::state_for_identity(store.clone(), identity.clone())
        .await
        .unwrap();
    assert_eq!(state["checks"][3]["status"], "pending");
    let claude = validate(&store, "bootstrap-owner", &issued.sessions["claude-code"]).unwrap();
    observed_success(store.clone(), claude).await.unwrap();
    let state = readiness::state_for_identity(store.clone(), identity.clone())
        .await
        .unwrap();
    assert_eq!(state["checks"][3]["status"], "verified");
    revoke(store.clone(), identity.clone()).await.unwrap();
    assert!(validate(&store, "bootstrap-owner", &issued.sessions["codex"]).is_err());
    assert!(observed_success(store.clone(), codex).await.is_err());
    assert_eq!(
        readiness::state_for_identity(store, identity)
            .await
            .unwrap()["checks"][3]["status"],
        "pending"
    );
}

#[tokio::test]
async fn changed_selection_expiry_and_configuration_reject_old_bindings() {
    let (_root, store, identity) = fixture().await;
    let old: Issued = serde_json::from_value(
        start(
            store.clone(),
            identity.clone(),
            json!({"clients":["codex"]}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let old_binding = validate(&store, "bootstrap-owner", &old.sessions["codex"]).unwrap();
    let current: Issued = serde_json::from_value(
        start(
            store.clone(),
            identity.clone(),
            json!({"clients":["claude-code"]}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(validate(&store, "bootstrap-owner", &old.sessions["codex"]).is_err());
    assert!(observed_success(store.clone(), old_binding).await.is_err());
    let current_binding =
        validate(&store, "bootstrap-owner", &current.sessions["claude-code"]).unwrap();
    let lock = lock(&store.storage_dir(), "bootstrap-owner").unwrap();
    let mut row = read(lock.path()).unwrap();
    row.expires = 0;
    save(&lock, &row).unwrap();
    drop(lock);
    assert!(validate(&store, "bootstrap-owner", &current.sessions["claude-code"]).is_err());
    assert!(
        observed_success(store.clone(), current_binding)
            .await
            .is_err()
    );
    let next: Issued = serde_json::from_value(
        start(store.clone(), identity, json!({"clients":["codex"]}))
            .await
            .unwrap(),
    )
    .unwrap();
    let binding = validate(&store, "bootstrap-owner", &next.sessions["codex"]).unwrap();
    labby_runtime::secure_atomic_file::write_secure_atomic(
        &store.storage_dir().join("config.toml"),
        b"[mcp]\nport = 19876\n",
    )
    .unwrap();
    assert!(validate(&store, "bootstrap-owner", &next.sessions["codex"]).is_err());
    assert!(observed_success(store, binding).await.is_err());
}

#[cfg(all(unix, feature = "gateway"))]
#[tokio::test]
async fn actual_http_tool_completion_records_client_use_but_listing_and_errors_do_not() {
    use axum::{Json, Router, routing::get};
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ServerCapabilities,
        ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    };
    use rmcp::{RoleServer, ServerHandler};
    use std::sync::Arc;
    drop(rustls::crypto::ring::default_provider().install_default());
    let directory = crate::access::test_support::secure_tempdir();
    let store = AccessStore::open(directory.path().join("access.db"))
        .await
        .unwrap();
    let identity = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "static-bearer:primary",
    )
    .unwrap();
    store
        .bootstrap_owner(
            crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
        )
        .await
        .unwrap();
    drop(store);
    let runtime = Arc::new(
        crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
    );
    let store = runtime.store().await.unwrap();
    let issued: Issued = serde_json::from_value(
        start(
            store.clone(),
            identity.clone(),
            json!({"clients":["codex"]}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    #[derive(Clone)]
    struct Fixture(Arc<crate::access::AccessRuntime>);
    impl ServerHandler for Fixture {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }
        async fn list_tools(
            &self,
            _: Option<rmcp::model::PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
            Ok(serde_json::from_value(json!({"tools":[{"name":"success","inputSchema":{"type":"object"}},{"name":"error","inputSchema":{"type":"object"}}]})).unwrap())
        }
        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, rmcp::ErrorData> {
            let result = if request.name == "error" {
                CallToolResult::error(vec![ContentBlock::text("fixture tool error")])
            } else {
                CallToolResult::success(vec![ContentBlock::text("actual-tool-result")])
            };
            let result = CallToolResponse::Complete(result);
            record_completed_tool(
                &self.0,
                binding_from_extensions(&context.extensions),
                &result,
            )
            .await;
            Ok(result)
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway = format!("http://{address}");
    let state = crate::api::state::AppState::new().with_access_runtime(runtime.clone());
    let factory = runtime.clone();
    let mcp = StreamableHttpService::new(
        move || Ok(Fixture(factory.clone())),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(vec![address.to_string()])
            .with_legacy_session_mode(false),
    );
    let router = Router::new()
        .nest_service("/mcp", mcp)
        .route(
            "/v1/gateway/actions",
            get(|| async { Json(json!([{"name":"gateway.reload"}])) }),
        )
        .route_layer(axum::middleware::from_fn_with_state(state, middleware))
        .route_layer(
            labby_auth::AuthLayer::new().with_static_token(Some(Arc::from("fixture-auth"))),
        )
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }));
    let stop = tokio_util::sync::CancellationToken::new();
    let cancel = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .unwrap()
    });
    let live = crate::live_gateway::detect_bound_bearer_target(&gateway, "fixture-auth".into())
        .await
        .unwrap();
    let client = live
        .connect_service_with_evidence((), Some(&issued.sessions["codex"]))
        .await
        .unwrap();
    client.peer().list_tools(None).await.unwrap();
    assert_eq!(
        readiness::state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap()["checks"][3]["status"],
        "pending"
    );
    client
        .peer()
        .call_tool(CallToolRequestParams::new("error"))
        .await
        .unwrap();
    assert_eq!(
        readiness::state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap()["checks"][3]["status"],
        "pending"
    );
    client
        .peer()
        .call_tool(CallToolRequestParams::new("success"))
        .await
        .unwrap();
    assert_eq!(
        readiness::state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap()["checks"][3]["status"],
        "verified"
    );
    drop(client.cancel().await);
    let no_auth = reqwest::Client::new()
        .post(format!("{gateway}/mcp"))
        .header(HEADER, &issued.sessions["codex"])
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(no_auth.status(), reqwest::StatusCode::UNAUTHORIZED);
    revoke(store, identity).await.unwrap();
    assert!(
        live.connect_service_with_evidence((), Some(&issued.sessions["codex"]))
            .await
            .is_err()
    );
    stop.cancel();
    server.await.unwrap();
}
