use super::*;
use crate::dispatch::agent_messages::proof::MessageKey;
use crate::dispatch::codemode_notices::NoticeStore;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};
use tower::ServiceExt as _;

const FIRST_KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECOND_KEY: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn fixture(root: &Path, token: &str, enabled: bool) -> Router {
    router(root, Some(token.to_owned()), None, enabled)
}

fn router(
    root: &Path,
    token: Option<String>,
    oauth: Option<labby_auth::state::AuthState>,
    enabled: bool,
) -> Router {
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            root.join("gateway.toml"),
            Default::default(),
        ),
    );
    let mut state = crate::api::AppState::new().with_gateway_manager(manager);
    state.agent_notifications = NoticeStore::open_at(root).unwrap();
    if enabled {
        state.agent_messages = Some(Arc::new(HttpMessageRuntime::new(
            MessageKey::from_test_bytes([8; 32]),
            "1".repeat(64),
        )));
    }
    crate::cli::serve::build_http_router(
        state,
        token,
        oauth,
        &crate::config::McpPreferences::default(),
        &[],
        crate::mcp::peers::PeerNotifier::default(),
        true,
        false,
    )
    .unwrap()
}

async fn rpc(
    router: &Router,
    method: &str,
    mut params: Value,
    token: &str,
    keys: &[&str],
) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"fixture","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{},
        "openai/session":"copied-caller-controlled-marker",
    });
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {token}"))
        .header("mcp-method", method)
        .header("mcp-protocol-version", "2026-07-28");
    if let Some(name) = params["name"].as_str() {
        request = request.header("mcp-name", name);
    }
    for key in keys {
        request = request.header("labby-session-key", *key);
    }
    let body =
        Body::from(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string());
    let reply = router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let bytes = to_bytes(reply.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn call(router: &Router, token: &str, keys: &[&str], action: &str, params: Value) -> Value {
    rpc(
        router,
        "tools/call",
        json!({"name":"messages","arguments":{"action":action,"params":params}}),
        token,
        keys,
    )
    .await
}

fn ok(reply: &Value) -> &Value {
    assert!(
        reply["error"].is_null() && reply["result"]["isError"] != true,
        "{reply}"
    );
    assert!(reply["result"]["structuredContent"].is_object(), "{reply}");
    &reply["result"]["structuredContent"]
}

#[tokio::test]
async fn message_tool_requires_exact_session_proof() {
    let temp = tempfile::tempdir().unwrap();
    let token = "synthetic-message-token";
    let router = fixture(temp.path(), token, true);
    for keys in [&[][..], &["not hex"][..], &[FIRST_KEY, SECOND_KEY][..]] {
        let reply = call(
            &router,
            token,
            keys,
            "register",
            json!({"label":"not registered"}),
        )
        .await;
        assert!(
            reply["result"]["structuredContent"]["session_id"].is_null(),
            "{reply}"
        );
    }
    let first = call(
        &router,
        token,
        &[FIRST_KEY],
        "register",
        json!({"label":"first"}),
    )
    .await;
    let second = call(
        &router,
        token,
        &[SECOND_KEY],
        "register",
        json!({"label":"second"}),
    )
    .await;
    let first_id = &ok(&first)["session_id"];
    let second_id = &ok(&second)["session_id"];
    assert_ne!(first_id, second_id);
    let forged = call(
        &router, token, &[FIRST_KEY], "send",
        json!({"to":{"kind":"session","session_id":second_id},"body":"forgery","idempotency_key":"forgery","sender_session_id":second_id}),
    ).await;
    assert_eq!(forged["error"]["data"]["kind"], "invalid_param");
    let body = "\\".repeat(16_384);
    let send = call(&router, token, &[FIRST_KEY], "send",
        json!({"to":{"kind":"session","session_id":second_id},"body":body,"idempotency_key":"proof"})).await;
    let id = &ok(&send)["message_id"];
    let sibling = call(
        &router,
        token,
        &[FIRST_KEY],
        "read",
        json!({"message_id":id}),
    )
    .await;
    assert!(sibling["result"]["structuredContent"]["body"].is_null());
    let read = call(
        &router,
        token,
        &[SECOND_KEY],
        "read",
        json!({"message_id":id}),
    )
    .await;
    assert_eq!(ok(&read)["body"], body);
    assert!(serde_json::to_vec(&read["result"]).unwrap().len() <= 128 * 1024);
}

#[tokio::test]
async fn verified_static_rotation_changes_the_owner_and_binding() {
    let temp = tempfile::tempdir().unwrap();
    let first = fixture(temp.path(), "fixture-old", true);
    let old = call(
        &first,
        "fixture-old",
        &[FIRST_KEY],
        "register",
        json!({"label":"old"}),
    )
    .await;
    let old_id = &ok(&old)["session_id"];
    drop(first);
    let second = fixture(temp.path(), "fixture-new", true);
    let new = call(
        &second,
        "fixture-new",
        &[FIRST_KEY],
        "register",
        json!({"label":"new"}),
    )
    .await;
    assert_ne!(&ok(&new)["session_id"], old_id);
    let dir = call(&second, "fixture-new", &[FIRST_KEY], "sessions", json!({})).await;
    assert_eq!(ok(&dir)["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(ok(&dir)["sessions"][0]["label"], "new");
}

#[tokio::test]
async fn local_message_bridge_requires_authenticated_installation() {
    use crate::dispatch::{
        agent_messages::proof::{BRIDGE_PROOF_PATH, BridgeProof, BridgeProofRequest},
        codemode_notices::now_ms,
    };
    use axum::{extract::ConnectInfo, http::StatusCode};
    use std::net::SocketAddr;

    async fn prove(
        router: &Router,
        challenge: &BridgeProofRequest,
        token: &str,
        peer: Option<SocketAddr>,
    ) -> (StatusCode, Option<BridgeProof>) {
        let mut request = Request::builder()
            .method("POST")
            .uri(BRIDGE_PROOF_PATH)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .header("forwarded", "for=127.0.0.1");
        if let Some(peer) = peer {
            request = request.extension(ConnectInfo(peer));
        }
        let response = router
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(challenge).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2048).await.unwrap();
        (status, serde_json::from_slice(&bytes).ok())
    }

    let temp = tempfile::tempdir().unwrap();
    let token = "synthetic-proof-bearer";
    let key = MessageKey::from_test_bytes([8; 32]);
    let installation = "1".repeat(64);
    let first = fixture(temp.path(), token, true);
    let challenge = BridgeProofRequest::new(&key, token, &installation, now_ms()).unwrap();
    let loopback = Some("127.0.0.1:12345".parse().unwrap());
    let remote = Some("192.0.2.42:12345".parse().unwrap());

    assert_eq!(
        prove(&first, &challenge, token, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        prove(&first, &challenge, token, remote).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        prove(&first, &challenge, "wrong", loopback).await.0,
        StatusCode::UNAUTHORIZED
    );

    let other = BridgeProofRequest::new(&key, token, &"2".repeat(64), now_ms()).unwrap();
    assert_eq!(
        prove(&first, &other, token, loopback).await.0,
        StatusCode::FORBIDDEN
    );
    let forgery = BridgeProofRequest::new(
        &MessageKey::from_test_bytes([7; 32]),
        token,
        &installation,
        now_ms(),
    )
    .unwrap();
    assert_eq!(
        prove(&first, &forgery, token, loopback).await.0,
        StatusCode::FORBIDDEN
    );

    let (status, result) = prove(&first, &challenge, token, loopback).await;
    assert_eq!(status, StatusCode::OK);
    let result = result.unwrap();
    result.verify(&key, &challenge, token, now_ms()).unwrap();
    assert_eq!(result.owner_key, key.static_owner(token));

    let restarted = fixture(temp.path(), token, true);
    let after = prove(&restarted, &challenge, token, loopback)
        .await
        .1
        .unwrap();
    after.verify(&key, &challenge, token, now_ms()).unwrap();
    assert_eq!(after.owner_key, result.owner_key);
    assert_ne!(after.daemon_epoch, result.daemon_epoch);

    let disabled = fixture(temp.path(), token, false);
    assert_eq!(
        prove(&disabled, &challenge, token, loopback).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn verified_oauth_owner_requires_current_consent_and_binds_client() {
    use labby_auth::{
        at_rest::TokenEncryptionKey,
        config::{AuthConfig, AuthMode, GoogleConfig},
        jwt::AccessClaims,
        state::AuthState,
        types::{RefreshTokenRow, RegisteredClient},
    };
    let temp = tempfile::tempdir().unwrap();
    let issuer = "https://lab.example.com";
    let resource = "https://lab.example.com/mcp";
    let now = crate::dispatch::codemode_notices::now_ms() / 1_000;
    let config = AuthConfig {
        mode: AuthMode::OAuth,
        public_url: Some(issuer.parse().unwrap()),
        sqlite_path: temp.path().join("oauth.db"),
        key_path: temp.path().join("jwt.pem"),
        bootstrap_secret: Some("fixture-bootstrap".into()),
        google: GoogleConfig {
            client_id: "fixture-provider".into(),
            client_secret: "fixture-secret".into(),
            callback_url: None,
            callback_path: "/auth/google/callback".into(),
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
        },
        token_encryption_key: Some(
            TokenEncryptionKey::from_encoded(
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            )
            .unwrap(),
        ),
        ..AuthConfig::default()
    };
    let auth = AuthState::new(config).await.unwrap();
    let provider = auth.inbound_provider_binding();
    let store = auth.store.clone();
    let mut tokens = Vec::new();
    for client in ["fixture-first", "fixture-second"] {
        store
            .register_client(RegisteredClient {
                client_id: client.into(),
                redirect_uris: vec!["https://example.com/callback".into()],
                created_at: now,
                token_endpoint_auth_method: "none".into(),
                token_endpoint_auth_methods: vec![],
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        store
            .upsert_bound_refresh_token(
                RefreshTokenRow {
                    refresh_token: format!("synthetic-refresh-{client}"),
                    client_id: client.into(),
                    subject: "fixture-subject".into(),
                    resource: resource.into(),
                    scope: "lab".into(),
                    provider_refresh_token: None,
                    created_at: now,
                    expires_at: now + 3600,
                },
                provider.clone(),
            )
            .await
            .unwrap();
        tokens.push(
            auth.signing_keys
                .issue_access_token(&AccessClaims {
                    iss: issuer.into(),
                    sub: "fixture-subject".into(),
                    aud: resource.into(),
                    exp: (now + 3600) as usize,
                    iat: now as usize,
                    jti: format!("synthetic-jti-{client}"),
                    scope: "lab".into(),
                    azp: client.into(),
                    identity_issuer: Some(provider.identity_issuer.clone()),
                    ..AccessClaims::default()
                })
                .unwrap(),
        );
    }
    let router = router(temp.path(), None, Some(auth), true);
    let first = call(
        &router,
        &tokens[0],
        &[FIRST_KEY],
        "register",
        json!({"label":"first"}),
    )
    .await;
    let second = call(
        &router,
        &tokens[1],
        &[FIRST_KEY],
        "register",
        json!({"label":"second"}),
    )
    .await;
    assert_ne!(ok(&first)["session_id"], ok(&second)["session_id"]);
    let directory = call(&router, &tokens[0], &[FIRST_KEY], "sessions", json!({})).await;
    assert_eq!(ok(&directory)["sessions"].as_array().unwrap().len(), 2);

    store
        .revoke_refresh_token("synthetic-refresh-fixture-first")
        .await
        .unwrap();
    let discovery = rpc(&router, "server/discover", json!({}), &tokens[0], &[]).await;
    assert!(discovery["result"].is_object(), "{discovery}");
    let denied = call(&router, &tokens[0], &[FIRST_KEY], "sessions", json!({})).await;
    assert_eq!(denied["error"]["data"]["kind"], "forbidden", "{denied}");
    let still_current = call(&router, &tokens[1], &[FIRST_KEY], "sessions", json!({})).await;
    assert_eq!(ok(&still_current)["sessions"].as_array().unwrap().len(), 2);
}
