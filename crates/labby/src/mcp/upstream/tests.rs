//! Tests for upstream-proxy result normalization. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::normalize_upstream_result;
use crate::mcp::envelope::build_error;
use rmcp::model::{CallToolResult, ContentBlock};

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
