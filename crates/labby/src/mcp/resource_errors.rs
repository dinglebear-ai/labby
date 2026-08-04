//! Model-actionable errors for MCP resource reads.

use labby_runtime::agent_error::{AgentErrorContext, AgentErrorOrigin, AgentSideEffectRisk};
use rmcp::ErrorData;
use serde_json::json;

use crate::mcp::agent_error::{
    internal as internal_agent_error, invalid_params as invalid_params_agent_error,
    resource_not_found as resource_not_found_agent_error,
};

fn context(uri: &str) -> AgentErrorContext {
    let mut context = AgentErrorContext::for_service_action("labby", "read_resource");
    context.resource = Some(uri.to_string());
    context
}

#[must_use]
pub(crate) fn unknown(uri: &str, ui: bool) -> ErrorData {
    let context = context(uri);
    let label = if ui {
        "unknown UI resource"
    } else {
        "unknown resource"
    };
    resource_not_found_agent_error(
        format!("{label}: {uri}. Call resources/list and retry with an advertised URI."),
        None,
        &context,
    )
}

#[must_use]
pub(crate) fn forbidden(uri: &str, message: &str, required_scopes: &[&str]) -> ErrorData {
    let mut context = context(uri);
    context.origin = Some(AgentErrorOrigin::Policy);
    context.side_effects = Some(AgentSideEffectRisk::NoneExpected);
    let extra = json!({ "required_scopes": required_scopes });
    invalid_params_agent_error("forbidden", message, Some(&extra), &context)
}

#[must_use]
pub(crate) fn route_scope(uri: &str, service: &str, message: &str) -> ErrorData {
    let mut context = context(uri);
    context.origin = Some(AgentErrorOrigin::Policy);
    context.side_effects = Some(AgentSideEffectRisk::NoneExpected);
    let extra = json!({ "service": service });
    invalid_params_agent_error("route_scope_denied", message, Some(&extra), &context)
}

#[must_use]
pub(crate) fn render(uri: &str, message: impl Into<String>) -> ErrorData {
    let message = message.into();
    let mut context = context(uri);
    context.cause = Some(labby_runtime::agent_error::sanitize_error_text(
        &message, 4096,
    ));
    internal_agent_error(
        "internal_error",
        format!("Failed to render resource `{uri}`."),
        None,
        &context,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_resource_points_to_discovery() {
        let error = unknown("lab://missing", false);
        let data = error.data.expect("agent error data");
        assert_eq!(data["resource"], "lab://missing");
        assert_eq!(data["recovery"]["action"], "rediscover");
        assert!(error.message.contains("resources/list"));
    }
}
