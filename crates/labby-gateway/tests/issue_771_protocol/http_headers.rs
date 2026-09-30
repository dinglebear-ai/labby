use super::fixture::{DEADLINE, client_info, detailed};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt,
    model::{
        CancelTaskParams, GetTaskParams, ProtocolVersion, ServerCapabilities, UpdateTaskParams,
    },
    transport::StreamableHttpClientTransport,
};
use serde_json::{Value, json};
use tokio::time::timeout;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::method};

#[tokio::test]
async fn sdk_http_helpers_emit_native_task_routing_headers_without_session_ids() {
    drop(rustls::crypto::ring::default_provider().install_default());
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &Request| {
        let body: Value = request.body_json().unwrap();
        let result = match body["method"].as_str().unwrap() {
            "server/discover" => json!({"resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": serde_json::to_value(ServerCapabilities::builder().enable_tasks().build()).unwrap(), "ttlMs": 0, "cacheScope": "private"}),
            "tasks/get" => { let mut task = detailed("working"); task["resultType"] = json!("complete"); task },
            "tasks/update" | "tasks/cancel" => json!({"resultType": "complete"}),
            other => return ResponseTemplate::new(500).set_body_string(format!("unexpected HTTP method: {other}")),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}))
    }).expect(4).mount(&server).await;
    let transport = StreamableHttpClientTransport::from_uri(server.uri());
    let client = timeout(
        DEADLINE,
        client_info().serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let native = "fixture-native-opaque-id";
    timeout(DEADLINE, client.get_task(GetTaskParams::new(native)))
        .await
        .unwrap()
        .unwrap();
    timeout(
        DEADLINE,
        client.update_task(UpdateTaskParams::new(native, Default::default())),
    )
    .await
    .unwrap()
    .unwrap();
    timeout(DEADLINE, client.cancel_task(CancelTaskParams::new(native)))
        .await
        .unwrap()
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    for request in requests.iter().skip(1) {
        let body: Value = request.body_json().unwrap();
        assert_eq!(
            request.headers.get("mcp-method").unwrap().to_str().unwrap(),
            body["method"].as_str().unwrap()
        );
        assert_eq!(
            request.headers.get("mcp-name").unwrap().to_str().unwrap(),
            native
        );
        assert_eq!(
            request
                .headers
                .get("mcp-protocol-version")
                .unwrap()
                .to_str()
                .unwrap(),
            "2026-07-28"
        );
        assert!(!request.headers.contains_key("mcp-session-id"));
    }
    client.cancel().await.unwrap();
}
