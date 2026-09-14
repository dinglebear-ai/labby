//! Additional stateless MCP 2026-07-28 HTTP contracts against the real product.
//! Requests are literal HTTP/JSON and deliberately bypass rmcp client behavior.

#![cfg(all(feature = "gateway", feature = "proxy-testkit", unix))]
#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_tools_transport_qualification.rs"]
mod transport;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use std::time::Duration;

use reqwest::{Client, Response};
use rmcp::model::ProtocolVersion;
use serde_json::{Value, json};
use transport::{TransportKind, TransportQualification};

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("bounded literal HTTP client")
}

fn request(id: Option<&str>, method: &str, version: &str) -> Value {
    let mut request = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientInfo": {"name": "http-contract", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }}
    });
    if let Some(id) = id {
        request["id"] = Value::String(id.to_owned());
    }
    request
}

fn effectful_request(id: &str, version: &str) -> Value {
    let mut request = request(Some(id), "tools/call", version);
    request["params"]["name"] = Value::String("forge.safe".to_owned());
    request["params"]["arguments"] = json!({});
    request
}

async fn start() -> TransportQualification {
    TransportQualification::start(TransportKind::StreamableHttp, "http-contract")
        .await
        .expect("real Labby MCP process")
}

async fn finish(runner: TransportQualification) {
    let cleanup = runner.finish().await;
    assert!(
        cleanup.is_clean(),
        "unclean fixture: {:?}",
        cleanup.failures
    );
}

fn post(
    client: &Client,
    runner: &TransportQualification,
    body: Value,
    protocol_version: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut request = client
        .post(runner.http_endpoint().expect("HTTP endpoint"))
        .bearer_auth(runner.http_token())
        .header("accept", "application/json, text/event-stream")
        .header("mcp-method", body["method"].as_str().expect("method"))
        .header("x-labby-project-id", "disposable")
        .header("x-labby-team-id", "bootstrap-initial-team")
        .json(&body);
    if let Some(version) = protocol_version {
        request = request.header("mcp-protocol-version", version);
    }
    if let Some(name) = body["params"]["name"].as_str() {
        request = request.header("mcp-name", name);
    }
    request
}

async fn body(response: Response) -> (u16, String) {
    let status = response.status().as_u16();
    let body = response.text().await.expect("bounded response body");
    (status, body)
}

async fn first_sse_event(mut response: Response) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut observed = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let chunk = tokio::time::timeout(remaining, response.chunk())
            .await
            .expect("bounded SSE response")
            .expect("read SSE response")
            .expect("SSE response ended before its first event");
        observed.extend_from_slice(&chunk);
        assert!(observed.len() <= 64 * 1024, "first SSE event was unbounded");
        if let Some(end) = observed.windows(2).position(|window| window == b"\n\n") {
            observed.truncate(end + 2);
            return String::from_utf8(observed).expect("UTF-8 SSE event");
        }
    }
}

async fn establish_effectful_control(
    client: &Client,
    runner: &TransportQualification,
) -> (u64, u64) {
    runner
        .call_raw("forge.safe", json!({}))
        .await
        .expect("initialize side-effect ledger");
    let before = runner.effect_counts().expect("pre-control side effects");
    let (status, response) = body(
        post(
            client,
            runner,
            effectful_request("positive-control", "2026-07-28"),
            Some("2026-07-28"),
        )
        .send()
        .await
        .expect("positive-control response"),
    )
    .await;
    assert_eq!(status, 200, "otherwise-valid request failed: {response}");
    let response: Value = serde_json::from_str(&response).expect("JSON control response");
    assert_eq!(response["id"], "positive-control");
    assert_eq!(response["result"]["isError"], false);
    let after = runner.effect_counts().expect("post-control side effects");
    assert_eq!(after, (before.0 + 1, before.1));
    after
}

#[tokio::test]
async fn mcp_spec_http_missing_protocol_version_is_bad_request() {
    let runner = start().await;
    let client = client();
    let before = establish_effectful_control(&client, &runner).await;
    let (status, response) = body(
        post(
            &client,
            &runner,
            effectful_request("positive-control", "2026-07-28"),
            None,
        )
        .send()
        .await
        .expect("HTTP response"),
    )
    .await;
    assert_eq!(status, 400, "missing protocol header accepted: {response}");
    assert_eq!(runner.effect_counts().expect("side effects"), before);
    finish(runner).await;
}

