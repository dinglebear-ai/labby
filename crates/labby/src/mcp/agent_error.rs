//! Model-actionable JSON-RPC error constructors for MCP protocol methods.

use labby_runtime::agent_error::{AgentErrorContext, build_agent_error_value};
use rmcp::ErrorData;
use serde_json::Value;

#[must_use]
pub fn protocol_error_data(
    kind: &str,
    message: &str,
    extra: Option<&Value>,
    context: &AgentErrorContext,
) -> Value {
    build_agent_error_value(kind, message, extra, context)
}

#[must_use]
pub fn invalid_params(
    kind: &str,
    message: impl Into<String>,
    extra: Option<&Value>,
    context: &AgentErrorContext,
) -> ErrorData {
    let message = message.into();
    ErrorData::invalid_params(
        message.clone(),
        Some(protocol_error_data(kind, &message, extra, context)),
    )
}

#[must_use]
pub fn resource_not_found(
    message: impl Into<String>,
    extra: Option<&Value>,
    context: &AgentErrorContext,
) -> ErrorData {
    let message = message.into();
    ErrorData::resource_not_found(
        message.clone(),
        Some(protocol_error_data("not_found", &message, extra, context)),
    )
}

#[must_use]
pub fn internal(
    kind: &str,
    message: impl Into<String>,
    extra: Option<&Value>,
    context: &AgentErrorContext,
) -> ErrorData {
    let message = message.into();
    ErrorData::internal_error(
        message.clone(),
        Some(protocol_error_data(kind, &message, extra, context)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_error_contains_recovery_and_context() {
        let context = AgentErrorContext {
            prompt: Some("missing-prompt".to_string()),
            ..Default::default()
        };
        let error = invalid_params(
            "not_found",
            "unknown prompt: missing-prompt",
            None,
            &context,
        );
        let data = error.data.expect("agent error data");
        assert_eq!(data["prompt"], "missing-prompt");
        assert_eq!(data["recovery"]["action"], "rediscover");
        assert_eq!(data["contract_version"], 1);
    }
}
