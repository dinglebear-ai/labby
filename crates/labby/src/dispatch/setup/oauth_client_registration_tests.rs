use super::*;

#[test]
fn protected_header_rotation_preserves_unrelated_entries_and_rejects_custom_auth() {
    for (client, original) in [
        (
            ExternalClient::Codex,
            "# existing comment\nmodel = 'selected'\n[mcp_servers.other]\ncommand = 'keep'\n",
        ),
        (
            ExternalClient::ClaudeCode,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"keep"}}}"#,
        ),
    ] {
        let gateway = "https://lab.example/mcp";
        let first = render(client, original, gateway, &"a".repeat(64), None).unwrap();
        let rotated = render(
            client,
            &first,
            gateway,
            &"b".repeat(64),
            Some(&"a".repeat(64)),
        )
        .unwrap();
        assert!(rotated.contains("keep"));
        assert!(!rotated.contains(&"a".repeat(64)));
        assert!(!rotated.contains("Authorization"));
        assert!(
            render(
                client,
                &first,
                gateway,
                &"b".repeat(64),
                Some("incorrect-private-record")
            )
            .is_err()
        );
        assert_eq!(
            render(
                client,
                &rotated,
                gateway,
                &"b".repeat(64),
                Some(&"b".repeat(64))
            )
            .unwrap(),
            rotated
        );
        let value = entry(client, &rotated).unwrap().unwrap();
        assert_eq!(
            value[header_key(client)][crate::dispatch::setup::client_evidence::HEADER],
            "b".repeat(64)
        );
    }
}

#[test]
fn observed_registration_output_redacts_proof_and_keeps_private_backup() {
    let home = crate::access::test_support::secure_tempdir();
    let gateway = "https://lab.example/mcp";
    for client in [ExternalClient::Codex, ExternalClient::ClaudeCode] {
        let original = match client {
            ExternalClient::Codex => "model='keep'\n",
            ExternalClient::ClaudeCode => "{\"theme\":\"keep\"}",
        };
        let path = target(home.path(), client).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        labby_runtime::secure_atomic_file::write_secure_atomic(&path, original.as_bytes()).unwrap();
        let proof = "c".repeat(64);
        let result = apply(
            home.path(),
            client,
            gateway,
            &version(original),
            &proof,
            None,
        )
        .unwrap();
        assert!(!serde_json::to_string(&result).unwrap().contains(&proof));
        assert!(!format!("{result:?}").contains(&proof));
        assert!(!result.connected);
        assert!(result.plan.readiness_supported);
        let backup = result.backup_path.unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o077,
                0
            );
            assert_eq!(
                std::fs::metadata(backup).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }
}

