use super::*;

fn meta() -> AuthServerMetadata {
    AuthServerMetadata {
        authorization_endpoint: "https://axon.example.com/authorize".to_string(),
        token_endpoint: "https://axon.example.com/token".to_string(),
        revocation_endpoint: Some("https://axon.example.com/revoke".to_string()),
        registration_endpoint: Some("https://axon.example.com/register".to_string()),
        native_callback_endpoint: None,
        native_poll_endpoint_v2: None,
        native_authorization_start_media_type: None,
    }
}

#[test]
fn discovery_url_appends_well_known_path() {
    assert_eq!(
        discovery_url("https://axon.example.com/"),
        "https://axon.example.com/.well-known/oauth-authorization-server"
    );
}

#[test]
fn metadata_deserializes_ignoring_extra_fields_and_optional_registration() {
    let json = r#"{
        "issuer": "https://axon.example.com",
        "authorization_endpoint": "https://axon.example.com/authorize",
        "token_endpoint": "https://axon.example.com/token",
        "revocation_endpoint": "https://axon.example.com/revoke",
        "registration_endpoint": "https://axon.example.com/register",
        "jwks_uri": "https://axon.example.com/jwks",
        "response_types_supported": ["code"]
    }"#;
    let parsed: AuthServerMetadata = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.token_endpoint, "https://axon.example.com/token");
    assert_eq!(
        parsed.revocation_endpoint.as_deref(),
        Some("https://axon.example.com/revoke")
    );
    assert_eq!(
        parsed.registration_endpoint.as_deref(),
        Some("https://axon.example.com/register")
    );

    // DCR-disabled server omits registration_endpoint → None, not a parse error.
    let no_dcr = r#"{
        "issuer": "https://axon.example.com",
        "authorization_endpoint": "https://axon.example.com/authorize",
        "token_endpoint": "https://axon.example.com/token"
    }"#;
    let parsed: AuthServerMetadata = serde_json::from_str(no_dcr).unwrap();
    assert!(parsed.registration_endpoint.is_none());
    assert!(parsed.revocation_endpoint.is_none());
}

#[test]
fn token_response_deserializes_with_and_without_refresh() {
    let with = r#"{"access_token":"a","token_type":"Bearer","expires_in":3600,"refresh_token":"r","scope":"axon:read axon:write"}"#;
    let parsed: TokenResponse = serde_json::from_str(with).unwrap();
    assert_eq!(parsed.refresh_token.as_ref().map(|s| s.expose()), Some("r"));
    assert_eq!(parsed.expires_in, 3600);

    let without = r#"{"access_token":"a","token_type":"Bearer","expires_in":3600,"scope":"axon:read axon:write"}"#;
    let parsed: TokenResponse = serde_json::from_str(without).unwrap();
    assert!(parsed.refresh_token.is_none());
}

#[test]
fn token_response_debug_redacts_tokens() {
    let parsed: TokenResponse = serde_json::from_str(
        r#"{"access_token":"secret-a","token_type":"Bearer","expires_in":3600,"refresh_token":"secret-r","scope":"axon:read"}"#,
    )
    .unwrap();
    let rendered = format!("{parsed:?}");
    assert!(!rendered.contains("secret-a"));
    assert!(!rendered.contains("secret-r"));
}

#[test]
fn shared_oauth_error_fixture_matches_provider_contract_without_exposing_bodies() {
    let error: labby_oauth_wire::OAuthErrorResponse = serde_json::from_str(
        r#"{"error":"invalid_grant","error_description":"unknown refresh_token"}"#,
    )
    .unwrap();
    assert_eq!(error.error, "invalid_grant");
    assert_eq!(error.error_description, "unknown refresh_token");
    assert!(error.error_uri.is_none());
}

#[test]
fn require_secure_url_allows_https_and_loopback_http_only() {
    assert!(require_secure_url("https://axon.example.com/token").is_ok());
    assert!(require_secure_url("http://127.0.0.1:8001/token").is_ok());
    assert!(require_secure_url("http://[::1]:8001/token").is_ok());
    assert!(require_secure_url("http://localhost:8001/token").is_ok());
    assert!(require_secure_url("http://axon.example.com/token").is_err()); // cleartext non-loopback
    assert!(require_secure_url("file:///etc/passwd").is_err());
    assert!(require_secure_url("not a url").is_err());
}

