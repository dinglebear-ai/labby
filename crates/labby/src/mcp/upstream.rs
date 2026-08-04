//! Upstream-proxy result normalization shared by direct MCP tool calls.

use labby_gateway::upstream::tool_error::{McpToolSafetyHints, enrich_completed_tool_error_result};
use rmcp::model::CallToolResult;

pub(crate) fn normalize_upstream_result(
    service: &str,
    action: &str,
    upstream: &str,
    result: CallToolResult,
    safety: &McpToolSafetyHints,
) -> (CallToolResult, &'static str, bool) {
    if result.is_error != Some(true) {
        return (result, "ok", false);
    }

    let tool = if service.contains("::") {
        service.to_string()
    } else {
        format!("{upstream}::{service}")
    };
    let (result, kind) =
        enrich_completed_tool_error_result(service, action, &tool, Some(upstream), result, safety);

    // A completed MCP result proves the upstream protocol connection worked.
    // Tool-local failures are model-visible but never breaker/health failures.
    (result, kind, false)
}

#[cfg(test)]
mod tests;
