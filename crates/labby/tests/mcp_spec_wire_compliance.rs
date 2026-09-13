//! Literal MCP 2026-07-28 HTTP oracles against the actual product process.
//! These clients deliberately do not use rmcp serialization or error constants.

#![cfg(all(feature = "gateway", feature = "proxy-testkit", unix))]
#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_tools_transport_qualification/mod.rs"]
mod transport;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use std::time::Duration;

use serde_json::{Value, json};
use transport::{TransportKind, TransportQualification};

#[derive(Clone, Copy, Debug)]
enum OriginCase {
    SameOrigin,
    Disallowed,
    Malformed,
    Duplicate,
    NonUtf8,
    Path,
    Query,
    Userinfo,
    Fragment,
}

impl OriginCase {
    fn headers(self, endpoint: &str) -> Vec<reqwest::header::HeaderValue> {
        let url = reqwest::Url::parse(endpoint).expect("fixture URL");
        let origin = url.origin().ascii_serialization();
        let values = match self {
            Self::SameOrigin => vec![origin.as_bytes().to_vec()],
            Self::Disallowed => vec![b"https://attacker.invalid".to_vec()],
            Self::Malformed => vec![b"not an origin".to_vec()],
            Self::Duplicate => vec![origin.as_bytes().to_vec(), origin.as_bytes().to_vec()],
            Self::NonUtf8 => vec![vec![0xff]],
            Self::Path => vec![format!("{origin}/evil").into_bytes()],
            Self::Query => vec![format!("{origin}?evil=true").into_bytes()],
            Self::Userinfo => vec![origin.replacen("://", "://attacker@", 1).into_bytes()],
            Self::Fragment => vec![format!("{origin}#evil").into_bytes()],
        };
        values
            .iter()
            .map(|value| {
                reqwest::header::HeaderValue::from_bytes(value).expect("HTTP header bytes")
            })
            .collect()
    }
}

fn request(method: &str, version: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": "independent-wire-oracle", "method": method,
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientInfo": {"name": "wire-oracle", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }}
    })
}

async fn rejected_request(
    body: Value,
    method_header: &str,
    version_header: &str,
    origin: Option<OriginCase>,
    expected_status: u16,
    expected_error: Option<i64>,
) {
    let runner = TransportQualification::start(TransportKind::StreamableHttp, "wire-oracle-canary")
        .await
        .expect("real Labby MCP process");
    runner
        .call_raw("forge.safe", json!({}))
        .await
        .expect("initialize independent side-effect ledger");
    let before = runner.effect_counts().expect("baseline side effects");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("bounded independent HTTP client");
    let endpoint = runner.http_endpoint().expect("HTTP endpoint");
    let mut call = client
        .post(&endpoint)
        .bearer_auth(runner.http_token())
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", version_header)
        .header("mcp-method", method_header)
        .header("x-labby-project-id", "disposable")
        .header("x-labby-team-id", "bootstrap-initial-team")
        .json(&body);
    if let Some(origin) = origin {
        for header in origin.headers(&endpoint) {
            call = call.header("origin", header);
        }
    }
    let response = call.send().await.expect("HTTP response");
    let status = response.status().as_u16();
    let response_body = response.text().await.expect("bounded response body");
    let after = runner.effect_counts().expect("post-rejection side effects");
    let cleanup = runner.finish().await;
    assert!(
        cleanup.is_clean(),
        "unclean fixture: {:?}",
        cleanup.failures
    );
    assert_eq!(
        status, expected_status,
        "unexpected HTTP status for {origin:?}: {response_body}"
    );
    assert_eq!(before, after, "rejected request reached upstream execution");
    if let Some(code) = expected_error {
        let error: Value = serde_json::from_str(&response_body).expect("JSON-RPC error response");
        assert_eq!(error["jsonrpc"], "2.0");
        assert_eq!(error["error"]["code"].as_i64(), Some(code));
    }
}

#[tokio::test]
async fn mcp_spec_http_invalid_origin_is_403() {
    // Streamable HTTP security: a present invalid Origin MUST receive 403.
    // A positive control prevents a blanket Origin rejection from passing.
    rejected_request(
        request("tools/list", "2026-07-28"),
        "tools/list",
        "2026-07-28",
        Some(OriginCase::SameOrigin),
        200,
        None,
    )
    .await;
    for origin in [
        OriginCase::Disallowed,
        OriginCase::Malformed,
        OriginCase::Duplicate,
        OriginCase::NonUtf8,
        OriginCase::Path,
        OriginCase::Query,
        OriginCase::Userinfo,
        OriginCase::Fragment,
    ] {
        rejected_request(
            request("tools/list", "2026-07-28"),
            "tools/list",
            "2026-07-28",
            Some(origin),
            403,
            None,
        )
        .await;
    }
}

#[tokio::test]
async fn mcp_spec_http_protocol_header_mismatch_is_400() {
    // The body is authoritative; a mirrored protocol header cannot disagree.
    rejected_request(
        request("tools/list", "2026-07-28"),
        "tools/list",
        "2025-11-25",
        None,
        400,
        Some(-32020),
    )
    .await;
}

#[tokio::test]
async fn mcp_spec_http_unknown_method_is_404_method_not_found() {
    rejected_request(
        request("unknown/compliance-oracle", "2026-07-28"),
        "unknown/compliance-oracle",
        "2026-07-28",
        None,
        404,
        Some(-32601),
    )
    .await;
}
