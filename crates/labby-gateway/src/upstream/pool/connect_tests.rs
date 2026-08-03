use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use labby_auth::upstream::cache::OauthClientCache;
use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::UpstreamPool;
use super::connect::connect_http_upstream;
use super::testsupport::test_upstream_config;

#[derive(Clone, Default)]
struct LegacyLifecycleResponder {
    discover_requests: Arc<AtomicUsize>,
    initialize_requests: Arc<AtomicUsize>,
    list_tools_requests: Arc<AtomicUsize>,
}

impl Respond for LegacyLifecycleResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "server/discover" => {
                self.discover_requests.fetch_add(1, Ordering::SeqCst);
                let version = request
                    .headers
                    .get("mcp-protocol-version")
                    .and_then(|value| value.to_str().ok());
                if version == Some("2026-07-28") {
                    ResponseTemplate::new(400).set_body_string(
                        "Bad Request: Unsupported MCP-Protocol-Version: 2026-07-28",
                    )
                } else {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": "Method not found"}
                    }))
                }
            }
            "initialize" => {
                self.initialize_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .insert_header("Mcp-Session-Id", "legacy-session")
                    .set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-11-25",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "legacy-test", "version": "1.0.0"}
                        }
                    }))
            }
            "notifications/initialized" => ResponseTemplate::new(202),
            "tools/list" => {
                self.list_tools_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "legacy_echo",
                            "description": "legacy lifecycle proof",
                            "inputSchema": {"type": "object"}
                        }]
                    }
                }))
            }
            other => ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

#[tokio::test]
async fn http_upstream_falls_back_after_transport_rejects_2026_discovery() {
    let server = MockServer::start().await;
    let responder = LegacyLifecycleResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "legacy-http".to_string();
    config.url = Some(format!("{}/mcp", server.uri()));

    let (_connection, tools) = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        None,
        None,
        (),
    )
    .await
    .expect("gateway should bridge a legacy upstream lifecycle");

    assert_eq!(
        responder.discover_requests.load(Ordering::SeqCst),
        1,
        "transport-level rejection must reconnect directly through initialize"
    );
    assert_eq!(
        responder.initialize_requests.load(Ordering::SeqCst),
        1,
        "transport-level protocol rejection must trigger legacy initialization"
    );
    assert_eq!(responder.list_tools_requests.load(Ordering::SeqCst), 1);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "legacy_echo");
}

#[derive(Clone, Default)]
struct DiscoveryMetadataResponder {
    discover_requests: Arc<AtomicUsize>,
    initialize_requests: Arc<AtomicUsize>,
    list_tools_requests: Arc<AtomicUsize>,
}

impl Respond for DiscoveryMetadataResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "server/discover" => {
                self.discover_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "resultType": "complete",
                        "supportedVersions": [
                            "2026-07-28",
                            "2025-11-25",
                            "2025-06-18"
                        ],
                        "capabilities": {"tools": {}},
                        "ttlMs": 0,
                        "cacheScope": "private",
                        "_meta": {
                            "io.modelcontextprotocol/serverInfo": {
                                "name": "metadata-discovery-test",
                                "version": "1.0.0"
                            }
                        }
                    }
                }))
            }
            "initialize" => {
                self.initialize_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .insert_header("Mcp-Session-Id", "metadata-fallback-session")
                    .set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-11-25",
                            "capabilities": {"tools": {}},
                            "serverInfo": {
                                "name": "metadata-discovery-test",
                                "version": "1.0.0"
                            }
                        }
                    }))
            }
            "notifications/initialized" => ResponseTemplate::new(202),
            "tools/list" => {
                self.list_tools_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "metadata_discovery_echo",
                            "description": "modern discovery metadata proof",
                            "inputSchema": {"type": "object"}
                        }]
                    }
                }))
            }
            other => ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

#[tokio::test]
async fn http_upstream_accepts_discovery_result_with_metadata() {
    let server = MockServer::start().await;
    let responder = DiscoveryMetadataResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "metadata-discovery-http".to_string();
    config.url = Some(format!("{}/mcp", server.uri()));

    let (_connection, tools) = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        None,
        None,
        (),
    )
    .await
    .expect("gateway should accept a modern discovery response with metadata");

    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 1);
    assert_eq!(responder.initialize_requests.load(Ordering::SeqCst), 0);
    assert_eq!(responder.list_tools_requests.load(Ordering::SeqCst), 1);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "metadata_discovery_echo");
}

#[derive(Clone, Default)]
struct SseMethodNotFoundResponder {
    initialize_requests: Arc<AtomicUsize>,
}

impl Respond for SseMethodNotFoundResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(Value::Null);

        let response = match method {
            "server/discover" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "server/discover"}
            }),
            "initialize" => {
                self.initialize_requests.fetch_add(1, Ordering::SeqCst);
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "sse-legacy", "version": "1.0.0"}
                    }
                })
            }
            "notifications/initialized" => return ResponseTemplate::new(202),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": []}
            }),
            other => {
                return ResponseTemplate::new(500)
                    .set_body_string(format!("unexpected MCP method: {other}"));
            }
        };

        ResponseTemplate::new(200).set_body_raw(
            format!("event: message\ndata: {response}\n\n"),
            "text/event-stream",
        )
    }
}

