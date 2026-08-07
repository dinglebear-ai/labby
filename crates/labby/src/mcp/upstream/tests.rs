//! Tests for direct upstream MCP result normalization.

use super::normalize_upstream_result;
use crate::mcp::envelope::build_error;
use labby_gateway::upstream::tool_error::{LABBY_ERROR_META_KEY, McpToolSafetyHints};
use rmcp::model::{CallToolResult, ContentBlock, MetaObject};
use serde_json::{Value, json};

#[test]
fn completed_user_error_is_enriched_without_poisoning_health() {
    let upstream = CallToolResult::error(vec![ContentBlock::text(
        build_error(
            "gateway-alpha",
            "status.update",
            "missing_param",
            "need title",
        )
        .to_string(),
    )]);

    let (normalized, kind) = normalize_upstream_result(
        "status",
        "call_tool",
        "gateway-alpha",
        upstream,
        &McpToolSafetyHints::default(),
    );

    assert_eq!(kind, "missing_param");
    let diagnostic = normalized.content[0].as_text().expect("diagnostic text");
    let value: Value = serde_json::from_str(&diagnostic.text).expect("agent error json");
    assert_eq!(value["origin"], "tool_execution");
    assert_eq!(value["tool"], "gateway-alpha::status");
    assert_eq!(value["recovery"]["action"], "revise_and_retry");
}

#[test]
fn completed_error_retains_every_upstream_payload_channel() {
    let mut meta = MetaObject::default();
    meta.0.insert("vendor.trace".to_string(), json!("trace-42"));
    let structured = json!({
        "machineReadable": true,
        "details": ["one", "two"]
    });
    let mut upstream = CallToolResult::structured_error(structured.clone()).with_meta(Some(meta));
    upstream.content = vec![
        ContentBlock::text(
            json!({
                "error": {
                    "kind": "server_error",
                    "message": "upstream exploded",
                    "vendorCode": 503
                }
            })
            .to_string(),
        ),
        ContentBlock::text("second diagnostic block"),
    ];
    let original_content = upstream.content.clone();

    let (normalized, kind) = normalize_upstream_result(
        "status",
        "call_tool",
        "gateway-alpha",
        upstream,
        &McpToolSafetyHints::default(),
    );

    assert_eq!(kind, "tool_error");
    assert_eq!(&normalized.content[1..], original_content.as_slice());
    assert_eq!(
        normalized.structured_content.as_ref().unwrap()["upstream_structured_content"],
        structured
    );
    let meta = normalized.meta.expect("metadata preserved and enriched");
    assert_eq!(meta.0["vendor.trace"], "trace-42");
    assert!(meta.0.contains_key(LABBY_ERROR_META_KEY));
}