#[test]
fn authorize_url_carries_all_required_pkce_params() {
    let url = build_authorize_url(
        &meta(),
        "client-123",
        "http://127.0.0.1:7777/callback",
        "axon:read axon:write",
        "state-xyz",
        "challenge-abc",
    )
    .unwrap();
    assert!(url.starts_with("https://axon.example.com/authorize?"));
    assert!(url.contains("response_type=code"));
    assert!(url.contains("client_id=client-123"));
    assert!(url.contains("code_challenge=challenge-abc"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("state=state-xyz"));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A7777%2Fcallback"));
    assert!(url.contains("scope=axon%3Aread+axon%3Awrite"));
}

#[test]
fn registration_body_wraps_single_redirect_uri() {
    assert_eq!(
        registration_body("http://127.0.0.1:7777/callback"),
        serde_json::json!({ "redirect_uris": ["http://127.0.0.1:7777/callback"] })
    );
}

#[test]
fn token_forms_have_required_fields() {
    let auth = authorization_code_form(
        "code-1",
        "client-123",
        "http://127.0.0.1:7777/callback",
        "verifier-1",
    );
    assert!(auth.contains(&("grant_type", "authorization_code".to_string())));
    assert!(auth.contains(&("code", "code-1".to_string())));
    assert!(auth.contains(&("client_id", "client-123".to_string())));
    assert!(auth.contains(&("redirect_uri", "http://127.0.0.1:7777/callback".to_string())));
    assert!(auth.contains(&("code_verifier", "verifier-1".to_string())));

    let refresh = refresh_form("client-123", "refresh-1");
    assert!(refresh.contains(&("grant_type", "refresh_token".to_string())));
    assert!(refresh.contains(&("refresh_token", "refresh-1".to_string())));
    assert!(refresh.contains(&("client_id", "client-123".to_string())));
}

#[test]
fn revocation_form_identifies_refresh_grant_without_exposing_it_elsewhere() {
    assert_eq!(
        revocation_form("client-123", "refresh-1"),
        vec![
            ("token", "refresh-1".to_string()),
            ("token_type_hint", "refresh_token".to_string()),
            ("client_id", "client-123".to_string()),
        ]
    );
}

#[test]
fn grant_rejection_only_for_definitive_codes_not_transient_4xx() {
    use reqwest::StatusCode;
    // Definitive grant rejections → clear the session.
    assert!(is_grant_rejection(StatusCode::BAD_REQUEST));
    assert!(is_grant_rejection(StatusCode::UNAUTHORIZED));
    assert!(is_grant_rejection(StatusCode::FORBIDDEN));
    assert!(is_grant_rejection(StatusCode::GONE));
    // Transient — must NOT wipe a valid OAuth session.
    assert!(!is_grant_rejection(StatusCode::TOO_MANY_REQUESTS)); // 429
    assert!(!is_grant_rejection(StatusCode::REQUEST_TIMEOUT)); // 408
    assert!(!is_grant_rejection(StatusCode::INTERNAL_SERVER_ERROR)); // 500
    assert!(!is_grant_rejection(StatusCode::SERVICE_UNAVAILABLE)); // 503
}

async fn serve_once(response: String) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 16 * 1024];
        let read = socket.read(&mut request).await.unwrap();
        request.truncate(read);
        socket.write_all(response.as_bytes()).await.unwrap();
        request
    });
    (format!("http://{address}"), task)
}

async fn capture_if_connected() -> (String, tokio::task::JoinHandle<bool>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_millis(250), listener.accept())
            .await
            .is_ok()
    });
    (format!("http://localhost:{}", address.port()), task)
}

async fn serve_tls_redirect_once(
    response: String,
) -> (
    String,
    reqwest::Certificate,
    tokio::task::JoinHandle<Vec<u8>>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::rustls::pki_types::PrivateKeyDer;

    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(["localhost".to_string()]).unwrap();
    let root = reqwest::Certificate::from_der(cert.der()).unwrap();
    let tls = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(signing_key.serialize_der().into()),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = acceptor.accept(socket).await.unwrap();
        let mut request = vec![0; 16 * 1024];
        let read = socket.read(&mut request).await.unwrap();
        request.truncate(read);
        socket.write_all(response.as_bytes()).await.unwrap();
        request
    });
    (format!("https://localhost:{}", address.port()), root, task)
}

#[derive(Clone, Copy, Debug)]
enum RedirectOperation {
    Registration,
    CodeExchange,
    Refresh,
    Revocation,
    NativePoll,
}

