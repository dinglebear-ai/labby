use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use serde_json::json;
use tower::ServiceExt as _;

use crate::authorize::tests::{test_auth_config, test_auth_state_with_config};
use crate::browser_authority::BrowserAuthority;
use crate::middleware::AuthLayer;
use crate::state::AuthState;
use crate::types::BrowserSessionRow;

async fn fixture(email: &str, verified: bool) -> (Arc<AuthState>, BrowserSessionRow) {
    let mut config = test_auth_config();
    config.viewer_email_domains = vec!["lime-technology.com".into()];
    let state = Arc::new(test_auth_state_with_config(config).await);
    let session = crate::session::create_bound_browser_session(
        &state,
        "viewer-provider-subject".into(),
        Some(email.into()),
        state.inbound_provider_binding(),
    )
    .await
    .unwrap();
    if verified {
        state
            .store
            .upsert_bound_verified_inbound_identity(
                &session.subject,
                email,
                crate::util::now_unix(),
                state.inbound_provider_binding(),
            )
            .await
            .unwrap();
    }
    (state, session)
}

#[test]
fn viewer_policy_is_default_off_and_exact_verified_email_only() {
    let mut config = test_auth_config();
    assert!(config.viewer_email_domains.is_empty());
    assert!(
        config
            .viewer_domain_for_verified_email(Some("member@lime-technology.com"), Some(true))
            .is_none()
    );
    config.viewer_email_domains = vec!["lime-technology.com".into()];
    assert_eq!(
        config
            .viewer_domain_for_verified_email(Some("Member@LIME-TECHNOLOGY.COM"), Some(true))
            .as_deref(),
        Some("lime-technology.com")
    );
    for email in [
        "member@sub.lime-technology.com",
        "member@lime-technology.com.evil",
        "member@lime-technоlogy.com",
        "member@evil.com",
        "@lime-technology.com",
        "a@b@lime-technology.com",
        " member@lime-technology.com",
        "member@lime-technology.com.",
    ] {
        assert!(
            config
                .viewer_domain_for_verified_email(Some(email), Some(true))
                .is_none(),
            "{email}"
        );
    }
    for verified in [None, Some(false)] {
        assert!(
            config
                .viewer_domain_for_verified_email(Some("member@lime-technology.com"), verified)
                .is_none()
        );
    }
}

