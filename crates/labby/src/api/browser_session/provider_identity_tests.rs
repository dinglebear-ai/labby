use super::*;
use base64::Engine as _;
use std::sync::Arc;
use tower::ServiceExt as _;

/// Discovery is the only network operation in this fixture. The stored session
/// represents facts already verified by the provider callback.
async fn authelia_state(
    directory: &std::path::Path,
    path: &str,
) -> (labby_auth::state::AuthState, tokio::task::JoinHandle<()>) {
    drop(rustls::crypto::ring::default_provider().install_default());
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let ca_path = directory.join("issuer-ca.pem");
    let encoded = base64::engine::general_purpose::STANDARD.encode(cert.der());
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).unwrap());
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    std::fs::write(&ca_path, pem).unwrap();
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let issuer = format!("{origin}{path}");
    let document = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/api/oidc/authorization"),
        "token_endpoint": format!("{issuer}/api/oidc/token"),
        "jwks_uri": format!("{issuer}/jwks.json"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
        "id_token_signing_alg_values_supported": ["RS256"]
    });
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    // AuthState initialization performs one bounded discovery request. No
    // detached accept loop or process-global trust-store changes are needed.
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let stream = acceptor.accept(socket).await.unwrap();
        let service = hyper::service::service_fn(move |_request| {
            let body = document.to_string();
            async move {
                Ok::<_, std::convert::Infallible>(
                    hyper::Response::builder()
                        .header("content-type", "application/json")
                        .header("connection", "close")
                        .body(http_body_util::Full::new(hyper::body::Bytes::from(body)))
                        .unwrap(),
                )
            }
        });
        let connection = hyper::server::conn::http1::Builder::new()
            .serve_connection(hyper_util::rt::TokioIo::new(stream), service);
        let _result = tokio::time::timeout(std::time::Duration::from_secs(10), connection).await;
    });
    let config = labby_auth::config::AuthConfig {
        mode: labby_auth::config::AuthMode::OAuth,
        inbound_provider: Some(labby_auth::config::InboundProviderKind::Authelia),
        authelia: Some(labby_auth::config::AutheliaConfig {
            issuer_url: issuer.parse().unwrap(),
            client_id: "client".into(),
            client_secret: "fixture-secret".into(),
            trusted_private_origin: Some(
                labby_auth::config::TrustedIssuerOrigin::new(origin.parse().unwrap()).unwrap(),
            ),
            ca_certificate_path: Some(ca_path),
        }),
        public_url: Some("https://lab.example.com".parse().unwrap()),
        sqlite_path: directory.join("auth.db"),
        key_path: directory.join("auth-key.pem"),
        admin_emails: vec!["owner@example.com".into()],
        token_encryption_key: Some(
            labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"11".repeat(32)).unwrap(),
        ),
        ..Default::default()
    };
    let state = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        labby_auth::state::AuthState::new(config),
    )
    .await
    .unwrap()
    .unwrap();
    (state, server)
}

#[tokio::test]
async fn configured_authelia_cookie_session_projects_same_identity_as_protected_route() {
    for path in ["", "/oidc/tenant"] {
        let directory = tempfile::tempdir().unwrap();
        let (auth, server) = authelia_state(directory.path(), path).await;
        let auth = Arc::new(auth);
        let session = labby_auth::session::create_bound_browser_session(
            &auth,
            "provider-subject".into(),
            Some("owner@example.com".into()),
            auth.inbound_provider_binding(),
        )
        .await
        .unwrap();
        let state = AppState::new()
            .with_auth_config((*auth.config).clone())
            .with_oauth_state((*auth).clone());
        let router = axum::Router::new()
            .route("/auth/session", axum::routing::get(auth_session))
            .route(
                "/protected",
                axum::routing::get(
                    |Extension(identity): Extension<labby_auth::VerifiedIdentity>| async move {
                        assert_eq!(
                            identity.authenticator(),
                            labby_auth::Authenticator::BrowserSession
                        );
                        assert_eq!(identity.transport_credential_issuer(), "browser-session");
                        match identity.principal_link() {
                            labby_auth::PrincipalLink::External { issuer, subject } => {
                                Json(serde_json::json!({ "issuer": issuer, "subject": subject }))
                            }
                            labby_auth::PrincipalLink::LocalCredential { credential_id } => {
                                Json(serde_json::json!({ "credential_id": credential_id }))
                            }
                        }
                    },
                )
                .layer(
                    labby_auth::AuthLayer::from_state(auth.clone()).with_allow_session_cookie(true),
                ),
            )
            .with_state(state.clone());
        let cookie = format!("{}={}", auth.config.session_cookie_name, session.session_id);
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/auth/session")
                    .header(header::COOKIE, &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "configured Authelia session {path}"
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["authenticated"], true);
        assert_eq!(payload["user"]["sub"], "provider-subject");
        assert_eq!(payload["expires_at"], session.expires_at);
        assert_eq!(payload["csrf_token"], session.csrf_token);
        assert_eq!(payload["is_admin"], false);

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/protected")
                    .header(header::COOKIE, &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            payload["issuer"],
            auth.inbound_provider_binding().identity_issuer
        );
        assert_eq!(payload["subject"], "provider-subject");

        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        let identity = fallback_session_identity(&state, &headers)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            identity.authenticator(),
            labby_auth::Authenticator::BrowserSession
        );
        assert_eq!(identity.transport_credential_issuer(), "browser-session");
        assert_eq!(
            identity.principal_link(),
            &labby_auth::PrincipalLink::External {
                issuer: auth.inbound_provider_binding().identity_issuer,
                subject: "provider-subject".into(),
            }
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn cookie_session_rejects_unselected_or_stale_provider_binding() {
    for issuer in [
        Some("https://unknown.example"),
        Some("https://accounts.google.com"),
        None,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (mut auth, server) = authelia_state(directory.path(), "/tenant").await;
        let binding = if let Some(issuer) = issuer {
            let activation = auth
                .store
                .activate_inbound_provider_checked(
                    "authelia",
                    issuer,
                    "other-provider",
                    Some("client"),
                    labby_auth::util::now_unix(),
                )
                .await
                .unwrap();
            auth.inbound_provider_generation = activation.generation;
            labby_auth::types::ProviderBinding {
                identity_issuer: issuer.into(),
                provider_generation: activation.generation,
            }
        } else {
            auth.inbound_provider_binding()
        };
        let session = labby_auth::session::create_bound_browser_session(
            &auth,
            "provider-subject".into(),
            Some("owner@example.com".into()),
            binding,
        )
        .await
        .unwrap();
        if issuer.is_none() {
            auth.inbound_provider_generation += 1;
        }
        let auth = Arc::new(auth);
        let state = AppState::new()
            .with_auth_config((*auth.config).clone())
            .with_oauth_state((*auth).clone());
        let router = axum::Router::new()
            .route("/auth/session", axum::routing::get(auth_session))
            .route(
                "/protected",
                axum::routing::get(|| async { "ok" }).layer(
                    labby_auth::AuthLayer::from_state(auth.clone()).with_allow_session_cookie(true),
                ),
            )
            .with_state(state);
        let cookie = format!("{}={}", auth.config.session_cookie_name, session.session_id);
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/auth/session")
                    .header(header::COOKIE, &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "binding {issuer:?}");
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["authenticated"], false, "binding {issuer:?}");
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/protected")
                    .header(header::COOKIE, &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "binding {issuer:?}"
        );
        server.await.unwrap();
    }
}
