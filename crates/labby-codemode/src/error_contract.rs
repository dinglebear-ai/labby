//! Structured, model-facing error contract for brokered Code Mode tool calls.
//!
//! MCP tool execution failures are successful protocol responses carrying
//! `isError: true`. The Code Mode broker converts those results into rejected
//! JavaScript promises, so the rejection payload must preserve enough evidence
//! for model-authored code to diagnose, course-correct, and retry safely.

use labby_runtime::agent_error::{
    AGENT_ERROR_CONTRACT_VERSION, AgentErrorOrigin, AgentRecoveryAction, AgentRecoveryAdvice,
    AgentSameArgumentsRetry, AgentSideEffectRisk, origin_for_kind as shared_origin_for_kind,
    recovery_for_kind as shared_recovery_for_kind,
};
use labby_runtime::error::ToolError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub type CodeModeErrorOrigin = AgentErrorOrigin;
pub type CodeModeRecoveryAction = AgentRecoveryAction;
pub type CodeModeSameArgumentsRetry = AgentSameArgumentsRetry;
pub type CodeModeSideEffectRisk = AgentSideEffectRisk;
pub type CodeModeRecoveryAdvice = AgentRecoveryAdvice;

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
#[derive(Debug, Clone, PartialEq, Serialize)]
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

impl<'de> Deserialize<'de> for CodeModeCallError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireError {
            #[serde(default)]
            contract_version: Option<u32>,
            kind: String,
            message: String,
            #[serde(default)]
            tool: Option<String>,
            #[serde(default)]
            origin: Option<CodeModeErrorOrigin>,
            #[serde(default)]
            recovery: Option<CodeModeRecoveryAdvice>,
            #[serde(default)]
            side_effects: Option<CodeModeSideEffectRisk>,
            #[serde(default)]
            original_kind: Option<String>,
            #[serde(default)]
            cause: Option<String>,
            #[serde(default)]
            safety: CodeModeToolSafetyHints,
            #[serde(default)]
            evidence: Option<CodeModeErrorEvidence>,
        }

        let wire = WireError::deserialize(deserializer)?;
        let recovery = wire
            .recovery
            .unwrap_or_else(|| recovery_for_kind(&wire.kind, &wire.safety, None));
        let origin = wire.origin.unwrap_or_else(|| origin_for_kind(&wire.kind));
        let side_effects = wire
            .side_effects
            .unwrap_or_else(|| side_effects_for_kind(&wire.kind));

        Ok(Self {
            contract_version: wire
                .contract_version
                .unwrap_or(AGENT_ERROR_CONTRACT_VERSION),
            kind: wire.kind,
            message: wire.message,
            tool: wire.tool,
            origin,
            recovery,
            side_effects,
            original_kind: wire.original_kind,
            cause: wire.cause,
            safety: wire.safety,
            evidence: wire.evidence,
        })
    }
}

impl CodeModeCallError {
    #[must_use]
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        let kind = kind.into();
        let message = message.into();
        Self {
            contract_version: AGENT_ERROR_CONTRACT_VERSION,
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
            contract_version: AGENT_ERROR_CONTRACT_VERSION,
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
            contract_version: AGENT_ERROR_CONTRACT_VERSION,
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
    match shared_origin_for_kind(kind) {
        CodeModeErrorOrigin::Runtime
        | CodeModeErrorOrigin::Discovery
        | CodeModeErrorOrigin::Bridge => CodeModeErrorOrigin::CodeMode,
        origin => origin,
    }
}

fn side_effects_for_kind(kind: &str) -> CodeModeSideEffectRisk {
    match origin_for_kind(kind) {
        CodeModeErrorOrigin::Validation
        | CodeModeErrorOrigin::Policy
        | CodeModeErrorOrigin::Budget
        | CodeModeErrorOrigin::Discovery => CodeModeSideEffectRisk::NoneExpected,
        CodeModeErrorOrigin::ToolExecution | CodeModeErrorOrigin::UpstreamTransport => {
            CodeModeSideEffectRisk::Possible
        }
        _ => CodeModeSideEffectRisk::Unknown,
    }
}

fn recovery_for_kind(
    kind: &str,
    safety: &CodeModeToolSafetyHints,
    retry_after_ms: Option<u64>,
) -> CodeModeRecoveryAdvice {
    shared_recovery_for_kind(kind, retry_after_ms, safety.exact_retry_is_hint_safe())
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
    fn legacy_kind_message_payload_upgrades_to_current_contract() {
        let error: CodeModeCallError = serde_json::from_value(serde_json::json!({
            "kind": "tool_error",
            "message": "legacy failure"
        }))
        .expect("legacy error must remain compatible");

        assert_eq!(error.contract_version, AGENT_ERROR_CONTRACT_VERSION);
        assert_eq!(error.kind, "tool_error");
        assert_eq!(error.message, "legacy failure");
        assert_eq!(error.origin, CodeModeErrorOrigin::ToolExecution);
        assert_eq!(error.side_effects, CodeModeSideEffectRisk::Possible);
        assert_eq!(
            error.recovery.action,
            CodeModeRecoveryAction::ReviseAndRetry
        );
        assert_eq!(
            error.recovery.same_arguments,
            CodeModeSameArgumentsRetry::Discouraged
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