async fn assert_operation_does_not_follow_redirect(operation: RedirectOperation, status: &str) {
    const SENTINEL: &str = "oauth-sentinel-must-not-leak";
    let (capture_url, capture) = capture_if_connected().await;
    let redirect = format!(
        "HTTP/1.1 {status}\r\nLocation: {capture_url}/downgrade-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let (origin, origin_request) = serve_once(redirect).await;
    let client = oauth_client().unwrap();
    let endpoint = format!("{origin}/oauth-operation");

    let error = match operation {
        RedirectOperation::Registration => register_client(&client, &endpoint, SENTINEL)
            .await
            .unwrap_err(),
        RedirectOperation::CodeExchange => exchange_code(
            &client,
            &endpoint,
            SENTINEL,
            "client-id",
            "http://127.0.0.1/callback",
            SENTINEL,
        )
        .await
        .unwrap_err(),
        RedirectOperation::Refresh => {
            refresh_access_token(&client, &endpoint, "client-id", SENTINEL)
                .await
                .unwrap_err()
                .message
        }
        RedirectOperation::Revocation => {
            revoke_refresh_token(&client, &endpoint, "client-id", SENTINEL)
                .await
                .unwrap_err()
        }
        RedirectOperation::NativePoll => poll_native_code(
            &client,
            &endpoint,
            SENTINEL,
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap_err(),
    };

    let request = origin_request.await.unwrap();
    assert!(
        String::from_utf8_lossy(&request).contains(SENTINEL),
        "{operation:?} did not send its sentinel to the configured origin"
    );
    assert!(
        !capture.await.unwrap(),
        "{operation:?} replayed a request to the redirect target"
    );
    assert!(
        error.contains(status.split_once(' ').unwrap().0),
        "unexpected {operation:?} error: {error}"
    );
    assert!(
        !error.contains(SENTINEL),
        "{operation:?} error disclosed its sentinel: {error}"
    );
}

#[tokio::test]
async fn production_oauth_client_never_replays_any_oauth_secret_across_307_or_308() {
    for status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
        for operation in [
            RedirectOperation::Registration,
            RedirectOperation::CodeExchange,
            RedirectOperation::Refresh,
            RedirectOperation::Revocation,
            RedirectOperation::NativePoll,
        ] {
            assert_operation_does_not_follow_redirect(operation, status).await;
        }
    }
}

#[tokio::test]
async fn redirect_policy_is_operation_independent_for_cross_origin_and_cleartext_targets() {
    const SENTINEL: &str = "refresh-sentinel-must-not-leak";
    for status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
        let (capture_url, capture) = capture_if_connected().await;
        let redirect = format!(
            "HTTP/1.1 {status}\r\nLocation: {capture_url}/downgrade-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let (origin, origin_request) = serve_once(redirect).await;
        let client = oauth_client().unwrap();

        let error =
            refresh_access_token(&client, &format!("{origin}/token"), "client-id", SENTINEL)
                .await
                .unwrap_err();

        let request = origin_request.await.unwrap();
        assert!(String::from_utf8_lossy(&request).contains(SENTINEL));
        assert!(
            !capture.await.unwrap(),
            "redirect target received a request"
        );
        assert!(!error.rejected);
        assert!(error.message.contains(status.split_once(' ').unwrap().0));
        assert!(!error.message.contains(SENTINEL));
    }
}

#[tokio::test]
async fn production_redirect_policy_refuses_https_to_http_downgrades() {
    const SENTINEL: &str = "downgrade-sentinel-must-not-leak";
    for status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
        let (capture_url, capture) = capture_if_connected().await;
        let redirect = format!(
            "HTTP/1.1 {status}\r\nLocation: {capture_url}/cleartext-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let (origin, root, origin_request) = serve_tls_redirect_once(redirect).await;
        let client = oauth_client_builder()
            .add_root_certificate(root)
            .build()
            .unwrap();

        let error =
            refresh_access_token(&client, &format!("{origin}/token"), "client-id", SENTINEL)
                .await
                .unwrap_err();

        let request = origin_request.await.unwrap();
        assert!(String::from_utf8_lossy(&request).contains(SENTINEL));
        assert!(
            !capture.await.unwrap(),
            "HTTP downgrade target received a request"
        );
        assert!(!error.rejected);
        assert!(error.message.contains(status.split_once(' ').unwrap().0));
        assert!(!error.message.contains(SENTINEL));
    }
}

#[tokio::test]
async fn token_response_failures_are_bounded_redacted_and_transient() {
    const SENTINEL: &str = "refresh-sentinel-must-not-leak";
    let cases = [
        (
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{"
                .to_string(),
            "invalid response",
        ),
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_OAUTH_JSON_BYTES + 1
            ),
            "invalid response",
        ),
        (
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
            "429",
        ),
        (
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
            "503",
        ),
    ];

    for (response, expected) in cases {
        let (origin, request) = serve_once(response).await;
        let error = refresh_access_token(
            &oauth_client().unwrap(),
            &format!("{origin}/token"),
            "client-id",
            SENTINEL,
        )
        .await
        .unwrap_err();
        request.await.unwrap();
        assert!(!error.rejected);
        assert!(error.message.contains(expected), "{}", error.message);
        assert!(!error.message.contains(SENTINEL));
    }
}

#[tokio::test]
async fn native_poll_timeout_is_bounded_and_does_not_echo_credential() {
    use tokio::io::AsyncReadExt;

    const SENTINEL: &str = "poll-sentinel-must-not-leak";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = socket.read(&mut request).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    });

    let error = poll_native_code(
        &oauth_client().unwrap(),
        &format!("http://{address}/poll"),
        SENTINEL,
        std::time::Duration::from_millis(40),
    )
    .await
    .unwrap_err();
    assert!(error.contains("timed out"));
    assert!(!error.contains(SENTINEL));
    server.abort();
}