#[tokio::test]
async fn signed_oauth_descriptor_calls_are_observed_per_client_and_principal() {
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
    let home = crate::access::test_support::secure_tempdir();
    let local = crate::access::test_support::secure_tempdir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base = format!("http://{address}");
    let gateway = format!("{base}/mcp");
    let auth = Arc::new(
        labby_auth::state::AuthState::new(labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://fixture-issuer.example").unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("jwt.pem"),
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                )
                .unwrap(),
            ),
            google: labby_auth::config::GoogleConfig {
                client_id: "fixture-client".into(),
                client_secret: "fixture-secret".into(),
                callback_path: "/auth/google/callback".into(),
                scopes: vec!["openid".into(), "email".into(), "profile".into()],
                ..labby_auth::config::GoogleConfig::default()
            },
            ..labby_auth::config::AuthConfig::default()
        })
        .await
        .unwrap(),
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize;
    let claims = |subject: &str| labby_auth::jwt::AccessClaims {
        iss: "https://fixture-issuer.example".into(),
        sub: subject.into(),
        aud: gateway.clone(),
        exp: now + 3600,
        nbf: None,
        iat: now,
        jti: uuid::Uuid::new_v4().to_string(),
        scope: "lab:read".into(),
        azp: "fixture-cli".into(),
        identity_issuer: Some("https://accounts.google.com".into()),
        identity_credential_id: None,
    };
    let owner_claims = claims("owner");
    let identity =
        labby_auth::verified_identity_from_access_claims(&owner_claims, &auth.config).unwrap();
    let store = crate::access::AccessStore::open(directory.path().join("access.db"))
        .await
        .unwrap();
    store
        .bootstrap_owner(
            crate::access::BootstrapOwnerInput::new(identity.clone(), "Owner", "Workspace")
                .unwrap(),
        )
        .await
        .unwrap();
    drop(store);
    let runtime = Arc::new(
        crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
    );
    let store = runtime.store().await.unwrap();
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
            Ok(serde_json::from_value(serde_json::json!({"tools":[{"name":"success","inputSchema":{"type":"object"}},{"name":"error","inputSchema":{"type":"object"}}]})).unwrap())
        }
        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, rmcp::ErrorData> {
            let result = CallToolResponse::Complete(if request.name == "error" {
                CallToolResult::error(vec![ContentBlock::text("fixture error")])
            } else {
                CallToolResult::success(vec![ContentBlock::text("genuine completed result")])
            });
            crate::dispatch::setup::client_evidence::record_completed_tool(
                &self.0,
                crate::dispatch::setup::client_evidence::binding_from_extensions(
                    &context.extensions,
                ),
                &result,
            )
            .await;
            Ok(result)
        }
    }
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
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::dispatch::setup::client_evidence::middleware,
        ))
        .nest(
            "/v1/setup",
            crate::api::services::setup::routes(state.clone()).router,
        )
        .route(
            "/v1/gateway/actions",
            get(|| async { Json(serde_json::json!([{"name":"gateway.reload"}])) }),
        )
        .route_layer(
            labby_auth::AuthLayer::new()
                .with_auth_state(Some(auth.clone()))
                .with_resource_url(Some(Arc::from(gateway.as_str()))),
        )
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok"})) }),
        )
        .with_state(state);
    let stop = tokio_util::sync::CancellationToken::new();
    let cancel = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .unwrap()
    });
    let access = auth.signing_keys.issue_access_token(&owner_claims).unwrap();
    let live = crate::live_gateway::detect_bound_bearer_target(&base, access.clone())
        .await
        .unwrap();
    let results = register_authenticated(
        home.path().into(),
        local.path().into(),
        gateway.clone(),
        vec![
            ExternalClient::ClaudeCode,
            ExternalClient::Codex,
            ExternalClient::Codex,
        ],
        &live,
    )
    .await
    .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["client"], "codex");
    assert_eq!(results[1]["client"], "claude-code");
    assert!(!serde_json::to_string(&results).unwrap().contains(&access));
    let mut proofs = BTreeMap::new();
    for client in [ExternalClient::Codex, ExternalClient::ClaudeCode] {
        let raw = std::fs::read_to_string(target(home.path(), client).unwrap()).unwrap();
        assert!(!raw.contains(&access));
        assert!(!raw.contains("Authorization"));
        let descriptor = entry(client, &raw).unwrap().unwrap();
        assert_eq!(descriptor["url"], gateway);
        proofs.insert(
            client.key(),
            descriptor[header_key(client)][crate::dispatch::setup::client_evidence::HEADER]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let mut codex_claims = claims("owner");
    codex_claims.azp = "fixture-codex".into();
    let codex_access = auth.signing_keys.issue_access_token(&codex_claims).unwrap();
    let codex_live = crate::live_gateway::detect_bound_bearer_target(&base, codex_access)
        .await
        .unwrap();
    let mut claude_claims = claims("owner");
    claude_claims.azp = "fixture-claude-code".into();
    let claude_access = auth
        .signing_keys
        .issue_access_token(&claude_claims)
        .unwrap();
    let claude_live = crate::live_gateway::detect_bound_bearer_target(&base, claude_access)
        .await
        .unwrap();
    let client = codex_live
        .connect_service_with_evidence((), Some(&proofs["codex"]))
        .await
        .unwrap();
    client.peer().list_tools(None).await.unwrap();
    client
        .peer()
        .call_tool(CallToolRequestParams::new("error"))
        .await
        .unwrap();
    assert_eq!(
        crate::dispatch::setup::readiness::state_for_identity(store.clone(), identity.clone())
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
        crate::dispatch::setup::readiness::state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap()["checks"][3]["status"],
        "pending"
    );
    drop(client.cancel().await);
    let other = auth
        .signing_keys
        .issue_access_token(&claims("different-user"))
        .unwrap();
    let otherlive = crate::live_gateway::detect_bound_bearer_target(&base, other)
        .await
        .unwrap();
    assert!(
        otherlive
            .connect_service_with_evidence((), Some(&proofs["claude-code"]))
            .await
            .is_err()
    );
    let client = claude_live
        .connect_service_with_evidence((), Some(&proofs["claude-code"]))
        .await
        .unwrap();
    client
        .peer()
        .call_tool(CallToolRequestParams::new("success"))
        .await
        .unwrap();
    drop(client.cancel().await);
    assert_eq!(
        crate::dispatch::setup::readiness::state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap()["checks"][3]["status"],
        "verified"
    );
    // Expiring the supplemental observation does not revoke the primary OAuth
    // grant or turn a historical successful-use receipt into a fresh receipt.
    let principal = crate::access::resolve_personal_owner(&store, identity.clone())
        .await
        .unwrap()
        .id()
        .to_owned();
    let journal = store.storage_dir().join("first-use-clients").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(principal.as_bytes()))
    ));
    let mut row: serde_json::Value = serde_json::from_slice(
        &crate::dispatch::setup::secure_file::read_private(&journal).unwrap(),
    )
    .unwrap();
    row["expires"] = serde_json::json!(0);
    labby_runtime::secure_atomic_file::write_secure_atomic(
        &journal,
        &serde_json::to_vec(&row).unwrap(),
    )
    .unwrap();
    let before =
        crate::dispatch::setup::readiness::state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap();
    let expired = codex_live
        .connect_service_with_evidence((), Some(&proofs["codex"]))
        .await
        .unwrap();
    expired
        .peer()
        .call_tool(CallToolRequestParams::new("success"))
        .await
        .unwrap();
    drop(expired.cancel().await);
    assert_eq!(
        before,
        crate::dispatch::setup::readiness::state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap()
    );
    // Reconnecting rotates both private descriptors and the server selection.
    register_authenticated(
        home.path().into(),
        local.path().into(),
        gateway.clone(),
        vec![ExternalClient::Codex, ExternalClient::ClaudeCode],
        &live,
    )
    .await
    .unwrap();
    assert!(
        live.connect_service_with_evidence((), Some(&proofs["codex"]))
            .await
            .is_err()
    );
    assert_eq!(
        crate::dispatch::setup::readiness::state_for_identity(store, identity)
            .await
            .unwrap()["checks"][3]["status"],
        "pending"
    );
    stop.cancel();
    server.await.unwrap();
}