#[tokio::test]
async fn http_upstream_falls_back_when_discover_method_not_found_arrives_over_sse() {
    let server = MockServer::start().await;
    let responder = SseMethodNotFoundResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "sse-legacy-http".to_string();
    config.url = Some(format!("{}/mcp", server.uri()));

    let (_connection, tools) = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        None,
        None,
        (),
    )
    .await
    .expect("SSE legacy upstream should initialize");

    assert!(tools.is_empty());
    assert_eq!(responder.initialize_requests.load(Ordering::SeqCst), 1);
}

#[derive(Clone, Default)]
struct SessionRequiredResponder {
    initialize_requests: Arc<AtomicUsize>,
}

impl Respond for SessionRequiredResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "server/discover" => ResponseTemplate::new(400).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32000, "message": "Bad Request: No valid session ID provided"}
            })),
            "initialize" => {
                self.initialize_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .insert_header("Mcp-Session-Id", "session-required")
                    .set_body_raw(
                        format!(
                            "event: message\ndata: {}\n\n",
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "protocolVersion": "2025-11-25",
                                    "capabilities": {"tools": {}},
                                    "serverInfo": {"name": "session-required", "version": "1.0.0"}
                                }
                            })
                        ),
                        "text/event-stream",
                    )
            }
            "notifications/initialized" => ResponseTemplate::new(202),
            "tools/list" => {
                assert_eq!(
                    request
                        .headers
                        .get("mcp-session-id")
                        .and_then(|value| value.to_str().ok()),
                    Some("session-required")
                );
                ResponseTemplate::new(200).set_body_raw(
                    format!(
                        "event: message\ndata: {}\n\n",
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {"tools": []}
                        })
                    ),
                    "text/event-stream",
                )
            }
            other => ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

#[tokio::test]
async fn http_upstream_initializes_when_discovery_requires_a_session() {
    let server = MockServer::start().await;
    let responder = SessionRequiredResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "session-required-http".to_string();
    config.url = Some(format!("{}/mcp", server.uri()));

    let (_connection, tools) = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        None,
        None,
        (),
    )
    .await
    .expect("session-required upstream should use initialize");

    assert!(tools.is_empty());
    assert_eq!(responder.initialize_requests.load(Ordering::SeqCst), 1);
}

#[derive(Clone, Default)]
struct ModernLifecycleResponder {
    discover_requests: Arc<AtomicUsize>,
    initialize_requests: Arc<AtomicUsize>,
}

impl Respond for ModernLifecycleResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "server/discover" => {
                self.discover_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "resultType": "complete",
                        "supportedVersions": ["2026-07-28"],
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "modern-test", "version": "1.0.0"},
                        "ttlMs": 0,
                        "cacheScope": "private"
                    }
                }))
            }
            "initialize" => {
                self.initialize_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(500)
                    .set_body_string("modern upstream must not receive initialize")
            }
            "tools/list" => ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": []}
            })),
            other => ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

#[tokio::test]
async fn http_upstream_keeps_modern_lifecycle_without_legacy_probe() {
    let server = MockServer::start().await;
    let responder = ModernLifecycleResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "modern-http".to_string();
    config.url = Some(format!("{}/mcp", server.uri()));

    let (_connection, tools) = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        None,
        None,
        (),
    )
    .await
    .expect("modern upstream should connect");

    assert!(tools.is_empty());
    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 1);
    assert_eq!(responder.initialize_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn http_upstream_does_not_downgrade_generic_server_failures() {
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    let request_counter = Arc::clone(&requests);
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(move |_request: &Request| {
            request_counter.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(500).set_body_string("Internal Server Error")
        })
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "broken-http".to_string();
    config.url = Some(format!("{}/mcp", server.uri()));

    connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        None,
        None,
        (),
    )
    .await
    .expect_err("generic server failures must remain failures");

    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "generic failures must not trigger lifecycle fallback"
    );
}

fn oauth_http_config() -> UpstreamConfig {
    UpstreamConfig {
        enabled: true,
        name: "oauth-upstream".into(),
        url: Some("http://127.0.0.1:8080/mcp".into()),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: vec![],
        env: std::collections::BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: Some(UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Preregistered {
                client_id: "client-id".into(),
                client_secret_env: None,
            },
            scopes: None,
            prefer_client_metadata_document: None,
        }),
        imported_from: None,
        priority: 1.0,
    }
}

#[tokio::test]
async fn subject_scoped_upstream_requires_authenticated_subject_for_oauth_http_connect() {
    let config = oauth_http_config();
    let error = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        Some(&OauthClientCache::new(Arc::new(dashmap::DashMap::new()))),
        None,
        (),
    )
    .await
    .expect_err("missing subject should fail");

    assert!(
        error
            .to_string()
            .contains("requires an authenticated subject")
    );
}

#[tokio::test]
async fn subject_scoped_upstream_requires_registered_cache_for_oauth_http_connect() {
    let config = oauth_http_config();
    let error = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        Some("alice"),
        None,
        None,
        (),
    )
    .await
    .expect_err("missing cache should fail");

    assert!(
        error
            .to_string()
            .contains("no auth client cache is registered")
    );
}

#[tokio::test]
async fn shared_discovery_skips_oauth_http_upstreams() {
    let pool = UpstreamPool::new()
        .with_oauth_client_cache(OauthClientCache::new(Arc::new(dashmap::DashMap::new())));
    pool.discover_all(&[oauth_http_config()]).await;

    assert_eq!(pool.upstream_count().await, 0);
    assert!(pool.upstream_status().await.is_empty());
}
