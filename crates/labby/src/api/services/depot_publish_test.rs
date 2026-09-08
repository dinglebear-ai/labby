use super::*;
use crate::access::{
    AccessRuntime, AccessStore, ActivateProofInput, ConsumeBootstrapInput, LiveAuthority,
    LiveAuthorityFuture, LiveAuthoritySnapshot, StoredBinding,
};
use crate::config::{
    GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
    ProtectedMcpRouteTarget,
};
use crate::dispatch::gateway::manager::GatewayManager;
use labby_auth::depot_delegation::DepotDelegationTarget;
use labby_auth::jwt::SigningKeys;
use labby_gateway::gateway::manager::GatewayRuntimeHandle;
use std::sync::Arc;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

struct CurrentPolicy;
impl LiveAuthority for CurrentPolicy {
    fn resolve<'a>(&'a self, binding: &'a StoredBinding) -> LiveAuthorityFuture<'a> {
        Box::pin(async move {
            Ok(LiveAuthoritySnapshot {
                loadout_id: binding.loadout_id.clone(),
                loadout_generation: binding.loadout_generation,
                assignment_generation: binding.assignment_generation,
                catalog_generation: binding.catalog_generation,
                route_id: binding.route_id.clone(),
                route_generation: binding.route_generation,
                resource: binding.resource.clone(),
                audience: binding.audience.clone(),
                scopes: binding.scopes.clone(),
                requires_admin: false,
                destructive: false,
                policy_fingerprint: binding.policy_fingerprint,
            })
        })
    }
}

#[tokio::test]
async fn project_browser_publishes_through_dynamic_team_route_with_delegated_receipt() {
    publish_fixture(false, None).await;
}

#[tokio::test]
async fn approved_google_browser_publishes_without_a_product_credential() {
    publish_fixture(true, None).await;
}

#[tokio::test]
async fn ordinary_google_member_publishes_without_owner_approval_and_denies_invalid_membership() {
    publish_fixture(true, Some("member")).await;
}

#[tokio::test]
async fn google_viewer_can_publish_but_cannot_execute_manage_or_link_owner() {
    publish_fixture(true, Some("viewer")).await;
}

async fn publish_fixture(google_browser: bool, member_role: Option<&str>) {
    publish_fixture_with_revocation(google_browser, member_role, None).await;
}

#[tokio::test]
async fn product_publish_stops_after_live_credential_revocation_at_either_phase() {
    for phase in [0, 1] {
        publish_fixture_with_revocation(false, None, Some((phase,
            "UPDATE project_credentials SET status='revoked',revoked_at=1 WHERE credential_id='credential-1'",
        ))).await;
    }
}

#[tokio::test]
async fn google_publish_stops_after_live_membership_revocation_at_either_phase() {
    for phase in [0, 1] {
        publish_fixture_with_revocation(true, Some("viewer"), Some((phase,
            "UPDATE project_memberships SET status='disabled' WHERE membership_id='member-membership'",
        ))).await;
    }
}

