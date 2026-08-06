//! Upstream-proxy result normalization shared by direct MCP tool calls.

use labby_gateway::upstream::tool_error::{McpToolSafetyHints, enrich_completed_tool_error_result};
use rmcp::model::CallToolResult;

/// Fully-qualified `<upstream>::<tool>` identity for envelopes and messages.
/// Shared by the upstream proxy tail (`call_tool_upstream.rs`) and result
/// normalization below.
pub(crate) fn qualified_upstream_tool(upstream: &str, service: &str) -> String {
    if service.contains("::") {
        service.to_string()
    } else {
        format!("{upstream}::{service}")
    }
}

/// Normalize a completed upstream `CallToolResult` for the model.
///
/// A completed MCP result — even with `isError: true` — proves the upstream
/// protocol connection worked. Tool-local failures are model-visible but never
/// breaker/health failures, so this function deliberately returns no
/// "counts as failure" signal: the proxy must not record health state for
/// completed results (the pool already recorded success in
/// `timed_capability_call` / `call_tool_relayed`).
pub(crate) fn normalize_upstream_result(
    service: &str,
    action: &str,
    upstream: &str,
    result: CallToolResult,
    safety: &McpToolSafetyHints,
) -> (CallToolResult, &'static str) {
    if result.is_error != Some(true) {
        return (result, "ok");
    }

    let tool = qualified_upstream_tool(upstream, service);
    enrich_completed_tool_error_result(service, action, &tool, Some(upstream), result, safety)
}

#[cfg(test)]
mod tests;
