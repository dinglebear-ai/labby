//! Versioned, surface-neutral error metadata for model and agent callers.
//!
//! `ToolError` remains Labby's canonical error type. This module supplies the
//! additive contract fields every surface can compute from a stable error kind:
//! origin, recovery advice, unchanged-retry safety, and partial-side-effect risk.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const AGENT_ERROR_CONTRACT_VERSION: u32 = 1;

/// Best-effort patterns for common secret shapes: provider API keys, JWTs,
/// `Bearer` authorization values, PEM private-key blocks, and URL-embedded
/// credentials (`scheme://user:pass@`).
///
/// This is defense-in-depth, not a guarantee. Novel or provider-specific token
/// formats pass through unrecognized, and a PEM body split across lines is only
/// caught from its `-----BEGIN` header onward. Never treat text that survived
/// this filter as safe to echo verbatim; avoid placing secrets in error text in
/// the first place.
static SECRET_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{82}|glpat-[A-Za-z0-9_-]{20}|xox[bp]-[A-Za-z0-9-]+|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+|(?i:bearer)[ \t]+[A-Za-z0-9._~+/-]{8,}=*|-----BEGIN[A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END[A-Z ]*PRIVATE KEY-----|$)|[A-Za-z][A-Za-z0-9+.-]*://[^/\s:@]+:[^/\s@]+@)",
    )
    .expect("agent error secret regex is valid")
});

/// Marker appended by [`sanitize_error_text`] when the length cap or the
/// bounded inspection window dropped input.
pub const SANITIZE_TRUNCATION_MARKER: &str = " …[truncated]";

/// Slice `input` to a bounded inspection window BEFORE the retain / replace /
/// redact passes run, so a multi-megabyte upstream payload cannot force
/// several full-string passes whose output is then discarded by the final
/// character cap (amplification DoS). A `char` is at most four bytes, so a
/// window of `max_len * 4` bytes always contains at least `max_len` characters
/// when the input has them. The cut respects UTF-8 char boundaries.
fn bounded_window(input: &str, max_len: usize) -> &str {
    let cap = max_len.saturating_mul(4);
    if input.len() <= cap {
        return input;
    }
    let mut end = cap;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

#[must_use]
pub fn sanitize_log_text(input: &str, max_len: usize) -> String {
    sanitize_log_line(input, max_len).0
}

/// Shared single-line sanitizer. Returns the sanitized text plus whether the
/// window or the character cap dropped input, so multi-line callers can append
/// a truncation marker without recounting characters.
fn sanitize_log_line(input: &str, max_len: usize) -> (String, bool) {
    let window = bounded_window(input, max_len);
    let mut sanitized = window.to_string();
    sanitized.retain(|ch| {
        !matches!(
            ch,
            '\u{0000}'..='\u{001F}'
                | '\u{007F}'..='\u{009F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
        )
    });
    for marker in ["<system>", "[INST]", "###", "<<"] {
        sanitized = sanitized.replace(marker, "");
    }
    let redacted = redact_secret_like_segments(&sanitized);
    let mut chars = redacted.chars();
    let output: String = chars.by_ref().take(max_len).collect();
    let capped = chars.next().is_some() || window.len() < input.len();
    (output, capped)
}

/// Sanitize multiline model-facing diagnostics while preserving line breaks.
///
/// When the character cap (or the bounded inspection window) drops input, the
/// output ends with [`SANITIZE_TRUNCATION_MARKER`] so downstream readers know
/// evidence was cut rather than complete.
#[must_use]
pub fn sanitize_error_text(input: &str, max_len: usize) -> String {
    let window = bounded_window(input, max_len);
    let mut truncated = window.len() < input.len();
    let mut output = String::new();
    // Running character count instead of an O(n²) `output.chars().count()`
    // recount per line.
    let mut output_chars = 0usize;
    let mut lines = window.lines().peekable();
    let mut first = true;
    while let Some(line) = lines.next() {
        if !first {
            output.push('\n');
            output_chars += 1;
        }
        first = false;
        let (sanitized, capped) = sanitize_log_line(line, max_len);
        truncated |= capped;
        output_chars += sanitized.chars().count();
        output.push_str(&sanitized);
        if output_chars >= max_len {
            truncated |= output_chars > max_len || lines.peek().is_some();
            break;
        }
    }
    let mut output: String = output.chars().take(max_len).collect();
    if truncated {
        output.push_str(SANITIZE_TRUNCATION_MARKER);
    }
    output
}

#[must_use]
pub fn redact_secret_like_segments(input: &str) -> String {
    let after_split = input
        .split_whitespace()
        .map(|segment| {
            let looks_secret = segment.starts_with("sk-")
                || segment.starts_with("ghp_")
                || segment.starts_with("github_pat_")
                || segment.starts_with("glpat-")
                || segment.starts_with("xoxb-")
                || segment.starts_with("xoxp-")
                || segment.starts_with("eyJ");
            if looks_secret {
                "[REDACTED]".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    SECRET_REGEX
        .replace_all(&after_split, "[REDACTED]")
        .into_owned()
}

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
    fn sanitize_small_input_is_unchanged() {
        let input = "line one\nline two: all clear";
        assert_eq!(sanitize_error_text(input, 4096), input);
        assert_eq!(sanitize_log_text("plain text", 4096), "plain text");
    }

    #[test]
    fn sanitize_error_text_caps_multi_megabyte_input_and_marks_truncation() {
        // 5 MiB of secret-free text; before the bounded window landed this ran
        // ~8 full passes over the whole payload before the 8 KiB cap applied.
        let input = "x".repeat(5 * 1024 * 1024);
        let output = sanitize_error_text(&input, 8 * 1024);
        assert!(output.ends_with(SANITIZE_TRUNCATION_MARKER));
        let body = output.trim_end_matches(SANITIZE_TRUNCATION_MARKER);
        assert_eq!(body.chars().count(), 8 * 1024);

        // Multi-line variant: many lines, each within cap, total far above it.
        let input = "line of text\n".repeat(1024 * 1024);
        let output = sanitize_error_text(&input, 8 * 1024);
        assert!(output.ends_with(SANITIZE_TRUNCATION_MARKER));
        assert!(output.chars().count() <= 8 * 1024 + SANITIZE_TRUNCATION_MARKER.chars().count());
    }

    #[test]
    fn sanitize_exact_cap_input_gets_no_marker() {
        let input = "y".repeat(4096);
        let output = sanitize_error_text(&input, 4096);
        assert_eq!(output, input);
    }

    #[test]
    fn redacts_bearer_pem_and_url_credentials() {
        let bearer = redact_secret_like_segments("Authorization: Bearer abcdef1234567890");
        assert!(!bearer.contains("abcdef1234567890"), "{bearer}");
        assert!(bearer.contains("[REDACTED]"));

        let pem = redact_secret_like_segments(
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA7\n-----END RSA PRIVATE KEY-----",
        );
        assert!(!pem.contains("MIIEowIBAAKCAQEA7"), "{pem}");
        assert!(pem.contains("[REDACTED]"));

        let url = redact_secret_like_segments("connect postgres://admin:hunter2@db.local:5432/app");
        assert!(!url.contains("hunter2"), "{url}");
        assert!(url.contains("[REDACTED]"));
        assert!(url.contains("db.local"));
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
