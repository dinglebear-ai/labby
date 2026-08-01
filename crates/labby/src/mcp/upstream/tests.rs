//! Tests for upstream-proxy result normalization. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::normalize_upstream_result;
use crate::mcp::envelope::build_error;
use rmcp::model::{CallToolResult, ContentBlock, MetaObject};
use serde_json::json;

#[test]
fn normalize_upstream_result_preserves_user_errors_without_poisoning_health() {
    let upstream = CallToolResult::error(vec![ContentBlock::text(
        build_error(
            "gateway-alpha",
            "status.update",
            "missing_param",
            "need title",
        )
        .to_string(),
    )]);

    let (_, kind, counts_as_failure) =
        normalize_upstream_result("gateway-alpha", "call_tool", upstream);

    assert_eq!(kind, "missing_param");
    assert!(!counts_as_failure);
}

#[test]
fn normalize_upstream_result_preserves_the_complete_upstream_payload() {
    let mut meta = MetaObject::default();
    meta.0.insert("vendor.trace".to_string(), json!("trace-42"));
    let structured = json!({
        "machineReadable": true,
        "details": ["one", "two"]
    });
    let mut upstream = CallToolResult::structured_error(structured).with_meta(Some(meta));
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
    let expected = upstream.clone();

    let (normalized, kind, counts_as_failure) =
        normalize_upstream_result("gateway-alpha", "call_tool", upstream);

    assert_eq!(normalized, expected);
    assert_eq!(kind, "server_error");
    assert!(counts_as_failure);
}