async fn publish_fixture_with_revocation(
    google_browser: bool,
    member_role: Option<&str>,
    revoke: Option<(usize, &'static str)>,
) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let db = dir.path().canonicalize().unwrap().join("access.db");
    let store = AccessStore::open(db.clone()).await.unwrap();
    let now = labby_auth::util::now_unix();
    store
        .activate_bootstrap_proof(ActivateProofInput {
            proof_id: "proof-1".into(),
            prepare_id: "prepare-1".into(),
            installation_id: "installation-1".into(),
            installation_generation: 1,
            proof_digest: [1; 32],
            manifest_digest: [2; 32],
            request_digest: [3; 32],
            idempotency_digest: [4; 32],
            credential_id: "credential-1".into(),
            credential_digest: [5; 32],
            proof_generation: 1,
            created_at: now,
            expires_at: now + 60,
        })
        .await
        .unwrap();
    let identity = VerifiedIdentity::external(
        Authenticator::BrowserSession,
        "https://accounts.google.com",
        "operator-1",
    )
    .unwrap();
    store
        .consume_bootstrap_proof(ConsumeBootstrapInput {
            proof_id: "proof-1".into(),
            proof_digest: [1; 32],
            request_digest: [3; 32],
            idempotency_digest: [4; 32],
            organization_name: "Local".into(),
            project_name: "Default".into(),
            canonical_issuer: "https://accounts.google.com".into(),
            subject: "operator-1".into(),
            identity_fingerprint: identity.safe_fingerprint(),
            loadout_id: "production".into(),
            loadout_generation: 1,
            catalog_generation: 1,
            loadout_policy_fingerprint: [6; 32],
            route_id: "team".into(),
            route_generation: 1,
            resource: "https://labby.example/mcp/team".into(),
            audience: "https://labby.example/mcp/team".into(),
            scopes_json: r#"["lab","lab:read"]"#.into(),
            now,
            credential_expires_at: now + 3600,
        })
        .await
        .unwrap();
    drop(store);
    let runtime = AccessRuntime::initialize(db.clone()).await;
    let adapter = runtime.credential_adapter(Arc::new(CurrentPolicy));
    let source = ProductCredentialGrant {
        issuer: "https://accounts.google.com".into(),
        subject: "operator-1".into(),
        credential_id: "credential-1".into(),
        credential_generation: 1,
        scopes: vec!["lab".into(), "lab:read".into()],
        resource: "https://labby.example/mcp/team".into(),
        audience: "https://labby.example/mcp/team".into(),
        expires_at: (now + 3600) as u64,
    };
    let grant = adapter.resolve(&source).await.unwrap();
    let manager = GatewayManager::new(
        dir.path().join("gateway.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .try_seed_config(GatewayConfig {
            upstream: vec![
                serde_json::from_value(
                    json!({"name":"team-depot","url":"http://127.0.0.1:4100/mcp"}),
                )
                .unwrap(),
            ],
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                upstreams: vec!["team-depot".into()],
                ..Default::default()
            }],
            protected_mcp_routes: vec![ProtectedMcpRouteConfig {
                name: "team".into(),
                enabled: true,
                public_host: "labby.example".into(),
                public_path: "/mcp/team".into(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".into(),
                scopes: vec!["lab".into()],
                health_path: None,
                target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                    ProtectedGatewaySubsetTarget {
                        project_id: Some(grant.project_id.clone()),
                        loadout: Some("production".into()),
                        ..Default::default()
                    },
                )),
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let server = MockServer::start().await;
    for (phase, (verb, url, body)) in [
        (
            "POST",
            "/api/operations/depot.uploads.create",
            json!({"result":{"upload":{"id":"upload-1"}}}),
        ),
        (
            "PUT",
            "/uploads/upload-1",
            json!({"upload":{"id":"upload-1","status":"ready"}}),
        ),
        (
            "POST",
            "/api/operations/depot.ingest.start",
            json!({"result":{"job":{"id":"job-1","status":"queued"}}}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let db = db.clone();
        Mock::given(method(verb))
            .and(path(url))
            .respond_with(move |_: &wiremock::Request| {
                // Change the real isolated credential/membership store before
                // returning the upstream phase response, without timing sleeps.
                if let Some((revoke_phase, sql)) = revoke
                    && phase == revoke_phase
                {
                    rusqlite::Connection::open(&db)
                        .unwrap()
                        .execute_batch(sql)
                        .unwrap();
                }
                ResponseTemplate::new(200).set_body_json(body.clone())
            })
            .expect(if revoke.is_some_and(|(last, _)| phase > last) {
                0
            } else {
                1
            })
            .mount(&server)
            .await;
    }
    let keys = Arc::new(SigningKeys::load_or_create(&dir.path().join("delegation.der")).unwrap());
    let client = crate::dispatch::depot::DepotClient::for_test(
        url::Url::parse(&server.uri()).unwrap(),
        "service-token-must-not-be-used",
    )
    .with_test_delegation(
        keys,
        DepotDelegationTarget {
            issuer: "https://labby.example".into(),
            audience: server.uri(),
            deployment_id: "test-depot".into(),
            account_id: "test-account".into(),
            tenant_id: "test-tenant".into(),
            team_id: None,
        },
    );
    let mut state = AppState::new()
        .with_access_runtime(Arc::new(runtime))
        .with_access_credential_adapter(adapter);
    state.gateway_manager = Some(Arc::new(manager));
    state.depot = Arc::new(client);
    let (_session_dir, mut authority, mut auth, _) =
        super::super::tests::browser_context_for_subject(&["lab:read", "lab"], &grant.principal_id)
            .await;
    let mut browser_identity = None;
    if google_browser {
        use tower::ServiceExt as _;
        let config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some("https://labby.example".parse().unwrap()),
            sqlite_path: dir.path().join("google-auth.db"),
            key_path: dir.path().join("google-key.pem"),
            admin_email: "owner@example.com".into(),
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap(),
            ),
            google: labby_auth::config::GoogleConfig {
                client_id: "test-client".into(),
                client_secret: "test-secret".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let google = Arc::new(
            labby_auth::state::AuthState::new(config.clone())
                .await
                .unwrap(),
        );
        if member_role.is_some() {
            google
                .store
                .add_allowed_user("member@example.com", "fixture", now)
                .await
                .unwrap();
        }
        let session = labby_auth::session::create_browser_session(
            &google,
            "browser-user".into(),
            Some(
                if member_role.is_some() {
                    "member@example.com"
                } else {
                    "owner@example.com"
                }
                .into(),
            ),
        )
        .await
        .unwrap();
        let captured = Arc::new(std::sync::Mutex::new(None));
        let output = captured.clone();
        let app = axum::Router::new()
            .route(
                "/",
                get(
                    move |Extension(a): Extension<BrowserAuthority>,
                          Extension(b): Extension<AuthContext>,
                          Extension(i): Extension<VerifiedIdentity>| async move {
                        *output.lock().unwrap() = Some((a, b, i));
                        StatusCode::OK
                    },
                ),
            )
            .route_layer(
                labby_auth::middleware::AuthLayer::from_state(google)
                    .with_allow_session_cookie(true)
                    .with_static_token_scopes(if member_role.is_some() {
                        vec!["lab:read".into()]
                    } else {
                        vec!["lab:read".into(), "lab".into(), "lab:admin".into()]
                    }),
            );
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header(
                        "cookie",
                        format!("{}={}", config.session_cookie_name, session.session_id),
                    )
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let (a, b, i) = captured.lock().unwrap().take().unwrap();
        authority = a;
        auth = b;
        browser_identity = Some(i.clone());
        state.auth_config = Some(Arc::new(config));
        state.installation_id = Some(Arc::from("installation-1"));
        Arc::make_mut(&mut state.config).depot.publish =
            Some(crate::config::depot::DepotPublishTarget {
                route_id: "team".into(),
                project_id: grant.project_id.clone(),
            });
        let store = state.access_runtime.store().await.unwrap();
        if let Some(role) = member_role {
            assert!(!auth.scopes.iter().any(|s| s == "lab:admin"));
            assert_eq!(
                authorize_google(&state, &authority, &auth, Some(&i))
                    .await
                    .err(),
                Some("project_access_denied")
            );
            store.execute_test_statement("INSERT INTO principals VALUES('member-user','bootstrap-local','user','active',NULL,1,1); INSERT INTO principal_links VALUES('member-link','member-user','external','https://accounts.google.com','browser-user',NULL,'active',1,1,1,1); INSERT INTO project_memberships VALUES('member-membership','bootstrap-local','bootstrap-default','member-user','member','active','bootstrap-owner',1,1)").await.unwrap();
            if role == "viewer" {
                store.execute_test_statement("UPDATE project_memberships SET role='viewer' WHERE membership_id='member-membership'").await.unwrap();
            }
            assert!(
                authorize_google(&state, &authority, &auth, Some(&i))
                    .await
                    .is_ok()
            );
            assert!(
                store
                    .owner_link_approval(i.clone(), "installation-1".into())
                    .await
                    .is_err()
            );
            let manager = state.gateway_manager.as_ref().unwrap();
            let enabled = manager.current_config().await;
            let mut disabled = enabled.clone();
            disabled.protected_mcp_routes[0].enabled = false;
            manager.try_seed_config(disabled).await.unwrap();
            assert!(
                authorize_google(&state, &authority, &auth, Some(&i))
                    .await
                    .is_err()
            );
            manager.try_seed_config(enabled).await.unwrap();
            let original = Arc::make_mut(&mut state.config)
                .depot
                .publish
                .as_mut()
                .unwrap()
                .project_id
                .clone();
            Arc::make_mut(&mut state.config)
                .depot
                .publish
                .as_mut()
                .unwrap()
                .project_id = "wrong-team".into();
            assert!(
                authorize_google(&state, &authority, &auth, Some(&i))
                    .await
                    .is_err()
            );
            Arc::make_mut(&mut state.config)
                .depot
                .publish
                .as_mut()
                .unwrap()
                .project_id = original;
            let wrong = VerifiedIdentity::external(
                Authenticator::BrowserSession,
                "https://accounts.google.com",
                "other-subject",
            )
            .unwrap();
            assert!(
                authorize_google(&state, &authority, &auth, Some(&wrong))
                    .await
                    .is_err()
            );
            store.execute_test_statement("UPDATE project_memberships SET status='disabled' WHERE membership_id='member-membership'").await.unwrap();
            assert!(
                authorize_google(&state, &authority, &auth, Some(&i))
                    .await
                    .is_err()
            );
            store.execute_test_statement("UPDATE project_memberships SET status='active' WHERE membership_id='member-membership'; UPDATE principal_links SET status='revoked' WHERE link_id='member-link'").await.unwrap();
            assert!(
                authorize_google(&state, &authority, &auth, Some(&i))
                    .await
                    .is_err()
            );
            store
                .execute_test_statement(
                    "UPDATE principal_links SET status='active' WHERE link_id='member-link'",
                )
                .await
                .unwrap();
            assert!(
                store
                    .authorize_project(crate::access::AuthorizeProjectInput::new(
                        i.clone(),
                        grant.project_id.clone(),
                        crate::access::Permission::ProjectManage
                    ))
                    .await
                    .is_err()
            );
            if role == "viewer" {
                assert!(
                    store
                        .authorize_project(crate::access::AuthorizeProjectInput::new(
                            i.clone(),
                            grant.project_id.clone(),
                            crate::access::Permission::AssetUse
                        ))
                        .await
                        .is_err()
                );
            }
            let mut csrf = HeaderMap::new();
            csrf.insert(
                "x-csrf-token",
                auth.csrf_token.as_ref().unwrap().parse().unwrap(),
            );
            let denied = crate::api::services::owner_link::consume(
                State(state.clone()),
                Some(Extension(authority.clone())),
                Some(Extension(auth.clone())),
                Some(Extension(i.clone())),
                csrf,
                Json(crate::api::services::owner_link::EmptyRequest {}),
            )
            .await;
            assert_eq!(denied.unwrap_err().0, StatusCode::FORBIDDEN);
        } else {
            let approval = crate::access::owner_link::OwnerLinkApproval {
                approval_id: "google-consent".into(),
                identity_fingerprint: crate::access::owner_link::identity_fingerprint(&i).unwrap(),
                installation_id: "installation-1".into(),
                principal_id: grant.principal_id.clone(),
                organization_id: grant.organization_id.clone(),
                project_id: grant.project_id.clone(),
                loadout_id: "production".into(),
                route_id: "team".into(),
                resource: "https://labby.example/mcp/team".into(),
                expires_at: now + 600,
            };
            store.prepare_owner_link(approval.clone()).await.unwrap();
            assert_eq!(
                authorize_google(&state, &authority, &auth, Some(&i))
                    .await
                    .err(),
                Some("owner_link_approval_pending")
            );
            let mut csrf = HeaderMap::new();
            csrf.insert(
                "x-csrf-token",
                auth.csrf_token.as_ref().unwrap().parse().unwrap(),
            );
            let consume = crate::api::services::owner_link::consume;
            let denied = consume(
                State(state.clone()),
                Some(Extension(authority.clone())),
                Some(Extension(auth.clone())),
                Some(Extension(i.clone())),
                HeaderMap::new(),
                Json(crate::api::services::owner_link::EmptyRequest {}),
            )
            .await;
            assert_eq!(denied.unwrap_err().0, StatusCode::FORBIDDEN);
            let linked = consume(
                State(state.clone()),
                Some(Extension(authority.clone())),
                Some(Extension(auth.clone())),
                Some(Extension(i)),
                csrf,
                Json(crate::api::services::owner_link::EmptyRequest {}),
            )
            .await
            .unwrap();
            assert_eq!(
                linked.0,
                json!({"linked":true,"projectId":grant.project_id})
            );
        }
    }
    let available = availability(
        State(state.clone()),
        Some(Extension(authority.clone())),
        Extension(auth.clone()),
        (!google_browser).then(|| Extension(source.clone())),
        (!google_browser).then(|| Extension(grant.clone())),
        browser_identity.clone().map(Extension),
    )
    .await;
    assert_eq!(available.0["available"], true, "{}", available.0);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-csrf-token",
        auth.csrf_token.as_ref().unwrap().parse().unwrap(),
    );
    let result = publish(
        State(state),
        Some(Extension(authority)),
        Extension(auth),
        (!google_browser).then_some(Extension(source)),
        (!google_browser).then_some(Extension(grant)),
        browser_identity.map(Extension),
        headers,
        Json(SkillRequest {
            name: "review".into(),
            source: "---\nname: review\ndescription: Review code\n---\n# Review\n".into(),
        }),
    )
    .await;
    if let Some((phase, _)) = revoke {
        assert_eq!(result.unwrap_err().0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(server.received_requests().await.unwrap().len(), phase + 1);
    } else {
        assert_eq!(
            result.unwrap().0,
            json!({"jobId":"job-1","status":"queued"})
        );
    }
    for request in server.received_requests().await.unwrap() {
        let bearer = request
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(bearer.starts_with("Bearer ey"));
        assert!(!bearer.contains("service-token"));
    }
}
