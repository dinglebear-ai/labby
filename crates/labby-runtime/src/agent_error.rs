//! Versioned, surface-neutral error metadata for model and agent callers.
//!
//! `ToolError` remains Labby's canonical error type. This module supplies the
//! additive contract fields every surface can compute from a stable error kind:
//! origin, recovery advice, unchanged-retry safety, and partial-side-effect risk.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const AGENT_ERROR_CONTRACT_VERSION: u32 = 1;

// The sanitize/secret helpers moved to `crate::redact` (the charter home for
// redaction). Re-exported here so existing `agent_error::…` imports keep
// working. Pure module-placement move — zero behavior change beyond the
// documented `tskey-` broadening.
pub use crate::redact::{
    SANITIZE_TRUNCATION_MARKER, redact_secret_like_segments, sanitize_error_text, sanitize_log_text,
};

/// MCP tool annotations that informed retry and side-effect guidance.
///
/// These are advisory hints supplied by the upstream server, not trusted
/// guarantees. This is the single canonical definition — the gateway's
/// `McpToolSafetyHints` and Code Mode's `CodeModeToolSafetyHints` are type
/// aliases of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSafetyHints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl ToolSafetyHints {
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

/// Sanitized evidence preserved from a completed upstream MCP tool result.
///
/// Single canonical definition — the gateway's `McpToolErrorEvidence` and Code
/// Mode's `CodeModeErrorEvidence` are type aliases of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolErrorEvidence {
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

