//! Surface-neutral error type for dispatch operations.
//!
//! `ToolError` is the single canonical error type across MCP, HTTP, and CLI.
//! Its wire representation includes the additive agent-error contract from
//! `crate::agent_error` so every surface gives model callers consistent
//! recovery and side-effect guidance.

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::agent_error::{
    AgentErrorContext, AgentErrorOrigin, AgentRecoveryAdvice, AgentSideEffectRisk,
    build_agent_error_value,
};

/// Refined agent-error payload carried by [`ToolError::Contract`].
///
/// Boxed inside the variant so the enum stays small; `kind` stays inline on
/// the variant so [`ToolError::kind`] remains a `const fn`.
#[derive(Debug, Clone)]
pub struct AgentContractPayload {
    pub message: String,
    /// Extra envelope fields beyond `kind`/`message` and the refined metadata
    /// below — e.g. `tool`, `cause`, `original_kind`, `safety`, `evidence`,
    /// `retry_after_ms`. Serialized additively into every surface envelope.
    pub extra: Map<String, Value>,
    /// Refined metadata that must override the kind-derived recomputation.
    pub origin: Option<AgentErrorOrigin>,
    pub recovery: Option<AgentRecoveryAdvice>,
    pub side_effects: Option<AgentSideEffectRisk>,
}

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
    /// Pre-computed agent-error contract from a producing subsystem (e.g. a
    /// Code Mode `CodeModeCallError`). Unlike [`ToolError::Sdk`], which keeps
    /// only `kind` + `message` and lets every envelope builder recompute lossy
    /// metadata from the bare kind, this variant carries the full contract:
    /// the payload's extras survive into every surface envelope and its
    /// refined `origin`/`recovery` (including `retry_after_ms`)/`side_effects`
    /// win over the kind-derived recomputation.
    Contract {
        kind: String,
        payload: Box<AgentContractPayload>,
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
            Self::Contract { kind, .. } => kind.as_str(),
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
            Self::Contract { payload, .. } => payload.message.as_str(),
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
            Self::Contract { payload, .. } => Value::Object(payload.extra.clone()),
        }
    }

    /// Build a contract-preserving error from a pre-computed agent-error
    /// object. `extra` carries the additive fields; `origin`/`recovery`/
    /// `side_effects` are the refined metadata that must survive envelope
    /// construction instead of being recomputed from the bare `kind`.
    #[must_use]
    pub fn contract(
        kind: impl Into<String>,
        message: impl Into<String>,
        extra: Map<String, Value>,
        origin: Option<AgentErrorOrigin>,
        recovery: Option<AgentRecoveryAdvice>,
        side_effects: Option<AgentSideEffectRisk>,
    ) -> Self {
        Self::Contract {
            kind: kind.into(),
            payload: Box::new(AgentContractPayload {
                message: message.into(),
                extra,
                origin,
                recovery,
                side_effects,
            }),
        }
    }

    /// Fill `context` with this error's refined contract metadata (when
    /// carried by [`ToolError::Contract`]) without overriding values the
    /// caller already set. No-op for every other variant.
    pub fn merge_contract_context(&self, context: &mut AgentErrorContext) {
        if let Self::Contract { payload, .. } = self {
            context.origin = context.origin.or(payload.origin);
            if context.recovery.is_none() {
                context.recovery.clone_from(&payload.recovery);
            }
            context.side_effects = context.side_effects.or(payload.side_effects);
        }
    }

    #[must_use]
    pub fn to_agent_value(&self) -> Value {
        self.to_agent_value_with_context(&AgentErrorContext::default())
    }

    #[must_use]
    pub fn to_agent_value_with_context(&self, context: &AgentErrorContext) -> Value {
        let extra = self.extra_fields();
        let mut context = context.clone();
        self.merge_contract_context(&mut context);
        build_agent_error_value(self.kind(), self.user_message(), Some(&extra), &context)
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
    fn contract_variant_preserves_refined_metadata_and_extras() {
        use crate::agent_error::{
            AgentErrorOrigin, AgentRecoveryAction, AgentRecoveryAdvice, AgentSameArgumentsRetry,
        };

        // `rate_limited` recomputed from the bare kind classifies as origin
        // `budget`; a producing subsystem refined it to `tool_execution` with
        // a retry hint. The envelope must carry the refined values plus the
        // additive extras (evidence-style fields) instead of recomputing.
        let mut extra = serde_json::Map::new();
        extra.insert("tool".to_string(), serde_json::json!("alpha::demo"));
        extra.insert("original_kind".to_string(), serde_json::json!("429"));
        let err = ToolError::contract(
            "rate_limited",
            "slow down",
            extra,
            Some(AgentErrorOrigin::ToolExecution),
            Some(AgentRecoveryAdvice {
                action: AgentRecoveryAction::RetryLater,
                same_arguments: AgentSameArgumentsRetry::Conditional,
                guidance: "wait for the interval".to_string(),
                retry_after_ms: Some(1500),
            }),
            Some(AgentSideEffectRisk::NoneExpected),
        );

        let value = err
            .to_agent_value_with_context(&AgentErrorContext::for_service_action("snippets", "run"));
        assert_eq!(value["kind"], "rate_limited");
        assert_eq!(value["message"], "slow down");
        assert_eq!(value["origin"], "tool_execution");
        assert_eq!(value["side_effects"], "none_expected");
        assert_eq!(value["recovery"]["retry_after_ms"], 1500);
        assert_eq!(value["tool"], "alpha::demo");
        assert_eq!(value["original_kind"], "429");
        assert_eq!(value["service"], "snippets");
        assert_eq!(value["action"], "run");

        // The default-context path (plain serialization) preserves it too.
        let serialized = serde_json::to_value(&err).expect("serialize");
        assert_eq!(serialized["origin"], "tool_execution");
        assert_eq!(serialized["recovery"]["retry_after_ms"], 1500);
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