#[tokio::test]
async fn mcp_spec_http_cold_tools_list_completes_without_prior_tool_call() {
    let runner = TransportQualification::start(TransportKind::StreamableHttp, "cold-tools-list")
        .await
        .expect("real Labby MCP process");
    let response = post(
        &client(),
        &runner,
        request(Some("cold-list"), "tools/list", "2026-07-28"),
        Some("2026-07-28"),
    )
    .send()
    .await
    .expect("cold literal tools/list response");
    let (status, body) = body(response).await;
    assert_eq!(status, 200, "cold list failed: {body}");
    let result: Value = serde_json::from_str(&body).expect("cold list JSON response");
    assert_eq!(result["id"], "cold-list");
    assert!(result["result"]["tools"].is_array());
    finish(runner).await;
}

#[tokio::test]
async fn mcp_spec_http_unsupported_protocol_version_is_bad_request() {
    let runner = start().await;
    let client = client();
    let before = establish_effectful_control(&client, &runner).await;
    let unsupported_versions = [
        "2999-12-31",
        "2025-11-25",
        "2025-06-18",
        "2025-03-26",
        "2024-11-05",
    ];
    let sdk_known_legacy_versions = ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .filter(|version| version.as_str() != ProtocolVersion::V_2026_07_28.as_str())
        .map(ProtocolVersion::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unsupported_versions[1..]
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        sdk_known_legacy_versions,
        "literal rejection cases drifted from the SDK-known legacy versions"
    );
    for unsupported in unsupported_versions {
        let (status, response) = body(
            post(
                &client,
                &runner,
                effectful_request("unsupported-version", unsupported),
                Some(unsupported),
            )
            .send()
            .await
            .expect("HTTP response"),
        )
        .await;
        assert_eq!(status, 400, "unsupported protocol accepted: {response}");
        let error: Value = serde_json::from_str(&response).expect("JSON-RPC error response");
        assert_eq!(error["id"], "unsupported-version");
        assert_eq!(error["error"]["code"], -32022);
        assert_eq!(error["error"]["data"]["requested"], unsupported);
        assert_eq!(
            error["error"]["data"]["supported"],
            json!(["2026-07-28"]),
            "HTTP boundary advertised versions it does not implement: {response}"
        );
    }
    assert_eq!(runner.effect_counts().expect("side effects"), before);
    finish(runner).await;
}

#[tokio::test]
async fn labby_legacy_initialized_notification_is_202_without_body() {
    let runner = start().await;
    let (status, response) = body(
        post(
            &client(),
            &runner,
            request(None, "notifications/initialized", "2026-07-28"),
            Some("2026-07-28"),
        )
        .send()
        .await
        .expect("HTTP response"),
    )
    .await;
    assert_eq!(status, 202, "notification was not accepted: {response}");
    assert!(
        response.is_empty(),
        "notification returned a body: {response}"
    );
    finish(runner).await;
}

#[tokio::test]
async fn mcp_spec_http_get_and_delete_are_method_not_allowed() {
    let runner = start().await;
    let endpoint = runner.http_endpoint().expect("HTTP endpoint");
    let client = client();
    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        let (status, response) = body(
            client
                .request(method.clone(), &endpoint)
                .bearer_auth(runner.http_token())
                .send()
                .await
                .expect("HTTP response"),
        )
        .await;
        assert_eq!(status, 405, "{method} was not rejected: {response}");
    }
    finish(runner).await;
}

