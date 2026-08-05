//! Surface-neutral error type for dispatch operations.
//!
//! `ToolError` is the single canonical error type across MCP, HTTP, and CLI.
//! Its wire representation includes the additive agent-error contract from
//! `crate::agent_error` so every surface gives model callers consistent
//! recovery and side-effect guidance.

use serde::Serialize;
use serde_json::{Value, json};

use crate::agent_error::{AgentErrorContext, build_agent_error_value};

#[derive(Debug, Clone)]
pub enum ToolError {
    UnknownAction {
        message: String,
        valid: Vec<String>,
        hint: Option<String>,
    },
    MissingParam {
        message: String,
        param: String,
    },
    InvalidParam {
        message: String,
        param: String,
    },
    #[allow(dead_code)]
    UnknownInstance {
        message: String,
        valid: Vec<String>,
    },
    AmbiguousTool {
        message: String,
        valid: Vec<String>,
    },
    ConfirmationRequired {
        message: String,
    },
    Conflict {
        message: String,
        existing_id: String,
    },
    Forbidden {
        message: String,
        required_scopes: Vec<String>,
    },
    Sdk {
        sdk_kind: String,
        message: String,
    },
}

impl Serialize for ToolError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_agent_value().serialize(serializer)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_string(self) {
            Ok(serialized) => f.write_str(&serialized),
            Err(_) => write!(f, "{self:?}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl ToolError {
    #[must_use]
    pub const fn kind(&self) -> &str {
        match self {
            Self::UnknownAction { .. } => "unknown_action",
            Self::MissingParam { .. } => "missing_param",
            Self::InvalidParam { .. } => "invalid_param",
            Self::UnknownInstance { .. } => "unknown_instance",
            Self::AmbiguousTool { .. } => "ambiguous_tool",
            Self::ConfirmationRequired { .. } => "confirmation_required",
            Self::Conflict { .. } => "conflict",
            Self::Forbidden { .. } => "forbidden",
            Self::Sdk { sdk_kind, .. } => sdk_kind.as_str(),
        }
    }

    #[must_use]
    pub fn user_message(&self) -> &str {
        match self {
            Self::UnknownAction { message, .. }
            | Self::MissingParam { message, .. }
            | Self::InvalidParam { message, .. }
            | Self::UnknownInstance { message, .. }
            | Self::AmbiguousTool { message, .. }
            | Self::ConfirmationRequired { message }
            | Self::Conflict { message, .. }
            | Self::Forbidden { message, .. }
            | Self::Sdk { message, .. } => message.as_str(),
        }
    }

    #[must_use]
    pub fn extra_fields(&self) -> Value {
        match self {
            Self::UnknownAction { valid, hint, .. } => json!({
                "valid": valid,
                "hint": hint,
            }),
            Self::MissingParam { param, .. } | Self::InvalidParam { param, .. } => {
                json!({ "param": param })
            }
            Self::UnknownInstance { valid, .. } | Self::AmbiguousTool { valid, .. } => {
                json!({ "valid": valid })
            }
            Self::ConfirmationRequired { .. } | Self::Sdk { .. } => json!({}),
            Self::Conflict { existing_id, .. } => json!({ "existing_id": existing_id }),
            Self::Forbidden {
                required_scopes, ..
            } => json!({ "required_scopes": required_scopes }),
        }
    }

    #[must_use]
    pub fn to_agent_value(&self) -> Value {
        self.to_agent_value_with_context(&AgentErrorContext::default())
    }

    #[must_use]
    pub fn to_agent_value_with_context(&self, context: &AgentErrorContext) -> Value {
        let extra = self.extra_fields();
        build_agent_error_value(self.kind(), self.user_message(), Some(&extra), context)
    }

    #[must_use]
    pub fn is_internal(&self) -> bool {
        matches!(
            self.kind(),
            "internal_error" | "server_error" | "decode_error"
        )
    }

    #[must_use]
    pub fn internal_message(message: impl Into<String>) -> Self {
        Self::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolError;
    use crate::agent_error::{AgentErrorContext, AgentRecoveryAction, AgentSideEffectRisk};

    #[test]
    fn sdk_variant_promotes_kind_and_agent_metadata() {
        let err = ToolError::Sdk {
            sdk_kind: "rate_limited".to_string(),
            message: "slow down".to_string(),
        };
        let value = serde_json::to_value(&err).expect("serialize");
        assert_eq!(value["kind"], "rate_limited");
        assert_eq!(value["message"], "slow down");
        assert_eq!(value["contract_version"], 1);
        assert_eq!(value["recovery"]["action"], "retry_later");
        assert!(value.get("sdk_kind").is_none());
    }

    #[test]
    fn variant_extras_survive_agent_enrichment() {
        let err = ToolError::UnknownAction {
            message: "unknown action".to_string(),
            valid: vec!["list".to_string()],
            hint: Some("list".to_string()),
        };
        let value = err
            .to_agent_value_with_context(&AgentErrorContext::for_service_action("example", "lst"));
        assert_eq!(value["valid"][0], "list");
        assert_eq!(value["hint"], "list");
        assert_eq!(value["service"], "example");
        assert_eq!(value["action"], "lst");
        assert_eq!(value["side_effects"], "none_expected");
    }

    #[test]
    fn upstream_failure_is_conservative() {
        let err = ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message: "connection closed".to_string(),
        };
        let value = err.to_agent_value();
        assert_eq!(value["side_effects"], "possible");
        assert_eq!(value["recovery"]["same_arguments"], "conditional");
    }

    #[test]
    fn typed_metadata_enums_match_wire_values() {
        let err = ToolError::InvalidParam {
            message: "bad".to_string(),
            param: "query".to_string(),
        };
        let value = err.to_agent_value();
        let metadata = crate::agent_error::metadata_for_kind(err.kind(), None);
        assert_eq!(
            metadata.recovery.action,
            AgentRecoveryAction::ReviseAndRetry
        );
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::NoneExpected);
        assert_eq!(value["origin"], "validation");
    }
}
