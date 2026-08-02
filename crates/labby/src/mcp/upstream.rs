//! Upstream-proxy result normalization shared by the `call_tool` upstream
//! tail (`call_tool_upstream.rs`).
//!
//! `normalize_upstream_result` was consolidated here from `server.rs`
//! (bead `lab-kvji.24.1.5`, Revision 2 #2): the live `canonical_kind`-based
//! body is the single source of truth; the dead `pub(crate)` duplicate and
//! the dead `static_kind` helper were deleted. Zero behavior change — the
//! live path already used `canonical_kind`.

use rmcp::model::CallToolResult;
use serde_json::Value;

use crate::mcp::error::canonical_kind;

pub(crate) fn normalize_upstream_result(
    _service: &str,
    _action: &str,
    result: CallToolResult,
) -> (CallToolResult, &'static str, bool) {
    if result.is_error != Some(true) {
        return (result, "ok", false);
    }

    // Classification is intentionally observational. The gateway must preserve
    // the complete upstream result, including every content block, structured
    // content, metadata, and extension-owned fields.
    let kind = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .and_then(|content| serde_json::from_str::<Value>(&content.text).ok())
        .and_then(|parsed| {
            parsed
                .get("error")
                .and_then(Value::as_object)
                .or_else(|| parsed.as_object())
                .and_then(|error| error.get("kind"))
                .and_then(Value::as_str)
                .map(canonical_kind)
        })
        .unwrap_or("upstream_error");
    let counts_as_failure = matches!(
        kind,
        "upstream_error" | "network_error" | "server_error" | "decode_error" | "internal_error"
    );

    (result, kind, counts_as_failure)
}

#[cfg(test)]
mod tests;
