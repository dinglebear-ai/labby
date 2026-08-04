//! Structured, model-facing error contract for brokered Code Mode tool calls.
//!
//! MCP tool execution failures are successful protocol responses carrying
//! `isError: true`. The Code Mode broker converts those results into rejected
//! JavaScript promises, so the rejection payload must preserve enough evidence
//! for model-authored code to diagnose, course-correct, and retry safely.

use labby_runtime::error::ToolError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Where a Code Mode call failure originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeErrorOrigin {
    /// The Code Mode runtime or broker rejected the call.
    CodeMode,
    /// The upstream MCP server returned a completed `isError: true` result.
    ToolExecution,
    /// The Labby-to-upstream transport did not yield a completed MCP result.
    UpstreamTransport,
    /// Input or output validation rejected the call before execution completed.
    Validation,
    /// Authorization, confirmation, or route policy rejected the call.
    Policy,
    /// A Code Mode execution/result budget rejected the call.
    Budget,
}

/// The next recovery move recommended to model-authored code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeRecoveryAction {
    ReviseAndRetry,
    RetryLater,
    Reauthenticate,
    Confirm,
    Rediscover,
    ReduceWork,
    InspectAndEscalate,
    DoNotRetry,
}

/// Whether repeating the exact same call is appropriate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeSameArgumentsRetry {
    Safe,
    Conditional,
    Discouraged,
    Never,
}

/// Conservative side-effect assessment for the failed call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeSideEffectRisk {
    NoneExpected,
    Possible,
    Unknown,
}

/// Structured recovery guidance paired with the human-readable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeRecoveryAdvice {
    pub action: CodeModeRecoveryAction,
    pub same_arguments: CodeModeSameArgumentsRetry,
    pub guidance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// MCP tool annotations that informed retry and side-effect guidance.
///
/// These are hints supplied by the upstream server, not trusted guarantees.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeToolSafetyHints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl CodeModeToolSafetyHints {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.read_only_hint.is_none()
            && self.destructive_hint.is_none()
            && self.idempotent_hint.is_none()
            && self.open_world_hint.is_none()
    }

    #[must_use]
    pub fn exact_retry_is_hint_safe(&self) -> bool {
        self.read_only_hint == Some(true) || self.idempotent_hint == Some(true)
    }
}

/// Sanitized evidence preserved from the upstream MCP tool result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodeModeErrorEvidence {
    /// Sanitized content blocks in their original order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Value>,
    /// Sanitized upstream `structuredContent`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    /// Parsed structured error object recovered from upstream content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed_error: Option<Value>,
    /// Number of content blocks omitted by the evidence cap.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted_content_blocks: usize,
}

impl CodeModeErrorEvidence {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
            && self.structured_content.is_none()
            && self.parsed_error.is_none()
            && self.omitted_content_blocks == 0
    }
}

const fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Stable JSON object carried in `Error.message` for a failed `callTool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeModeCallError {
    /// Version of this model-facing contract. Additive changes keep version 1.
    pub contract_version: u32,
    /// Stable canonical error kind used for control flow.
    pub kind: String,
    /// Human-readable diagnosis. This field must remain useful on its own.
    pub message: String,
    /// Fully-qualified `<namespace>::<tool>` identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub origin: CodeModeErrorOrigin,
    pub recovery: CodeModeRecoveryAdvice,
    pub side_effects: CodeModeSideEffectRisk,
    /// Original upstream-local kind before Labby canonicalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_kind: Option<String>,
    /// Sanitized original failure text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(default, skip_serializing_if = "CodeModeToolSafetyHints::is_empty")]
    pub safety: CodeModeToolSafetyHints,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<CodeModeErrorEvidence>,
}

impl CodeModeCallError {
    #[must_use]
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        let kind = kind.into();
        let message = message.into();
        Self {
            contract_version: 1,
            origin: origin_for_kind(&kind),
            recovery: recovery_for_kind(&kind, &CodeModeToolSafetyHints::default(), None),
            side_effects: side_effects_for_kind(&kind),
            kind,
            message,
            tool: None,
            original_kind: None,
            cause: None,
            safety: CodeModeToolSafetyHints::default(),
            evidence: None,
        }
    }

    /// Build a completed MCP tool-execution error with preserved evidence.
    #[must_use]
    pub fn tool_execution(
        tool: impl Into<String>,
        kind: impl Into<String>,
        original_kind: Option<String>,
        cause: impl Into<String>,
        evidence: CodeModeErrorEvidence,
        safety: CodeModeToolSafetyHints,
        retry_after_ms: Option<u64>,
    ) -> Self {
        let tool = tool.into();
        let kind = kind.into();
        let cause = cause.into();
        let recovery = recovery_for_kind(&kind, &safety, retry_after_ms);
        let side_effects = if safety.read_only_hint == Some(true) {
            CodeModeSideEffectRisk::NoneExpected
        } else {
            CodeModeSideEffectRisk::Possible
        };
        let message = tool_execution_message(&tool, &cause, &recovery, side_effects);
        Self {
            contract_version: 1,
            kind,
            message,
            tool: Some(tool),
            origin: CodeModeErrorOrigin::ToolExecution,
            recovery,
            side_effects,
            original_kind,
            cause: (!cause.is_empty()).then_some(cause),
            safety,
            evidence: (!evidence.is_empty()).then_some(evidence),
        }
    }

    /// Build a Labby-to-upstream transport failure.
    #[must_use]
    pub fn upstream_transport(tool: impl Into<String>, cause: impl Into<String>) -> Self {
        Self::upstream_transport_with_safety(tool, cause, CodeModeToolSafetyHints::default())
    }

    /// Build a transport failure while retaining advisory MCP safety hints.
    #[must_use]
    pub fn upstream_transport_with_safety(
        tool: impl Into<String>,
        cause: impl Into<String>,
        safety: CodeModeToolSafetyHints,
    ) -> Self {
        let tool = tool.into();
        let cause = cause.into();
        let recovery = CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::RetryLater,
            same_arguments: CodeModeSameArgumentsRetry::Conditional,
            guidance: "Retry after the upstream reconnects, but first consider whether the tool may have committed partial side effects.".to_string(),
            retry_after_ms: None,
        };
        let message = format!(
            "Tool `{tool}` did not return a completed MCP result because the upstream transport failed. The tool may have started before the connection closed, so do not repeat a mutating call unchanged unless it is known to be safe.

Upstream transport error:
{cause}"
        );
        Self {
            contract_version: 1,
            kind: "upstream_error".to_string(),
            message,
            tool: Some(tool),
            origin: CodeModeErrorOrigin::UpstreamTransport,
            recovery,
            side_effects: if safety.read_only_hint == Some(true) {
                CodeModeSideEffectRisk::NoneExpected
            } else {
                CodeModeSideEffectRisk::Possible
            },
            original_kind: None,
            cause: (!cause.is_empty()).then_some(cause),
            safety,
            evidence: None,
        }
    }

    #[must_use]
    pub fn with_tool(mut self, tool: impl Into<String>) -> Self {
        if self.tool.is_none() {
            self.tool = Some(tool.into());
        }
        self
    }

    #[must_use]
    pub fn with_origin(mut self, origin: CodeModeErrorOrigin) -> Self {
        self.origin = origin;
        self
    }

    #[must_use]
    pub fn with_side_effects(mut self, side_effects: CodeModeSideEffectRisk) -> Self {
        self.side_effects = side_effects;
        self
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    #[must_use]
    pub fn user_message(&self) -> &str {
        &self.message
    }

    /// Serialize every field except `kind` and `message` for a shared MCP/API
    /// error envelope.
    #[must_use]
    pub fn extra_fields(&self) -> Value {
        let Ok(Value::Object(mut object)) = serde_json::to_value(self) else {
            return Value::Object(Map::new());
        };
        object.remove("kind");
        object.remove("message");
        Value::Object(object)
    }

    #[must_use]
    pub fn into_tool_error(self) -> ToolError {
        ToolError::Sdk {
            sdk_kind: self.kind,
            message: self.message,
        }
    }
}

impl From<ToolError> for CodeModeCallError {
    fn from(error: ToolError) -> Self {
        Self::new(error.kind(), error.user_message())
    }
}

impl std::fmt::Display for CodeModeCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_string(self) {
            Ok(serialized) => f.write_str(&serialized),
            Err(_) => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for CodeModeCallError {}

fn origin_for_kind(kind: &str) -> CodeModeErrorOrigin {
    match kind {
        "missing_param" | "invalid_param" | "validation_failed" | "invalid_code_mode_id" => {
            CodeModeErrorOrigin::Validation
        }
        "forbidden"
        | "permission_denied"
        | "route_scope_denied"
        | "confirmation_required"
        | "auth_failed" => CodeModeErrorOrigin::Policy,
        "budget_exceeded"
        | "call_budget_exceeded"
        | "quota_exceeded"
        | "result_too_large"
        | "artifact_too_large" => CodeModeErrorOrigin::Budget,
        _ => CodeModeErrorOrigin::CodeMode,
    }
}

fn side_effects_for_kind(kind: &str) -> CodeModeSideEffectRisk {
    match origin_for_kind(kind) {
        CodeModeErrorOrigin::Validation
        | CodeModeErrorOrigin::Policy
        | CodeModeErrorOrigin::Budget => CodeModeSideEffectRisk::NoneExpected,
        _ => CodeModeSideEffectRisk::Unknown,
    }
}

fn recovery_for_kind(
    kind: &str,
    safety: &CodeModeToolSafetyHints,
    retry_after_ms: Option<u64>,
) -> CodeModeRecoveryAdvice {
    let exact_retry = if safety.exact_retry_is_hint_safe() {
        CodeModeSameArgumentsRetry::Conditional
    } else {
        CodeModeSameArgumentsRetry::Discouraged
    };
    match kind {
        "missing_param" | "invalid_param" | "validation_failed" | "tool_error"
        | "conflict" => CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::ReviseAndRetry,
            same_arguments: exact_retry,
            guidance: "Inspect the preserved tool evidence, correct the command or parameters, and retry only after changing the call.".to_string(),
            retry_after_ms: None,
        },
        "unknown_tool" | "unknown_action" | "unknown_subaction" | "not_found"
        | "invalid_code_mode_id" | "snippet_not_found" => CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::Rediscover,
            same_arguments: CodeModeSameArgumentsRetry::Never,
            guidance: "Rediscover the available tool, action, or identifier, then call the corrected target.".to_string(),
            retry_after_ms: None,
        },
        "rate_limited" | "queue_saturated" => CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::RetryLater,
            same_arguments: CodeModeSameArgumentsRetry::Conditional,
            guidance: "Wait for the supplied retry interval when present, then retry with bounded concurrency.".to_string(),
            retry_after_ms,
        },
        "timeout" | "network_error" | "upstream_error" | "service_unavailable" => {
            CodeModeRecoveryAdvice {
                action: CodeModeRecoveryAction::RetryLater,
                same_arguments: CodeModeSameArgumentsRetry::Conditional,
                guidance: "Retry after the transient condition clears, but verify whether the previous call may have committed partial effects.".to_string(),
                retry_after_ms,
            }
        }
        "auth_failed" | "oauth_needs_reauth" | "authorization_failed" => {
            CodeModeRecoveryAdvice {
                action: CodeModeRecoveryAction::Reauthenticate,
                same_arguments: CodeModeSameArgumentsRetry::Never,
                guidance: "Repair or refresh authentication before retrying the call.".to_string(),
                retry_after_ms: None,
            }
        }
        "confirmation_required" => CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::Confirm,
            same_arguments: CodeModeSameArgumentsRetry::Never,
            guidance: "Obtain the required user confirmation, then retry through the confirmed path.".to_string(),
            retry_after_ms: None,
        },
        "budget_exceeded" | "call_budget_exceeded" | "quota_exceeded"
        | "result_too_large" | "artifact_too_large" => CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::ReduceWork,
            same_arguments: CodeModeSameArgumentsRetry::Never,
            guidance: "Reduce fan-out or payload size, split the work, or write large results to an artifact before retrying.".to_string(),
            retry_after_ms: None,
        },
        "forbidden" | "permission_denied" | "route_scope_denied" => {
            CodeModeRecoveryAdvice {
                action: CodeModeRecoveryAction::DoNotRetry,
                same_arguments: CodeModeSameArgumentsRetry::Never,
                guidance: "The caller lacks permission for this operation. Use an authorized route or ask the user/operator to grant access.".to_string(),
                retry_after_ms: None,
            }
        }
        "server_error" | "internal_error" | "decode_error" => CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::InspectAndEscalate,
            same_arguments: CodeModeSameArgumentsRetry::Discouraged,
            guidance: "Inspect the preserved evidence and server diagnostics. Escalate if the failure is not explained by the call input.".to_string(),
            retry_after_ms: None,
        },
        _ => CodeModeRecoveryAdvice {
            action: CodeModeRecoveryAction::InspectAndEscalate,
            same_arguments: exact_retry,
            guidance: "Inspect the preserved evidence, adjust the call when possible, and avoid an unchanged retry when side effects are uncertain.".to_string(),
            retry_after_ms,
        },
    }
}

fn tool_execution_message(
    tool: &str,
    cause: &str,
    recovery: &CodeModeRecoveryAdvice,
    side_effects: CodeModeSideEffectRisk,
) -> String {
    let mut message = format!(
        "Tool `{tool}` ran but reported a failure. The MCP request completed successfully, so this is a tool execution failure rather than a gateway transport failure. {}",
        recovery.guidance
    );
    if side_effects == CodeModeSideEffectRisk::Possible {
        message.push_str(
            " Commands or operations completed before the failure may already have changed the target system.",
        );
    }
    if !cause.is_empty() {
        message.push_str(
            "

Original tool error:
",
        );
        message.push_str(cause);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_execution_error_is_actionable_and_preserves_evidence() {
        let error = CodeModeCallError::tool_execution(
            "claude-dookie::Bash",
            "tool_error",
            Some("upstream_error".to_string()),
            "Exit code 7",
            CodeModeErrorEvidence {
                content: vec![serde_json::json!({"type":"text","text":"Exit code 7"})],
                ..CodeModeErrorEvidence::default()
            },
            CodeModeToolSafetyHints::default(),
            None,
        );
        assert_eq!(error.origin, CodeModeErrorOrigin::ToolExecution);
        assert_eq!(error.side_effects, CodeModeSideEffectRisk::Possible);
        assert_eq!(
            error.recovery.action,
            CodeModeRecoveryAction::ReviseAndRetry
        );
        assert!(error.message.contains("claude-dookie::Bash"));
        assert!(
            error
                .message
                .contains("rather than a gateway transport failure")
        );
        assert!(error.message.contains("Exit code 7"));
        assert_eq!(error.original_kind.as_deref(), Some("upstream_error"));
    }

    #[test]
    fn read_only_hint_reduces_side_effect_risk_without_claiming_safe_retry() {
        let error = CodeModeCallError::tool_execution(
            "search::query",
            "tool_error",
            None,
            "bad query",
            CodeModeErrorEvidence::default(),
            CodeModeToolSafetyHints {
                read_only_hint: Some(true),
                ..CodeModeToolSafetyHints::default()
            },
            None,
        );
        assert_eq!(error.side_effects, CodeModeSideEffectRisk::NoneExpected);
        assert_eq!(
            error.recovery.same_arguments,
            CodeModeSameArgumentsRetry::Conditional
        );
    }

    #[test]
    fn extra_fields_omits_shared_envelope_fields() {
        let error = CodeModeCallError::new("invalid_param", "bad input");
        let extra = error.extra_fields();
        assert!(extra.get("kind").is_none());
        assert!(extra.get("message").is_none());
        assert_eq!(extra["origin"], "validation");
        assert_eq!(extra["side_effects"], "none_expected");
    }
}