impl ToolErrorEvidence {
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

/// Canonical model-facing message for a completed MCP tool-execution failure.
///
/// Shared by the gateway analyzer and Code Mode so their wording cannot drift.
/// `cause` must already be sanitized by the caller.
#[must_use]
pub fn tool_execution_message(
    tool: &str,
    cause: &str,
    guidance: &str,
    side_effects: AgentSideEffectRisk,
) -> String {
    let mut message = format!(
        "Tool `{tool}` ran but reported a failure. The MCP request completed successfully, so this is a tool execution failure rather than a gateway transport failure. {guidance}"
    );
    if side_effects == AgentSideEffectRisk::Possible {
        message.push_str(
            " Operations completed before the failure may already have changed the target system.",
        );
    }
    if !cause.is_empty() {
        message.push_str("\n\nOriginal tool error:\n");
        message.push_str(cause);
    }
    message
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorOrigin {
    Runtime,
    CodeMode,
    ToolExecution,
    UpstreamTransport,
    Validation,
    Policy,
    Budget,
    Discovery,
    Bridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRecoveryAction {
    ReviseAndRetry,
    RetryLater,
    Reauthenticate,
    Confirm,
    Rediscover,
    ReduceWork,
    StartDependency,
    InspectAndEscalate,
    DoNotRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSameArgumentsRetry {
    Safe,
    Conditional,
    Discouraged,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSideEffectRisk {
    NoneExpected,
    Possible,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecoveryAdvice {
    pub action: AgentRecoveryAction,
    pub same_arguments: AgentSameArgumentsRetry,
    pub guidance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentErrorMetadata {
    pub contract_version: u32,
    pub origin: AgentErrorOrigin,
    pub recovery: AgentRecoveryAdvice,
    pub side_effects: AgentSideEffectRisk,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentErrorContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentErrorOrigin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<AgentRecoveryAdvice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side_effects: Option<AgentSideEffectRisk>,
}

impl AgentErrorContext {
    #[must_use]
    pub fn for_service_action(service: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            service: Some(service.into()),
            action: Some(action.into()),
            ..Self::default()
        }
    }
}

#[must_use]
pub fn metadata_for_kind(kind: &str, retry_after_ms: Option<u64>) -> AgentErrorMetadata {
    metadata_for_kind_with_retry_safety(kind, retry_after_ms, false)
}

#[must_use]
pub fn metadata_for_kind_with_retry_safety(
    kind: &str,
    retry_after_ms: Option<u64>,
    exact_retry_hint_safe: bool,
) -> AgentErrorMetadata {
    AgentErrorMetadata {
        contract_version: AGENT_ERROR_CONTRACT_VERSION,
        origin: origin_for_kind(kind),
        recovery: recovery_for_kind(kind, retry_after_ms, exact_retry_hint_safe),
        side_effects: side_effects_for_kind(kind),
    }
}

#[must_use]
pub fn build_agent_error_value(
    kind: &str,
    message: &str,
    extra: Option<&Value>,
    context: &AgentErrorContext,
) -> Value {
    let retry_after_ms = extra
        .and_then(Value::as_object)
        .and_then(retry_after_ms_from_object);
    let metadata = metadata_for_kind(kind, retry_after_ms);
    let mut object = extra
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    object.insert(
        "contract_version".to_string(),
        json!(metadata.contract_version),
    );
    object.insert("kind".to_string(), json!(kind));
    object.insert("message".to_string(), json!(message));
    object.insert(
        "origin".to_string(),
        json!(context.origin.unwrap_or(metadata.origin)),
    );
    object.insert(
        "recovery".to_string(),
        json!(context.recovery.as_ref().unwrap_or(&metadata.recovery)),
    );
    object.insert(
        "side_effects".to_string(),
        json!(context.side_effects.unwrap_or(metadata.side_effects)),
    );

    insert_optional(&mut object, "service", context.service.as_deref());
    insert_optional(&mut object, "action", context.action.as_deref());
    insert_optional(&mut object, "tool", context.tool.as_deref());
    insert_optional(&mut object, "upstream", context.upstream.as_deref());
    insert_optional(&mut object, "command", context.command.as_deref());
    insert_optional(&mut object, "prompt", context.prompt.as_deref());
    insert_optional(&mut object, "resource", context.resource.as_deref());
    insert_optional(&mut object, "cause", context.cause.as_deref());

    Value::Object(object)
}

fn insert_optional(object: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        object.insert(key.to_string(), Value::String(value.to_string()));
    }
}

/// Read a retry hint from a structured error object, accepting both snake_case
/// and camelCase spellings. Canonical helper shared by every surface that
/// inspects upstream error objects.
#[must_use]
pub fn retry_after_ms_from_object(object: &Map<String, Value>) -> Option<u64> {
    object
        .get("retry_after_ms")
        .or_else(|| object.get("retryAfterMs"))
        .and_then(Value::as_u64)
}

#[must_use]
pub fn origin_for_kind(kind: &str) -> AgentErrorOrigin {
    match kind {
        // `conflict` fails against current state before any mutation commits,
        // so it classifies with the fix-the-request family rather than the
        // runtime catch-all.
        "missing_param"
        | "invalid_param"
        | "validation_failed"
        | "invalid_hint"
        | "conflict"
        | "path_traversal"
        | "symlink_rejected"
        | "invalid_encoding"
        | "ssrf_blocked"
        | "content_too_large"
        | "relay_invalid_target"
        | "invalid_code_mode_id" => AgentErrorOrigin::Validation,
        "forbidden"
        | "permission_denied"
        | "confirmation_required"
        | "auth_failed"
        | "auth_required"
        | "oauth_needs_reauth"
        | "route_scope_denied" => AgentErrorOrigin::Policy,
        "rate_limited"
        | "queue_saturated"
        | "quota_exceeded"
        | "budget_exceeded"
        | "call_budget_exceeded"
        | "result_too_large"
        | "artifact_too_large"
        | "snippet_budget_exceeded"
        | "snippet_resolve_limit" => AgentErrorOrigin::Budget,
        "unknown_action" | "unknown_subaction" | "unknown_tool" | "unknown_upstream"
        | "unknown_instance" | "ambiguous_tool" | "not_found" | "snippet_not_found" => {
            AgentErrorOrigin::Discovery
        }
        "tool_error" => AgentErrorOrigin::ToolExecution,
        // `timeout` means no completed result arrived from the dependency or
        // sandbox; treat it as transport-family so side-effect guidance stays
        // conservative (`possible`) instead of the runtime catch-all.
        "upstream_error"
        | "network_error"
        | "timeout"
        | "bad_gateway"
        | "service_unavailable"
        | "provider_unavailable"
        | "provider_timeout"
        | "not_connected"
        | "connection_error"
        | "relay_forwarder_init_failed" => AgentErrorOrigin::UpstreamTransport,
        "bridge_transport_error" => AgentErrorOrigin::Bridge,
        _ => AgentErrorOrigin::Runtime,
    }
}

#[must_use]
pub fn side_effects_for_kind(kind: &str) -> AgentSideEffectRisk {
    match origin_for_kind(kind) {
        AgentErrorOrigin::Validation
        | AgentErrorOrigin::Policy
        | AgentErrorOrigin::Budget
        | AgentErrorOrigin::Discovery => AgentSideEffectRisk::NoneExpected,
        AgentErrorOrigin::ToolExecution | AgentErrorOrigin::UpstreamTransport => {
            AgentSideEffectRisk::Possible
        }
        AgentErrorOrigin::Runtime | AgentErrorOrigin::CodeMode | AgentErrorOrigin::Bridge => {
            AgentSideEffectRisk::Unknown
        }
    }
}

#[must_use]
pub fn recovery_for_kind(
    kind: &str,
    retry_after_ms: Option<u64>,
    exact_retry_hint_safe: bool,
) -> AgentRecoveryAdvice {
    let revised_retry = if exact_retry_hint_safe {
        AgentSameArgumentsRetry::Conditional
    } else {
        AgentSameArgumentsRetry::Discouraged
    };
    match kind {
        "missing_param" | "invalid_param" | "validation_failed" | "invalid_hint"
        | "conflict" | "tool_error" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReviseAndRetry,
            same_arguments: revised_retry,
            guidance: "Inspect the error details, correct the command or parameters, and retry only after changing the call.".to_string(),
            retry_after_ms: None,
        },
        "unknown_action" | "unknown_subaction" | "unknown_tool" | "unknown_upstream"
        | "unknown_instance" | "ambiguous_tool" | "not_found" | "snippet_not_found"
        | "invalid_code_mode_id" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Rediscover,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "List or search the available actions, tools, prompts, or resources, then retry with a valid identifier.".to_string(),
            retry_after_ms: None,
        },
        "rate_limited" | "queue_saturated" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::RetryLater,
            same_arguments: AgentSameArgumentsRetry::Conditional,
            guidance: "Wait for the supplied retry interval when present, reduce concurrency, and retry after the limit clears.".to_string(),
            retry_after_ms,
        },
        "timeout" | "network_error" | "upstream_error" | "bad_gateway"
        | "service_unavailable" | "provider_unavailable" | "provider_timeout"
        | "not_connected" | "connection_error" | "relay_forwarder_init_failed" => {
            AgentRecoveryAdvice {
                action: AgentRecoveryAction::RetryLater,
                same_arguments: AgentSameArgumentsRetry::Conditional,
                guidance: "Retry after the dependency or transport recovers, but first verify whether the previous call may have committed partial effects.".to_string(),
                retry_after_ms,
            }
        }
        "auth_failed" | "auth_required" | "oauth_needs_reauth" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Reauthenticate,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Repair or refresh authentication before retrying. For an upstream OAuth server, use the gateway OAuth start action for that upstream.".to_string(),
            retry_after_ms: None,
        },
        "confirmation_required" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Confirm,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Obtain explicit user confirmation and retry through the confirmed destructive-action path.".to_string(),
            retry_after_ms: None,
        },
        "budget_exceeded" | "call_budget_exceeded" | "quota_exceeded"
        | "result_too_large" | "artifact_too_large" | "content_too_large"
        | "snippet_budget_exceeded" | "snippet_resolve_limit" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReduceWork,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Reduce fan-out or payload size, split the work, or use an artifact before retrying.".to_string(),
            retry_after_ms: None,
        },
        "forbidden" | "permission_denied" | "route_scope_denied" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::DoNotRetry,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "The caller lacks permission for this operation. Use an authorized route or ask the user or operator to grant access.".to_string(),
            retry_after_ms: None,
        },
        "bridge_transport_error" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::StartDependency,
            same_arguments: AgentSameArgumentsRetry::Conditional,
            guidance: "Start or restart the canonical Labby daemon (`labby serve`), verify it is ready, then retry. Check whether the forwarded operation may have reached the daemon before the bridge disconnected.".to_string(),
            retry_after_ms: None,
        },
        "internal_error" | "server_error" | "decode_error" | "invalid_provider_output" => {
            AgentRecoveryAdvice {
                action: AgentRecoveryAction::InspectAndEscalate,
                same_arguments: AgentSameArgumentsRetry::Discouraged,
                guidance: "Inspect server diagnostics and preserved evidence. Escalate if the failure is not explained by the request input.".to_string(),
                retry_after_ms: None,
            }
        }
        "cancelled" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::DoNotRetry,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "The request was cancelled. Retry only when the caller still wants the operation and partial effects have been checked.".to_string(),
            retry_after_ms: None,
        },
        _ => AgentRecoveryAdvice {
            action: AgentRecoveryAction::InspectAndEscalate,
            same_arguments: revised_retry,
            guidance: "Inspect the error details, adjust the request when possible, and avoid an unchanged retry when side effects are uncertain.".to_string(),
            retry_after_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_is_fixable_without_side_effects() {
        let metadata = metadata_for_kind("invalid_param", None);
        assert_eq!(metadata.origin, AgentErrorOrigin::Validation);
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::NoneExpected);
        assert_eq!(
            metadata.recovery.action,
            AgentRecoveryAction::ReviseAndRetry
        );
    }

    #[test]
    fn upstream_error_warns_about_partial_effects() {
        let metadata = metadata_for_kind("upstream_error", None);
        assert_eq!(metadata.origin, AgentErrorOrigin::UpstreamTransport);
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::Possible);
        assert_eq!(
            metadata.recovery.same_arguments,
            AgentSameArgumentsRetry::Conditional
        );
    }

    #[test]
    fn timeout_classifies_as_transport_with_possible_side_effects() {
        let metadata = metadata_for_kind("timeout", None);
        assert_eq!(metadata.origin, AgentErrorOrigin::UpstreamTransport);
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::Possible);
        assert_eq!(metadata.recovery.action, AgentRecoveryAction::RetryLater);
    }

    #[test]
    fn conflict_classifies_as_validation_without_side_effects() {
        let metadata = metadata_for_kind("conflict", None);
        assert_eq!(metadata.origin, AgentErrorOrigin::Validation);
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::NoneExpected);
        assert_eq!(
            metadata.recovery.action,
            AgentRecoveryAction::ReviseAndRetry
        );
    }

    #[test]
    fn context_fields_are_additive_and_reserved_fields_win() {
        let value = build_agent_error_value(
            "missing_param",
            "missing query",
            Some(&json!({"param":"query","kind":"wrong"})),
            &AgentErrorContext::for_service_action("search", "query"),
        );
        assert_eq!(value["kind"], "missing_param");
        assert_eq!(value["service"], "search");
        assert_eq!(value["action"], "query");
        assert_eq!(value["param"], "query");
        assert_eq!(value["contract_version"], 1);
    }
}
