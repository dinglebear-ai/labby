//! Controlled authorization server plus a real Labby Skills resource server.

use super::*;
use axum::{Json, Router, body::Body, routing::get};
use base64::Engine;
use labby::config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};
use labby::oauth::upstream::{encryption::load_key, manager::UpstreamOauthManager};
use labby_auth::sqlite::SqliteStore;
use sha2::{Digest, Sha256};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

#[tokio::test]
async fn skills_oauth_code_exchange_and_refresh_reach_real_server() {
    tokio::time::timeout(Duration::from_mins(1), oauth_roundtrip())
        .await
        .expect("OAuth Skills deadline");
}

async fn oauth_roundtrip() {
    let leaf = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", "skills-e2e-disposable-token")
        .start()
        .await
        .expect("real Skills server");
    let authority = MockServer::start().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let endpoint = format!("{origin}/mcp");
    let issuer = format!("{origin}/");
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "code_challenge_methods_supported": ["S256"]
        })))
        .mount(&authority)
        .await;
    for (grant, token, expiry) in [
        (
            "authorization_code",
            "initial-token-not-accepted-by-leaf",
            10,
        ),
        ("refresh_token", "skills-e2e-disposable-token", 3600),
    ] {
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(format!("grant_type={grant}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": token, "token_type": "Bearer", "expires_in": expiry,
                "refresh_token": "skills-refresh", "scope": "read"
            })))
            .expect(1)
            .mount(&authority)
            .await;
    }

    // Publish resource metadata and forward MCP bytes unchanged to real Labby.
    let metadata = json!({"resource": endpoint, "authorization_servers": [issuer]});
    let leaf_endpoint = format!("{}/mcp", leaf.connection().base_url);
    let authority_origin = authority.uri();
    let router = Router::new()
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(move || {
                let metadata = metadata.clone();
                async move { Json(metadata) }
            }),
        )
        .fallback(move |request: axum::extract::Request| {
            let leaf_endpoint = leaf_endpoint.clone();
            let authority_origin = authority_origin.clone();
            async move {
                let (parts, body) = request.into_parts();
                // Keep OAuth endpoints on the explicitly trusted resource origin.
                let target = if parts.uri.path() == "/mcp" {
                    leaf_endpoint
                } else {
                    format!("{authority_origin}{}", parts.uri)
                };
                let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                let mut headers = parts.headers;
                headers.remove("host");
                headers.remove("connection");
                headers.remove("transfer-encoding");
                headers.remove("content-length");
                let response = reqwest::Client::new()
                    .request(parts.method, target)
                    .headers(headers)
                    .body(bytes)
                    .send()
                    .await
                    .unwrap();
                let mut result = axum::http::Response::builder().status(response.status());
                *result.headers_mut().unwrap() = response.headers().clone();
                for header in ["connection", "transfer-encoding", "content-length"] {
                    result.headers_mut().unwrap().remove(header);
                }
                result
                    .body(Body::from(response.bytes().await.unwrap()))
                    .unwrap()
            }
        });
    let proxy = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let proxy_guard = ProxyGuard(proxy);
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("auth.sqlite"))
        .await
        .unwrap();
    let key = load_key(&base64::engine::general_purpose::STANDARD.encode([0u8; 32])).unwrap();
    let upstream = UpstreamConfig {
        name: "skills-oauth".into(),
        url: Some(endpoint.clone()),
        enabled: true,
        oauth: Some(UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Preregistered {
                client_id: "skills-client".into(),
                client_secret_env: None,
            },
            scopes: Some(vec!["read".into()]),
            credential: Default::default(),
            prefer_client_metadata_document: None,
        }),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: vec![],
        env: Default::default(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: true,
        expose_skills: None,
        code_mode_hint: None,
        imported_from: None,
        priority: 1.0,
    };
    let manager =
        UpstreamOauthManager::new(store, key, upstream, "http://127.0.0.1/callback".into());
    let begin = manager.begin_authorization("skills-user").await.unwrap();
    let url = url::Url::parse(&begin.authorization_url).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["resource"], endpoint);
    // Simulate consent at the controlled AS; Labby performs the real code exchange.
    manager
        .complete_authorization_callback("skills-user", "skills-code", &query["state"])
        .await
        .unwrap();
    let capped = BodyCappedHttpClient::new(reqwest::Client::new(), 1024 * 1024);
    let authenticated = manager
        .build_auth_client_with("skills-user", capped)
        .await
        .unwrap();
    // No static bearer: only the refreshed credential is accepted by the leaf.
    let worker = StreamableHttpClientWorker::new(
        authenticated,
        StreamableHttpClientTransportConfig::with_uri(endpoint),
    );
    let client = ().serve_with_lifecycle(worker, ClientLifecycleMode::Initialize).await.unwrap();
    verify_skill_roundtrip(&client, "skill://labby/using-labby/SKILL.md").await;
    client.cancel().await.unwrap();
    let requests = authority.received_requests().await.unwrap();
    let exchange = requests
        .iter()
        .find(|r| {
            r.url.path() == "/token"
                && String::from_utf8_lossy(&r.body).contains("grant_type=authorization_code")
        })
        .unwrap();
    let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(&exchange.body)
        .into_owned()
        .collect();
    assert_eq!(form["code"], "skills-code");
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(form["code_verifier"].as_bytes())),
        query["code_challenge"]
    );
    authority.verify().await;
    drop(proxy_guard);
    assert!(leaf.finish().await.is_clean());
}

struct ProxyGuard(tokio::task::JoinHandle<()>);
impl Drop for ProxyGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}
