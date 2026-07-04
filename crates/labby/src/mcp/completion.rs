//! Completion + per-service action-schema helpers for the MCP server.
//!
//! Pure free functions extracted from `server.rs` (bead `lab-kvji.24.1.1`).
//! No behavior change — relocation + `pub(crate)` visibility only.

use rmcp::model::CompletionInfo;
use serde_json::Value;

use crate::registry::ToolRegistry;

/// JSON Schema for every service tool's input: `action` (required) + `params` (optional object).
#[allow(clippy::expect_used)]
pub(crate) fn action_schema() -> serde_json::Map<String, Value> {
    serde_json::json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "description": "Action to perform (e.g. \"movie.search\"). Use \"help\" to list all actions."
            },
            "params": {
                "type": "object",
                "description": "Action-specific parameters (varies per action)"
            }
        },
        "required": ["action"]
    })
    .as_object()
    .cloned()
    .expect("schema literal is always an object")
}

pub(crate) fn completion_info(values: Vec<String>) -> CompletionInfo {
    let total = values.len() as u32;
    let has_more = values.len() > CompletionInfo::MAX_VALUES;
    let values = values
        .into_iter()
        .take(CompletionInfo::MAX_VALUES)
        .collect();
    CompletionInfo::with_pagination(values, Some(total), has_more)
        .expect("completion values are capped at rmcp's maximum")
}

pub(crate) fn complete_prompt_arg(
    registry: &ToolRegistry,
    prompt_name: &str,
    argument_name: &str,
    prefix: &str,
) -> CompletionInfo {
    match (prompt_name, argument_name) {
        ("run-action", "action") => completion_info(registry.action_name_completions(prefix)),
        ("run-action" | "service-discover", "service") => {
            completion_info(service_name_completions(registry, prefix))
        }
        _ => completion_info(Vec::new()),
    }
}

pub(crate) fn service_name_completions(registry: &ToolRegistry, prefix: &str) -> Vec<String> {
    registry
        .services()
        .iter()
        .map(|service| service.name)
        .filter(|name| name.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests;
