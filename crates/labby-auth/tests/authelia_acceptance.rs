#![cfg(feature = "http-axum")]

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use labby_auth::authelia::AutheliaProvider;
use labby_auth::config::{
    AuthConfig, AuthMode, AutheliaConfig, InboundProviderKind, TrustedIssuerOrigin,
};
use labby_auth::google::AuthorizeUrlRequest;
use labby_auth::oauth_provider::InboundProviderRuntime;
use labby_auth::types::{AuthorizationRequestRow, BrowserLoginStateRow, RegisteredClient};
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use tower::ServiceExt as _;
use url::Url;

#[tokio::test]
#[ignore = "requires `just test-authelia` pinned container harness"]
async fn real_authelia_authorization_code_pkce_flow() {
    assert_eq!(
        std::env::var("LABBY_AUTHELIA_ACCEPTANCE").as_deref(),
        Ok("1")
    );
    let issuer = Url::parse(&std::env::var("LABBY_AUTHELIA_ACCEPTANCE_ISSUER").unwrap()).unwrap();
    let ca_path = std::path::PathBuf::from(std::env::var("LABBY_AUTHELIA_ACCEPTANCE_CA").unwrap());
    let config = AutheliaConfig {
        issuer_url: issuer.clone(),
        client_id: "labby-acceptance".into(),
        client_secret: "labby-authelia-test-secret".into(),
        trusted_private_origin: Some(TrustedIssuerOrigin::new(issuer.join("/").unwrap()).unwrap()),
        ca_certificate_path: Some(ca_path.clone()),
    };
    let redirect = Url::parse("https://labby.localhost/auth/oidc/callback").unwrap();
    let cold_started = std::time::Instant::now();
    let provider = AutheliaProvider::discover(config.clone(), redirect)
        .await
        .unwrap();
    let cold_discovery = cold_started.elapsed();
    let runtime = InboundProviderRuntime::Authelia(Box::new(provider));
    let verifier = "acceptance-verifier-with-more-than-forty-three-characters-000000";
    let challenge =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    let state = "acceptance-state-unique";
    let authorize = runtime
        .authorize_url(&AuthorizeUrlRequest {
            state: state.into(),
            scope: "openid email profile".into(),
            code_challenge: challenge,
            code_challenge_method: "S256".into(),
            offline_access: false,
            force_consent: false,
        })
        .unwrap();

    let ca = reqwest::Certificate::from_pem(&std::fs::read(ca_path).unwrap()).unwrap();
    let browser = reqwest::Client::builder()
        .add_root_certificate(ca)
        .redirect(Policy::none())
        .build()
        .unwrap();
    let initial = browser.get(authorize.clone()).send().await.unwrap();
    let mut cookie = initial
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");
    let login = browser.post(issuer.join("/api/firstfactor").unwrap())
        .header(header::COOKIE, &cookie)
        .json(&serde_json::json!({"username":"tester","password":"test-password","targetURL":authorize.as_str()}))
        .send().await.unwrap();
    assert!(
        login.status().is_success(),
        "first factor failed: {}",
        login.status()
    );
    for value in login.headers().get_all(header::SET_COOKIE) {
        if let Some(pair) = value
            .to_str()
            .ok()
            .and_then(|value| value.split(';').next())
        {
            cookie = pair.to_string();
        }
    }
    let response = browser
        .get(authorize)
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_redirection(),
        "authorize did not redirect: {}",
        response.status()
    );
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let callback = Url::parse(location).unwrap();
    assert_eq!(callback.path(), "/auth/oidc/callback");
    assert_eq!(
        callback
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1,
        state
    );
    let code = callback
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned();
    let exchange_started = std::time::Instant::now();
    let exchange = runtime.exchange_code(&code, verifier, state).await.unwrap();
    let token_exchange = exchange_started.elapsed();
    assert_eq!(exchange.email.as_deref(), Some("tester@example.com"));
    assert_eq!(exchange.email_verified, Some(true));
    assert!(!exchange.access_token.is_empty());
    assert!(exchange.refresh_token.is_none());

    let data = tempfile::tempdir().unwrap();
    let auth_state = labby_auth::state::AuthState::new(AuthConfig {
        mode: AuthMode::OAuth,
        public_url: Some(Url::parse("https://labby.localhost").unwrap()),
        sqlite_path: data.path().join("auth.db"),
        key_path: data.path().join("auth-key.pem"),
        admin_email: "tester@example.com".into(),
        inbound_provider: Some(InboundProviderKind::Authelia),
        authelia: Some(config),
        token_encryption_key: Some(
            labby_auth::at_rest::TokenEncryptionKey::from_encoded(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .unwrap(),
        ),
        allowed_client_redirect_uris: vec!["http://127.0.0.1:7777/callback".into()],
        ..AuthConfig::default()
    })
    .await
    .unwrap();
    auth_state
        .store
        .register_client(RegisteredClient {
            client_id: "acceptance-client".into(),
            redirect_uris: vec!["http://127.0.0.1:7777/callback".into()],
            created_at: 1,
            token_endpoint_auth_method: "none".into(),
            token_endpoint_auth_methods: Vec::new(),
            jwks: None,
            jwks_uri: None,
        })
        .await
        .unwrap();
    let app = labby_auth::routes::router(auth_state.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 19443))));

    // MCP callback -> downstream code -> local token.
    let mcp_state = "real-mcp-state";
    let mcp_verifier = "real-mcp-upstream-verifier-with-sufficient-length-000000";
    auth_state
        .store
        .insert_authorization_request(AuthorizationRequestRow {
            state: mcp_state.into(),
            client_id: "acceptance-client".into(),
            redirect_uri: "http://127.0.0.1:7777/callback".into(),
            client_state: "client-state".into(),
            native_poll_token_hash: None,
            resource: "https://labby.localhost/mcp".into(),
            scope: "lab".into(),
            provider_code_verifier: mcp_verifier.into(),
            code_challenge: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(b"downstream-verifier")),
            code_challenge_method: "S256".into(),
            created_at: unix_now(),
            expires_at: unix_now() + 300,
        })
        .await
        .unwrap();
    let mcp_code = real_code(&runtime, &browser, &cookie, mcp_state, mcp_verifier).await;
    let callback = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/auth/oidc/callback?state={mcp_state}&code={mcp_code}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    let downstream = Url::parse(
        callback
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let downstream_code = downstream
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned();
    let token = app.clone().oneshot(Request::builder().method("POST").uri("/token").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(Body::from(format!("grant_type=authorization_code&code={downstream_code}&client_id=acceptance-client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=downstream-verifier"))).unwrap()).await.unwrap();
    assert_eq!(token.status(), StatusCode::OK);

    // Browser callback -> bound session cookie.
    let browser_state = "real-browser-state";
    let browser_verifier = "real-browser-upstream-verifier-with-sufficient-length-0000";
    auth_state
        .store
        .insert_browser_login_state(BrowserLoginStateRow {
            state: browser_state.into(),
            return_to: "/gateway".into(),
            provider_code_verifier: browser_verifier.into(),
            created_at: unix_now(),
            expires_at: unix_now() + 300,
        })
        .await
        .unwrap();
    let browser_code =
        real_code(&runtime, &browser, &cookie, browser_state, browser_verifier).await;
    // The MCP callback above populated this AuthState generation's JWKS cache.
    // This second callback therefore measures token exchange plus warm verification.
    let warm_verification_started = std::time::Instant::now();
    let callback = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/auth/oidc/callback?state={browser_state}&code={browser_code}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let warm_verification = warm_verification_started.elapsed();
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    assert!(callback.headers().contains_key(header::SET_COOKIE));

    // Native callback -> one poll result.
    let native_state = "real-native-state";
    let native_verifier = "real-native-upstream-verifier-with-sufficient-length-00000";
    let poll_token = "real-native-poll-token";
    auth_state
        .store
        .insert_authorization_request(AuthorizationRequestRow {
            state: native_state.into(),
            client_id: "acceptance-client".into(),
            redirect_uri: "https://labby.localhost/native/callback".into(),
            client_state: "native-client-state".into(),
            native_poll_token_hash: Some(
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(poll_token)),
            ),
            resource: "https://labby.localhost/mcp".into(),
            scope: "lab".into(),
            provider_code_verifier: native_verifier.into(),
            code_challenge: "unused".into(),
            code_challenge_method: "S256".into(),
            created_at: unix_now(),
            expires_at: unix_now() + 300,
        })
        .await
        .unwrap();
    let native_code = real_code(&runtime, &browser, &cookie, native_state, native_verifier).await;
    let callback = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/auth/oidc/callback?state={native_state}&code={native_code}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::OK);
    let poll = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/native/poll")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"poll_token":"{poll_token}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(poll.status(), StatusCode::OK);
    assert!(cold_discovery < std::time::Duration::from_secs(10));
    assert!(token_exchange < std::time::Duration::from_secs(10));
    assert!(warm_verification < std::time::Duration::from_secs(10));
    eprintln!(
        "authelia acceptance timings: cold_discovery_ms={} token_exchange_and_cold_jwks_ms={} warm_verification_ms={}",
        cold_discovery.as_millis(),
        token_exchange.as_millis(),
        warm_verification.as_millis()
    );
}

async fn real_code(
    runtime: &InboundProviderRuntime,
    browser: &reqwest::Client,
    cookie: &str,
    state: &str,
    verifier: &str,
) -> String {
    let challenge =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    let authorize = runtime
        .authorize_url(&AuthorizeUrlRequest {
            state: state.into(),
            scope: "openid email profile".into(),
            code_challenge: challenge,
            code_challenge_method: "S256".into(),
            offline_access: false,
            force_consent: false,
        })
        .unwrap();
    let response = browser
        .get(authorize)
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_redirection());
    let callback = Url::parse(
        response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap(),
    )
    .unwrap();
    callback
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