#[tokio::test]
async fn domain_only_session_and_opaque_authority_never_inherit_admin_scopes() {
    for (email, verified, expected) in [
        ("member@lime-technology.com", true, StatusCode::OK),
        (
            "member@lime-technology.com",
            false,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "member@sub.lime-technology.com",
            true,
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let (state, session) = fixture(email, verified).await;
        let cookie = format!(
            "{}={}",
            state.config.session_cookie_name, session.session_id
        );
        let app = Router::new().route("/probe", get(
            |axum::Extension(context): axum::Extension<crate::AuthContext>,
             axum::Extension(authority): axum::Extension<BrowserAuthority>| async move {
                assert_eq!(context.scopes, vec!["lab:read"]);
                let grant = authority.revalidate().await.unwrap();
                assert!(grant.has_scope("lab:read"));
                assert!(!grant.has_scope("lab:admin"));
                assert!(!grant.has_scope("lab"));
                let proof = authority.verified_viewer_domain().await.unwrap().unwrap();
                assert_eq!(proof.domain(), "lime-technology.com");
                proof.revalidate().await.unwrap();
                StatusCode::OK
            }
        )).layer(AuthLayer::from_state(state).with_allow_session_cookie(true)
            .with_static_token_scopes(vec!["lab".into(), "lab:admin".into(), "lab:read".into()]));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

#[tokio::test]
async fn explicitly_allowlisted_domain_member_keeps_existing_admin_behavior() {
    let (state, session) = fixture("existing-admin@lime-technology.com", true).await;
    state
        .store
        .add_allowed_user(
            "existing-admin@lime-technology.com",
            "operator",
            crate::util::now_unix(),
        )
        .await
        .unwrap();
    let cookie = format!(
        "{}={}",
        state.config.session_cookie_name, session.session_id
    );
    let app = Router::new()
        .route(
            "/probe",
            get(
                |axum::Extension(context): axum::Extension<crate::AuthContext>| async move {
                    assert!(context.scopes.iter().any(|scope| scope == "lab:admin"));
                    StatusCode::OK
                },
            ),
        )
        .layer(
            AuthLayer::from_state(state)
                .with_allow_session_cookie(true)
                .with_static_token_scopes(vec!["lab:admin".into()]),
        );
    assert_eq!(
        app.oneshot(
            Request::builder()
                .uri("/probe")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn viewer_proof_rejects_changed_verified_email_revocation_and_provider_generation() {
    for change in ["email", "revocation", "provider"] {
        let (state, session) = fixture("member@lime-technology.com", true).await;
        let authority =
            BrowserAuthority::from_google(state.clone(), session.clone(), vec!["lab:read".into()])
                .await
                .unwrap();
        let proof = authority.verified_viewer_domain().await.unwrap().unwrap();
        let debug = format!("{proof:?}");
        assert!(!debug.contains("member") && !debug.contains(&session.subject));
        match change {
            "email" => state
                .store
                .upsert_bound_verified_inbound_identity(
                    &session.subject,
                    "another@lime-technology.com",
                    crate::util::now_unix(),
                    state.inbound_provider_binding(),
                )
                .await
                .unwrap(),
            "revocation" => {
                state
                    .store
                    .revoke_inbound_identity(
                        &state.inbound_provider_binding().identity_issuer,
                        &session.subject,
                    )
                    .await
                    .unwrap();
            }
            _ => {
                state
                    .store
                    .activate_inbound_provider(
                        "google",
                        "https://accounts.google.com",
                        "changed-provider",
                        crate::util::now_unix(),
                    )
                    .await
                    .unwrap();
            }
        }
        assert!(proof.revalidate().await.is_err(), "{change}");
    }
}

#[tokio::test]
async fn removed_viewer_policy_denies_existing_session_and_missing_evidence_never_becomes_proof() {
    let (state, session) = fixture("member@lime-technology.com", true).await;
    let mut config = (*state.config).clone();
    config.viewer_email_domains.clear();
    let removed = Arc::new(AuthState::for_tests(
        config,
        state.store.clone(),
        (*state.signing_keys).clone(),
        state.google().clone(),
    ));
    let authority = BrowserAuthority::from_google(removed, session, vec!["lab:read".into()])
        .await
        .unwrap();
    assert!(authority.verified_viewer_domain().await.unwrap().is_none());
    let (state, session) = fixture("member@lime-technology.com", false).await;
    let authority = BrowserAuthority::from_google(state, session, vec!["lab:read".into()])
        .await
        .unwrap();
    assert!(authority.verified_viewer_domain().await.unwrap().is_none());
}

#[tokio::test]
async fn callback_admits_only_verified_domain_and_persists_real_bound_evidence() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    for verified in [true, false] {
        let mut config = test_auth_config();
        config.viewer_email_domains = vec!["lime-technology.com".into()];
        config.allowed_email_domains = vec!["unraid.net".into()];
        let base = test_auth_state_with_config(config).await;
        let server = MockServer::start().await;
        let id_token = crate::authorize::tests::signed_test_id_token_with_email_and_hosted_domain(
            "new-member@lime-technology.com",
            verified,
            Some("unraid.net"),
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"fixture-access", "expires_in":3600, "id_token":id_token}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(crate::authorize::tests::test_jwks()),
            )
            .mount(&server)
            .await;
        let google = crate::google::GoogleProvider::new(
            "client-id".into(),
            "client-secret".into(),
            "https://lab.example.com/auth/google/callback"
                .parse()
                .unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse().unwrap(),
            format!("{}/token", server.uri()).parse().unwrap(),
        )
        .with_jwks_endpoint(format!("{}/certs", server.uri()).parse().unwrap());
        let state = AuthState::for_tests(
            (*base.config).clone(),
            base.store.clone(),
            (*base.signing_keys).clone(),
            google,
        );
        state
            .store
            .insert_browser_login_state(crate::types::BrowserLoginStateRow {
                state: "viewer-login".into(),
                return_to: "/".into(),
                provider_code_verifier: "fixture-verifier".into(),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 300,
            })
            .await
            .unwrap();
        let response = crate::routes::router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=viewer-login&code=fixture-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().is_redirection(), verified);
        assert_eq!(
            response.headers().contains_key(header::SET_COOKIE),
            verified
        );
        let evidence = state
            .store
            .current_verified_inbound_email("https://accounts.google.com", "google-subject-123")
            .await
            .unwrap();
        assert_eq!(evidence.is_some(), verified);
        assert!(state.store.list_allowed_users().await.unwrap().is_empty());
        // The same signed email + legacy administrative `hd` must not mint an
        // OAuth-client admin grant. Viewer admission is deliberately browser-only.
        state
            .store
            .register_client(crate::types::RegisteredClient {
                client_id: "viewer-client".into(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".into()],
                created_at: crate::util::now_unix(),
                token_endpoint_auth_method: "none".into(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        state
            .store
            .insert_authorization_request(crate::types::AuthorizationRequestRow {
                state: "viewer-oauth".into(),
                client_id: "viewer-client".into(),
                redirect_uri: "http://127.0.0.1:7777/callback".into(),
                client_state: "viewer-client-state".into(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".into(),
                scope: "lab:admin".into(),
                provider_code_verifier: "fixture-verifier".into(),
                code_challenge: "challenge".into(),
                code_challenge_method: "S256".into(),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 300,
            })
            .await
            .unwrap();
        let denied = crate::routes::router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=viewer-oauth&code=fixture-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let location: url::Url = denied
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            location
                .query_pairs()
                .any(|(key, value)| key == "error" && value == "access_denied")
        );
        assert!(!location.query_pairs().any(|(key, _)| key == "code"));
        assert!(
            state
                .store
                .find_google_provider_credential("google-subject-123")
                .await
                .unwrap()
                .is_none()
        );
        if verified {
            let cookie = response
                .headers()
                .get(header::SET_COOKIE)
                .unwrap()
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_string();
            let app = Router::new().route("/probe", get(
                |axum::Extension(context): axum::Extension<crate::AuthContext>,
                 axum::Extension(authority): axum::Extension<BrowserAuthority>| async move {
                    assert_eq!(context.scopes, vec!["lab:read"]);
                    assert!(!authority.revalidate().await.unwrap().has_scope("lab:admin"));
                    assert!(authority.verified_viewer_domain().await.unwrap().is_some());
                    StatusCode::OK
                }
            )).layer(AuthLayer::from_state(Arc::new(state)).with_allow_session_cookie(true)
                .with_static_token_scopes(vec!["lab:admin".into(), "lab".into()]));
            assert_eq!(
                app.oneshot(
                    Request::builder()
                        .uri("/probe")
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
                StatusCode::OK
            );
        }
    }
}
