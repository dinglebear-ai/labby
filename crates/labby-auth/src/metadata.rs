use axum::{Json, extract::State};

use crate::state::AuthState;
use crate::types::{AuthorizationServerMetadata, ProtectedResourceMetadata};

pub async fn authorization_server_metadata(
    State(state): State<AuthState>,
) -> Json<AuthorizationServerMetadata> {
    let base = public_base_url(&state);
    let has_machine_clients = !state.config.machine_clients.is_empty();
    let has_enterprise_issuers = !state.config.enterprise_issuers.is_empty();
    let mut grant_types_supported = vec![
        "authorization_code".to_string(),
        "refresh_token".to_string(),
    ];
    if has_machine_clients {
        grant_types_supported.push("client_credentials".to_string());
    }
    if has_enterprise_issuers {
        grant_types_supported.push("urn:ietf:params:oauth:grant-type:jwt-bearer".to_string());
    }
    // `private_key_jwt` is unconditional: any CIMD client may declare it in its
    // metadata document, and `/token` honours that declaration whether or not
    // preregistered machine clients exist. Only `client_secret_basic` depends
    // on machine clients.
    let mut token_auth_methods = vec!["none".to_string(), "private_key_jwt".to_string()];
    if has_machine_clients {
        token_auth_methods.push("client_secret_basic".to_string());
    }
    Json(AuthorizationServerMetadata {
        issuer: base.clone(),
        authorization_endpoint: format!("{base}/authorize"),
        token_endpoint: format!("{base}/token"),
        revocation_endpoint: format!("{base}/revoke"),
        registration_endpoint: state
            .config
            .enable_dynamic_registration
            .then(|| format!("{base}/register")),
        native_callback_endpoint: Some(native_callback_endpoint(&state)),
        // Keep the legacy field absent so pre-v2 Palette releases safely fall
        // back to loopback rather than polling caller-controlled `state`.
        native_poll_endpoint: None,
        native_poll_endpoint_v2: Some(native_poll_endpoint(&state)),
        native_authorization_start_media_type: Some(
            "application/vnd.labby.native-oauth-start+json".to_string(),
        ),
        jwks_uri: format!("{base}/jwks"),
        response_types_supported: vec!["code".to_string()],
        scopes_supported: state.config.scopes_supported.clone(),
        grant_types_supported,
        code_challenge_methods_supported: vec!["S256".to_string()],
        token_endpoint_auth_methods_supported: token_auth_methods,
        // RFC 8414 requires this whenever `private_key_jwt` is advertised, and
        // it now always is. The list mirrors `ensure_allowed_algorithm` in
        // `token.rs`.
        token_endpoint_auth_signing_alg_values_supported: vec![
            "EdDSA".to_string(),
            "RS256".to_string(),
            "ES256".to_string(),
        ],
        // Codex 0.144.3 drops `iss` from its local callback before handing the
        // response to rmcp (openai/codex#34684). Operators may explicitly
        // disable RFC 9207 response-issuer binding until that client defect is
        // fixed; standards-compliant metadata remains the default.
        authorization_response_iss_parameter_supported: true,
        client_id_metadata_document_supported: true,
        authorization_grant_profiles_supported: if has_enterprise_issuers {
            vec!["urn:ietf:params:oauth:grant-profile:id-jag".to_string()]
        } else {
            Vec::new()
        },
    })
}

pub async fn protected_resource_metadata(
    State(state): State<AuthState>,
) -> Json<ProtectedResourceMetadata> {
    let base = public_base_url(&state);
    Json(ProtectedResourceMetadata {
        resource: canonical_resource_url(&state),
        authorization_servers: vec![base],
        scopes_supported: state.config.scopes_supported.clone(),
        bearer_methods_supported: vec!["header".to_string()],
    })
}

pub async fn jwks(State(state): State<AuthState>) -> Json<crate::jwt::JwksDocument> {
    Json(state.signing_keys.jwks().clone())
}

pub(crate) fn public_base_url(state: &AuthState) -> String {
    // Panicking on absent public_url is intentional: this is a programmer/operator
    // error (misconfigured server). Callers are not expected to handle a missing URL.
    #[allow(clippy::expect_used)]
    state
        .config
        .public_url
        .as_ref()
        .expect("oauth state must have public_url configured")
        .as_str()
        .trim_end_matches('/')
        .to_string()
}

pub(crate) fn native_callback_endpoint(state: &AuthState) -> String {
    format!("{}/native/callback", public_base_url(state))
}

pub(crate) fn native_poll_endpoint(state: &AuthState) -> String {
    format!("{}/native/poll", public_base_url(state))
}

