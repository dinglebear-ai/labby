use Future;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rmcp::RoleClient;
use rmcp::model::{
    DetailedTask, NumberOrString, ProgressNotification, ProgressNotificationParam, ProgressToken,
    ServerNotification, Task, TaskPayload, TaskStatus, TaskStatusNotification,
    TaskStatusNotificationParams,
};
use rmcp::service::{RawRxJsonRpcMessage, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use labby_auth::upstream::cache::OauthClientCache;
use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::UpstreamPool;
use super::connect::{
    OrderedRelayNotification, OrderedRelayNotificationTransport, RelayNotificationInterceptor,
    connect_http_upstream,
};
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
                if body
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    != Some("2025-11-25")
                {
                    return ResponseTemplate::new(400)
                        .set_body_string("legacy initialize must advertise 2025-11-25");
                }
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
struct VersionNegotiationResponder {
    discover_requests: Arc<AtomicUsize>,
    initialize_requests: Arc<AtomicUsize>,
}

impl Respond for VersionNegotiationResponder {
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
                            "2025-11-25",
                            "2025-06-18",
                            "2025-03-26",
                            "2024-11-05"
                        ],
                        "capabilities": {"tools": {}},
                        "ttlMs": 0,
                        "cacheScope": "private",
                        "_meta": {
                            "io.modelcontextprotocol/serverInfo": {
                                "name": "version-negotiation-test",
                                "version": "1.0.0"
                            }
                        }
                    }
                }))
            }
            "initialize" => {
                self.initialize_requests.fetch_add(1, Ordering::SeqCst);
                if body
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    != Some("2025-11-25")
                {
                    return ResponseTemplate::new(400)
                        .set_body_string("legacy initialize must advertise 2025-11-25");
                }
                ResponseTemplate::new(200)
                    .insert_header("Mcp-Session-Id", "version-negotiation-session")
                    .set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-11-25",
                            "capabilities": {"tools": {}},
                            "serverInfo": {
                                "name": "version-negotiation-test",
                                "version": "1.0.0"
                            }
                        }
                    }))
            }
            "notifications/initialized" => ResponseTemplate::new(202),
            "tools/list" => ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "negotiated_echo",
                        "description": "version negotiation proof",
                        "inputSchema": {"type": "object"}
                    }]
                }
            })),
            other => ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

#[tokio::test]
async fn http_upstream_falls_back_when_discovery_versions_do_not_overlap() {
    let server = MockServer::start().await;
    let responder = VersionNegotiationResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "version-negotiation-http".to_string();
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
    .expect("gateway should negotiate a legacy lifecycle version");

    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 1);
    assert_eq!(responder.initialize_requests.load(Ordering::SeqCst), 1);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "negotiated_echo");
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
                            "description": "discovery metadata compatibility proof",
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
async fn http_upstream_accepts_discovery_result_with_server_info_metadata() {
    let server = MockServer::start().await;
    let responder = DiscoveryMetadataResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "metadata-fallback-http".to_string();
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
    .expect("gateway should accept a valid discovery response");

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

#[derive(Clone, Default)]
struct HeaderRecoveryResponder {
    list_tools_requests: Arc<AtomicUsize>,
    tool_calls: Arc<AtomicUsize>,
}

impl Respond for HeaderRecoveryResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "server/discover" => ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resultType": "complete",
                    "supportedVersions": ["2026-07-28"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "header-recovery", "version": "1.0.0"},
                    "ttlMs": 0,
                    "cacheScope": "private"
                }
            })),
            "tools/list" => {
                let call = self.list_tools_requests.fetch_add(1, Ordering::SeqCst);
                let owner_schema = if call == 0 {
                    json!({"type": "string"})
                } else {
                    json!({"type": "string", "x-mcp-header": "owner"})
                };
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "pull_request_read",
                            "description": "header recovery proof",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"owner": owner_schema},
                                "required": ["owner"]
                            }
                        }]
                    }
                }))
            }
            "tools/call" => {
                let attempt = self.tool_calls.fetch_add(1, Ordering::SeqCst);
                let owner = request
                    .headers
                    .get("mcp-param-owner")
                    .and_then(|value| value.to_str().ok());
                if attempt == 0 && owner.is_none() {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": rmcp::model::ErrorCode::HEADER_MISMATCH.0,
                            "message": "header mismatch: missing Mcp-Param-owner header for parameter \"owner\""
                        }
                    }));
                }
                if owner != Some("dinglebear-ai") {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": rmcp::model::ErrorCode::HEADER_MISMATCH.0,
                            "message": "header mismatch: Mcp-Param-owner did not refresh"
                        }
                    }));
                }
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": "http-recovered"}],
                        "isError": false
                    }
                }))
            }
            other => ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