#[tokio::test]
async fn mcp_spec_http_ignores_legacy_session_header() {
    let runner = start().await;
    let client = client();
    let clean_response = post(
        &client,
        &runner,
        request(Some("legacy-headers"), "tools/list", "2026-07-28"),
        Some("2026-07-28"),
    )
    .send()
    .await
    .expect("clean HTTP response");
    let (clean_status, clean_body) = body(clean_response).await;
    assert_eq!(clean_status, 200, "clean control failed: {clean_body}");
    let clean_json: Value = serde_json::from_str(&clean_body).expect("JSON response control");
    assert_eq!(clean_json["jsonrpc"], "2.0");
    assert_eq!(clean_json["id"], "legacy-headers");

    let response = post(
        &client,
        &runner,
        request(Some("legacy-headers"), "tools/list", "2026-07-28"),
        Some("2026-07-28"),
    )
    .header("mcp-session-id", "obsolete-session")
    .send()
    .await
    .expect("HTTP response");
    assert_eq!(response.status().as_u16(), 200);
    assert!(
        response.headers().get("mcp-session-id").is_none(),
        "server echoed or minted an obsolete session ID"
    );
    let legacy_body = response.text().await.expect("legacy response body");
    assert_eq!(legacy_body, clean_body, "legacy header changed semantics");
    finish(runner).await;
}

#[tokio::test]
async fn mcp_spec_http_ignores_legacy_last_event_id_and_has_no_resume_endpoint() {
    let runner = start().await;
    let client = client();
    let clean_response = post(
        &client,
        &runner,
        request(Some("legacy-last-event-id"), "tools/list", "2026-07-28"),
        Some("2026-07-28"),
    )
    .header("accept", "application/json")
    .send()
    .await
    .expect("bounded clean HTTP response");
    assert_eq!(clean_response.status().as_u16(), 200);
    assert_eq!(
        clean_response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let clean_body = clean_response
        .text()
        .await
        .expect("bounded clean JSON body");

    let response = post(
        &client,
        &runner,
        request(Some("legacy-last-event-id"), "tools/list", "2026-07-28"),
        Some("2026-07-28"),
    )
    .header("accept", "application/json")
    .header("last-event-id", "obsolete-stream-event")
    .send()
    .await
    .expect("bounded HTTP response");
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let legacy_body = response.text().await.expect("bounded legacy JSON body");
    assert_eq!(legacy_body, clean_body, "Last-Event-ID changed semantics");
    let response: Value = serde_json::from_str(&legacy_body).expect("meaningful JSON response");
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "legacy-last-event-id");
    assert!(response["result"]["tools"].is_array());

    let mut listen = request(
        Some("legacy-last-event-id-stream"),
        "subscriptions/listen",
        "2026-07-28",
    );
    listen["params"]["notifications"] = json!({"toolsListChanged": true});
    let clean_stream = post(&client, &runner, listen.clone(), Some("2026-07-28"))
        .send()
        .await
        .expect("bounded clean SSE response");
    assert_eq!(clean_stream.status().as_u16(), 200);
    assert!(
        clean_stream
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "subscriptions/listen did not select SSE"
    );
    let clean_event = first_sse_event(clean_stream).await;

    let legacy_stream = post(&client, &runner, listen, Some("2026-07-28"))
        .header("last-event-id", "obsolete-stream-event")
        .send()
        .await
        .expect("bounded legacy SSE response");
    assert_eq!(legacy_stream.status().as_u16(), 200);
    assert!(
        legacy_stream
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "legacy subscriptions/listen did not select SSE"
    );
    let legacy_event = first_sse_event(legacy_stream).await;
    assert_eq!(
        legacy_event, clean_event,
        "Last-Event-ID resumed or otherwise changed the POST SSE stream"
    );
    assert!(
        legacy_event.contains("notifications/subscriptions/acknowledged"),
        "POST SSE stream did not begin with a fresh subscription acknowledgement: {legacy_event}"
    );
    assert!(
        !legacy_event.lines().any(|line| line.starts_with("id:")),
        "POST SSE stream emitted a resumable event ID: {legacy_event}"
    );

    let resume_response = client
        .get(runner.http_endpoint().expect("HTTP endpoint"))
        .bearer_auth(runner.http_token())
        .header("accept", "text/event-stream")
        .header("last-event-id", "obsolete-stream-event")
        .send()
        .await
        .expect("bounded legacy resume response");
    assert_eq!(resume_response.status().as_u16(), 405);
    assert_eq!(
        resume_response
            .headers()
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("POST"),
        "legacy resume traffic exposed a resumable stream method"
    );
    finish(runner).await;
}