pub fn canonical_resource_url(state: &AuthState) -> String {
    let base = public_base_url(state);
    let suffix = state.config.resource_path.trim_start_matches('/');
    if suffix.is_empty() {
        base
    } else {
        format!("{base}/{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;

    use crate::routes::router;

    use super::super::authorize::tests::{
        test_auth_config, test_auth_state, test_auth_state_with_config,
    };

    #[tokio::test]
    async fn authorization_server_metadata_exposes_lab_endpoints() {
        let app = router(test_auth_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["issuer"], "https://lab.example.com");
        assert_eq!(
            json["authorization_endpoint"],
            "https://lab.example.com/authorize"
        );
        assert_eq!(json["token_endpoint"], "https://lab.example.com/token");
        assert_eq!(
            json["code_challenge_methods_supported"],
            serde_json::json!(["S256"])
        );
        assert_eq!(
            json["authorization_response_iss_parameter_supported"], true,
            "RFC 9207 issuer binding must remain the default"
        );
        assert_eq!(json["client_id_metadata_document_supported"], true);
        assert!(json.get("native_poll_endpoint").is_none());
        assert_eq!(
            json["native_poll_endpoint_v2"],
            "https://lab.example.com/native/poll"
        );
        assert_eq!(
            json["native_authorization_start_media_type"],
            "application/vnd.labby.native-oauth-start+json"
        );
        assert_eq!(
            json["revocation_endpoint"],
            "https://lab.example.com/revoke"
        );
    }

    #[tokio::test]
    async fn authorization_server_metadata_omits_unmounted_registration_endpoint() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = false;
        let app = crate::routes::bearer_only_router(test_auth_state_with_config(config).await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("registration_endpoint").is_none());
    }

    #[tokio::test]
    async fn authorization_server_metadata_advertises_private_key_jwt_without_machine_clients() {
        // CIMD clients declare `private_key_jwt` in their own metadata document
        // and `/token` honours it, so the advertisement cannot be conditional on
        // preregistered machine clients.
        let app = router(test_auth_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let methods = json["token_endpoint_auth_methods_supported"]
            .as_array()
            .unwrap();
        assert!(methods.contains(&serde_json::json!("none")));
        assert!(methods.contains(&serde_json::json!("private_key_jwt")));
        assert!(
            !methods.contains(&serde_json::json!("client_secret_basic")),
            "client_secret_basic still requires configured machine clients"
        );
        assert!(
            json["token_endpoint_auth_signing_alg_values_supported"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("RS256")),
            "RFC 8414 requires signing algs whenever private_key_jwt is advertised"
        );
    }

    #[tokio::test]
    async fn authorization_server_metadata_keeps_issuer_binding_in_compatibility_mode() {
        use crate::authorize::tests::{test_auth_config, test_auth_state_with_config};

        let mut config = test_auth_config();
        config.codex_issuer_compatibility = true;

        let app = router(test_auth_state_with_config(config).await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authorization_response_iss_parameter_supported"], true);
    }

    #[tokio::test]
    async fn protected_resource_metadata_uses_canonical_mcp_resource_uri() {
        let app = router(test_auth_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["resource"], "https://lab.example.com/mcp");
        assert_eq!(
            json["authorization_servers"],
            serde_json::json!(["https://lab.example.com"])
        );
        assert_eq!(
            json["scopes_supported"],
            serde_json::json!(["lab:read", "lab", "lab:admin"]),
            "root discovery must lead with the least-privilege read scope"
        );
        assert!(json["resource"].as_str().unwrap().starts_with("https://"));
    }

    #[tokio::test]
    async fn protected_resource_metadata_advertises_configured_scopes_and_resource_path() {
        use crate::authorize::tests::test_auth_state_with_config;
        use crate::config::AuthConfig;

        // Synthesize a config that overrides scopes_supported and resource_path,
        // matching how syslog-mcp will eventually configure labby-auth.
        let dir = tempfile::tempdir().unwrap();
        let config = AuthConfig {
            mode: crate::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://syslog.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth.pem"),
            admin_email: "admin@example.com".into(),
            google: crate::config::GoogleConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
                callback_url: None,
                callback_path: "/auth/google/callback".into(),
                scopes: vec!["openid".into(), "email".into()],
            },
            token_encryption_key: Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "metadata-test-provider-key",
            )),
            scopes_supported: vec!["syslog:read".to_string(), "syslog:admin".to_string()],
            resource_path: "/syslog/mcp".to_string(),
            ..AuthConfig::default()
        };
        let state = test_auth_state_with_config(config).await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-protected-resource")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["resource"], "https://syslog.example.com/syslog/mcp");
        assert_eq!(
            json["scopes_supported"],
            serde_json::json!(["syslog:read", "syslog:admin"])
        );
    }
}