#[tokio::test]
async fn http_header_mismatch_refreshes_rmcp_schema_cache_and_mcp_param_header() {
    let server = MockServer::start().await;
    let responder = HeaderRecoveryResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let mut config = test_upstream_config();
    config.name = "header-recovery-http".to_string();
    config.url = Some(format!("{}/mcp", server.uri()));

    let (connection, tools) = connect_http_upstream(
        config.url.as_deref().expect("url"),
        &config,
        None,
        None,
        None,
        (),
    )
    .await
    .expect("header recovery upstream connects");
    assert_eq!(tools.len(), 1);
    assert_eq!(responder.list_tools_requests.load(Ordering::SeqCst), 1);

    let mut params = rmcp::model::CallToolRequestParams::new("pull_request_read");
    params.arguments = Some(serde_json::Map::from_iter([(
        "owner".to_string(),
        Value::String("dinglebear-ai".to_string()),
    )]));
    let pool = UpstreamPool::new();
    let response = super::tools_call::call_tool_once_with_header_recovery(
        &pool,
        &connection.peer,
        &config.name,
        params,
    )
    .await
    .expect("HeaderMismatch should refresh the real HTTP transport schema cache");

    assert!(matches!(
        response,
        rmcp::model::CallToolResponse::Complete(_)
    ));
    assert_eq!(
        responder.list_tools_requests.load(Ordering::SeqCst),
        2,
        "one recovery tools/list must refresh rmcp's transport-local schema cache"
    );
    assert_eq!(
        responder.tool_calls.load(Ordering::SeqCst),
        2,
        "the original tools/call may be replayed exactly once"
    );
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
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: Some(UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Preregistered {
                client_id: "client-id".into(),
                client_secret_env: None,
            },
            scopes: None,
            credential: Default::default(),
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

struct OrderedRelayNotificationFixture {
    messages: VecDeque<RxJsonRpcMessage<RoleClient>>,
}

struct OrderedRelayRawFixture {
    messages: VecDeque<RawRxJsonRpcMessage<RoleClient>>,
}

impl Transport<RoleClient> for OrderedRelayRawFixture {
    type Error = Infallible;

    fn preserves_raw_responses() -> bool {
        true
    }

    fn send(
        &mut self,
        _item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        std::future::ready(Ok(()))
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        None
    }

    fn receive_raw(
        &mut self,
    ) -> impl Future<Output = Option<RawRxJsonRpcMessage<RoleClient>>> + Send {
        std::future::ready(self.messages.pop_front())
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        std::future::ready(Ok(()))
    }
}

impl Transport<RoleClient> for OrderedRelayNotificationFixture {
    type Error = Infallible;

    fn send(
        &mut self,
        _item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        std::future::ready(Ok(()))
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        std::future::ready(self.messages.pop_front())
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        std::future::ready(Ok(()))
    }
}

fn progress_message(message: &str, progress: f64) -> RxJsonRpcMessage<RoleClient> {
    RxJsonRpcMessage::<RoleClient>::notification(ServerNotification::ProgressNotification(
        ProgressNotification::new(
            ProgressNotificationParam::new(
                ProgressToken(NumberOrString::String("ordered-progress".into())),
                progress,
            )
            .with_message(message),
        ),
    ))
}

fn task_status_message() -> RxJsonRpcMessage<RoleClient> {
    let task = DetailedTask::new(
        Task::new(
            "native-task",
            TaskStatus::Working,
            "2026-08-01T00:00:00Z",
            "2026-08-01T00:00:01Z",
        ),
        TaskPayload::Working,
    );
    RxJsonRpcMessage::<RoleClient>::notification(ServerNotification::TaskStatusNotification(
        TaskStatusNotification::new(TaskStatusNotificationParams::new(task)),
    ))
}

#[tokio::test]
async fn ordered_relay_raw_receive_preserves_order_cancellation_and_result_body() {
    let observed = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let interceptor_observed = Arc::clone(&observed);
    let interceptor_release = Arc::clone(&release);
    let interceptor: RelayNotificationInterceptor = Arc::new(move |notification| {
        let observed = Arc::clone(&interceptor_observed);
        let release = Arc::clone(&interceptor_release);
        Box::pin(async move {
            release.acquire().await.expect("release permit").forget();
            if let OrderedRelayNotification::Progress(params) = notification {
                observed
                    .lock()
                    .await
                    .push(params.message.unwrap_or_default());
            }
        })
    });
    let notification = RawRxJsonRpcMessage::<RoleClient>::notification(
        ServerNotification::ProgressNotification(ProgressNotification::new(
            ProgressNotificationParam::new(
                ProgressToken(NumberOrString::String("ordered-progress".into())),
                0.5,
            )
            .with_message("before-result"),
        )),
    );
    let result_body = json!({
        "resultType": "complete",
        "skills": [{"uri": "skill://up/x/SKILL.md"}],
        "extensionOnly": {"preserved": true}
    });
    let response =
        RawRxJsonRpcMessage::<RoleClient>::response(result_body.clone(), NumberOrString::Number(7));
    let fixture = OrderedRelayRawFixture {
        messages: VecDeque::from([notification, response]),
    };
    let mut transport = OrderedRelayNotificationTransport::new(fixture, Some(interceptor));

    let timed_out = tokio::time::timeout(
        std::time::Duration::from_millis(10),
        transport.receive_raw(),
    )
    .await;
    assert!(
        timed_out.is_err(),
        "raw response must wait for notification delivery"
    );

    release.add_permits(1);
    let message = transport.receive_raw().await.expect("raw response");
    let RawRxJsonRpcMessage::<RoleClient>::Response(response) = message else {
        panic!("expected raw response");
    };
    assert_eq!(observed.lock().await.as_slice(), ["before-result"]);
    assert_eq!(response.result, result_body);
}

#[tokio::test]
async fn ordered_relay_notification_transport_preserves_progress_wire_order() {
    let observed = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let interceptor_observed = Arc::clone(&observed);
    let interceptor: RelayNotificationInterceptor = Arc::new(move |notification| {
        let observed = Arc::clone(&interceptor_observed);
        Box::pin(async move {
            match notification {
                OrderedRelayNotification::Progress(params) => observed
                    .lock()
                    .await
                    .push(params.message.unwrap_or_default()),
                OrderedRelayNotification::TaskStatus(_) => observed
                    .lock()
                    .await
                    .push("unexpected-task-status".to_string()),
            }
        })
    });
    let fixture = OrderedRelayNotificationFixture {
        messages: VecDeque::from([
            progress_message("quarter", 0.25),
            progress_message("three-quarters", 0.75),
        ]),
    };
    let mut transport = OrderedRelayNotificationTransport::new(fixture, Some(interceptor));

    assert!(transport.receive().await.is_none());
    assert_eq!(
        observed.lock().await.as_slice(),
        ["quarter", "three-quarters"]
    );
}

#[tokio::test]
async fn ordered_relay_notification_transport_preserves_progress_after_receive_cancellation() {
    let observed = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let interceptor_observed = Arc::clone(&observed);
    let interceptor_release = Arc::clone(&release);
    let interceptor: RelayNotificationInterceptor = Arc::new(move |notification| {
        let observed = Arc::clone(&interceptor_observed);
        let release = Arc::clone(&interceptor_release);
        Box::pin(async move {
            match notification {
                OrderedRelayNotification::Progress(params) => {
                    release
                        .acquire()
                        .await
                        .expect("progress release permit")
                        .forget();
                    observed
                        .lock()
                        .await
                        .push(params.message.unwrap_or_default());
                }
                OrderedRelayNotification::TaskStatus(_) => observed
                    .lock()
                    .await
                    .push("unexpected-task-status".to_string()),
            }
        })
    });
    let fixture = OrderedRelayNotificationFixture {
        messages: VecDeque::from([
            progress_message("quarter", 0.25),
            progress_message("three-quarters", 0.75),
        ]),
    };
    let mut transport = OrderedRelayNotificationTransport::new(fixture, Some(interceptor));

    let timed_out =
        tokio::time::timeout(std::time::Duration::from_millis(10), transport.receive()).await;
    assert!(
        timed_out.is_err(),
        "receive should be cancelled while progress forwarding is blocked"
    );

    release.add_permits(2);
    assert!(transport.receive().await.is_none());
    assert_eq!(
        observed.lock().await.as_slice(),
        ["quarter", "three-quarters"]
    );
}

#[tokio::test]
async fn ordered_relay_notification_transport_preserves_task_status_after_receive_cancellation() {
    let observed = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let interceptor_observed = Arc::clone(&observed);
    let interceptor_release = Arc::clone(&release);
    let interceptor: RelayNotificationInterceptor = Arc::new(move |notification| {
        let observed = Arc::clone(&interceptor_observed);
        let release = Arc::clone(&interceptor_release);
        Box::pin(async move {
            match notification {
                OrderedRelayNotification::TaskStatus(params) => {
                    release
                        .acquire()
                        .await
                        .expect("task status release permit")
                        .forget();
                    observed.lock().await.push(params.task.task.task_id);
                }
                OrderedRelayNotification::Progress(_) => observed
                    .lock()
                    .await
                    .push("unexpected-progress".to_string()),
            }
        })
    });
    let fixture = OrderedRelayNotificationFixture {
        messages: VecDeque::from([task_status_message()]),
    };
    let mut transport = OrderedRelayNotificationTransport::new(fixture, Some(interceptor));

    let timed_out =
        tokio::time::timeout(std::time::Duration::from_millis(10), transport.receive()).await;
    assert!(
        timed_out.is_err(),
        "receive should be cancelled while task-status forwarding is blocked"
    );

    release.add_permits(1);
    assert!(transport.receive().await.is_none());
    assert_eq!(observed.lock().await.as_slice(), ["native-task"]);
}
