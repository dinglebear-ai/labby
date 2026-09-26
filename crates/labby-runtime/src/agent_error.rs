//! Versioned, surface-neutral error metadata for model and agent callers.
//!
//! `ToolError` remains Labby's canonical error type. This module supplies the
//! additive contract fields every surface can compute from a stable error kind:
//! origin, recovery advice, unchanged-retry safety, and partial-side-effect risk.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Version number embedded in every structured agent-error envelope.
pub const AGENT_ERROR_CONTRACT_VERSION: u32 = 1;

/// Canonical operator commands for completing or removing a pending owner
/// access bootstrap. Error producers compose these constants so recovery text
/// cannot drift from the current CLI hierarchy.
pub const ACCESS_BOOTSTRAP_CONSUME_COMMAND: &str = "labby auth bootstrap consume";
pub const ACCESS_BOOTSTRAP_CLEANUP_COMMAND: &str = "labby auth bootstrap cleanup";

/// Canonical owner-setup guidance shared by surface-specific setup errors.
pub const ACCESS_SETUP_OPERATOR_GUIDANCE: &str = "Ask the operator of the Labby server to complete owner setup: installs with any OAuth provider (including bearer plus OAuth) complete browser owner setup in the Labby web UI, which takes effect without a restart; bearer-token-only installs run `labby setup` on the Labby server host and then restart the serving Labby process, because a running Labby only re-reads access setup at startup.";

/// Canonical pending-bootstrap guidance, optionally including surface-specific
/// arguments such as a known prepare id.
#[must_use]
pub fn pending_access_bootstrap_operator_guidance(command_arguments: &str) -> String {
    format!(
        "Ask the operator of the Labby server to finish any pending owner access bootstrap with `{ACCESS_BOOTSTRAP_CONSUME_COMMAND}{command_arguments}` or remove it with `{ACCESS_BOOTSTRAP_CLEANUP_COMMAND}{command_arguments}` while Labby is stopped."
    )
}

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSafetyHints {
    /// Upstream claim that invoking the tool does not mutate state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// Upstream claim that the tool may perform destructive operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// Upstream claim that repeating the same call is idempotent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    /// Upstream claim that the tool interacts with an open-ended external world.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl ToolSafetyHints {
    /// Return `true` when the upstream supplied no safety annotations.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.read_only_hint.is_none()
            && self.destructive_hint.is_none()
            && self.idempotent_hint.is_none()
            && self.open_world_hint.is_none()
    }

    /// Return whether the upstream hints make an exact retry plausibly safe.
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
    /// Return `true` when no sanitized upstream evidence was retained.
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

/// Subsystem family in which an agent-facing error originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorOrigin {
    /// Generic Labby runtime failure.
    Runtime,
    /// Code Mode execution or sandbox failure.
    CodeMode,
    /// Upstream tool completed but reported a tool-level failure.
    ToolExecution,
    /// Network or protocol transport failure talking to an upstream.
    UpstreamTransport,
    /// Caller input or validation failure.
    Validation,
    /// Authorization, confirmation, or policy rejection.
    Policy,
    /// Bounded resource, quota, or size limit failure.
    Budget,
    /// Lookup or identifier discovery failure.
    Discovery,
    /// Inter-layer bridge or relay failure.
    Bridge,
}

/// High-level recovery action recommended to an agent caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRecoveryAction {
    /// Change the request, then retry.
    ReviseAndRetry,
    /// Wait for a transient condition to clear before retrying.
    RetryLater,
    /// Repair or refresh authentication before retrying.
    Reauthenticate,
    /// Obtain explicit user confirmation before retrying.
    Confirm,
    /// Refresh discovery/catalog state and choose a valid target.
    Rediscover,
    /// Reduce payload size, fan-out, or resource consumption.
    ReduceWork,
    /// Start or repair a required dependency.
    StartDependency,
    /// Inspect evidence and escalate instead of blindly retrying.
    InspectAndEscalate,
    /// Do not retry this operation.
    DoNotRetry,
}

/// Safety classification for repeating the exact same request arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentSameArgumentsRetry {
    /// Exact retry is expected to be safe.
    Safe,
    /// Exact retry is safe only after a stated condition changes.
    Conditional,
    /// Prefer revising or inspecting before an exact retry.
    Discouraged,
    /// Never repeat the same request unchanged.
    Never,
}

/// Whether a failed operation may already have produced side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentSideEffectRisk {
    /// No side effects are expected to have committed.
    NoneExpected,
    /// Some side effects may have committed before the failure.
    Possible,
    /// The subsystem cannot determine whether side effects occurred.
    Unknown,
}

/// Structured recovery guidance attached to an agent-facing error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRecoveryAdvice {
    /// Recommended high-level recovery action.
    pub action: AgentRecoveryAction,
    /// Whether the exact same arguments may be retried.
    pub same_arguments: AgentSameArgumentsRetry,
    /// Human-readable recovery guidance for an agent or operator.
    pub guidance: String,
    /// Optional minimum retry delay in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// Stable metadata derived from an error kind before request context is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentErrorMetadata {
    /// Agent-error contract version.
    pub contract_version: u32,
    /// Subsystem family that produced the error.
    pub origin: AgentErrorOrigin,
    /// Recommended recovery behavior.
    pub recovery: AgentRecoveryAdvice,
    /// Estimated side-effect risk for the failed operation.
    pub side_effects: AgentSideEffectRisk,
}

/// Optional request and subsystem context merged into an agent-error envelope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentErrorContext {
    /// Built-in service associated with the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Service action associated with the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// MCP or Code Mode tool identifier associated with the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Upstream gateway name associated with the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Sanitized command identifier associated with the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Prompt identifier associated with the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Resource URI associated with the failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// Sanitized underlying cause text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// Explicit origin override supplied by the producing subsystem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentErrorOrigin>,
    /// Explicit recovery override supplied by the producing subsystem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<AgentRecoveryAdvice>,
    /// Explicit side-effect-risk override supplied by the producing subsystem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side_effects: Option<AgentSideEffectRisk>,
}

impl AgentErrorContext {
    /// Construct context identifying a built-in service action.
    #[must_use]
    pub fn for_service_action(service: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            service: Some(service.into()),
            action: Some(action.into()),
            ..Self::default()
        }
    }
}

/// Derive canonical agent metadata from a stable error kind.
#[must_use]
pub fn metadata_for_kind(kind: &str, retry_after_ms: Option<u64>) -> AgentErrorMetadata {
    metadata_for_kind_with_retry_safety(kind, retry_after_ms, false)
}

/// Derive agent metadata while incorporating trusted exact-retry safety context.
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

/// Build the additive JSON error envelope shared by CLI, HTTP, MCP, and Code Mode.
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

/// Classify a stable error kind into its canonical subsystem origin.
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
        | "stale_suggestion"
        | "merge_write_conflict"
        | "workspace_not_configured"
        // The durable access store was never initialized: a deterministic
        // setup gate evaluated before any dispatch, not a transport outage.
        | "access_setup_required"
        | "restart_required"
        | "oauth_account_ambiguous"
        | "oauth_client_mismatch"
        | "path_traversal"
        | "symlink_rejected"
        | "invalid_encoding"
        | "ssrf_blocked"
        | "content_too_large"
        | "relay_invalid_target"
        // Skills verification failures (SEP-2640): Labby rejected the content
        // before returning any of it, so nothing committed and the caller's
        // request — not the upstream transport — is what has to change.
        | "skill_digest_mismatch"
        | "skill_manifest_stale"
        | "invalid_code_mode_id" => AgentErrorOrigin::Validation,
        "forbidden"
        | "permission_denied"
        | "confirmation_required"
        | "auth_failed"
        | "auth_required"
        | "upstream_credential_missing"
        | "oauth_state_invalid"
        | "oauth_resource_mismatch"
        | "oauth_issuer_mismatch"
        | "oauth_unsupported_method"
        | "oauth_needs_reauth"
        | "oauth_scope_upgrade_required"
        | "oauth_shared_credential_protected"
        | "route_scope_denied"
        // Membership, policy, or the execution lease moved under a running
        // Agent session or Task attempt.
        | "authority_changed" => AgentErrorOrigin::Policy,
        // A capability the operator did not build in. Policy rather than
        // discovery: the action is genuinely unavailable here, and no amount of
        // rediscovery changes that.
        "feature_not_compiled" => AgentErrorOrigin::Policy,
        "rate_limited"
        | "queue_saturated"
        | "quota_exceeded"
        | "budget_exceeded"
        | "call_budget_exceeded"
        | "result_too_large"
        | "artifact_too_large"
        // `response_too_large` predates this table and was never registered,
        // so it silently classified as the runtime catch-all. It belongs with
        // the payload-limit family.
        | "response_too_large"
        | "snippet_budget_exceeded"
        | "snippet_resolve_limit" => AgentErrorOrigin::Budget,
        "unknown_action" | "unknown_subaction" | "unknown_tool" | "unknown_upstream"
        | "unknown_instance" | "ambiguous_tool" | "not_found" | "snippet_not_found"
        // A path that resolves to no registered route is a lookup failure, not
        // the `Runtime` catch-all it would otherwise land in.
        | "route_not_found" => AgentErrorOrigin::Discovery,
        "invalid_cursor" => AgentErrorOrigin::Discovery,
        "tool_error" => AgentErrorOrigin::ToolExecution,
        // `timeout` means no completed result arrived from the dependency or
        // sandbox; treat it as transport-family so side-effect guidance stays
        // conservative (`possible`) instead of the runtime catch-all.
        "upstream_error"
        | "network_error"
        | "timeout"
        | "bad_gateway"
        | "service_unavailable"
        | "runtime_unavailable"
        | "provider_error"
        | "provider_unavailable"
        | "provider_timeout"
        | "not_connected"
        | "connection_error"
        | "connection_refused"
        | "dns_error"
        | "relay_forwarder_init_failed"
        // Agent execution: the OpenAI-compatible provider (or the pinned
        // payload it depends on) is unreachable, or it answered with bytes
        // that do not satisfy the expected contract. Neither is a completed
        // provider result.
        | "unavailable"
        | "protocol_error" => AgentErrorOrigin::UpstreamTransport,
        "bridge_transport_error" => AgentErrorOrigin::Bridge,
        _ => AgentErrorOrigin::Runtime,
    }
}

/// Estimate side-effect risk from the canonical origin of an error kind.
#[must_use]
pub fn side_effects_for_kind(kind: &str) -> AgentSideEffectRisk {
    // These limits can reject a result or a later step after work committed.
    // Budget origin alone does not establish pre-execution rejection.
    // `authority_changed` is a policy outcome observed mid-execution: the run
    // may already have reached its provider before the fence closed.
    if matches!(
        kind,
        "result_too_large"
            | "response_too_large"
            | "budget_exceeded"
            | "call_budget_exceeded"
            | "authority_changed"
    ) {
        return AgentSideEffectRisk::Possible;
    }
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

/// Derive canonical recovery guidance for a stable error kind.
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
        | "conflict" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReviseAndRetry,
            same_arguments: revised_retry,
            guidance: "Inspect the error details, correct the command or parameters, and retry only after changing the call.".to_string(),
            retry_after_ms: None,
        },
        "tool_error" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReviseAndRetry,
            same_arguments: revised_retry,
            guidance: "Inspect the preserved upstream error and current operation status. Check whether partial effects committed before revising the command or parameters; resume only unfinished work. Consult existing operation records and reuse an idempotency key only for the same operation and arguments when supported.".to_string(),
            retry_after_ms: None,
        },
        "invalid_cursor" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Rediscover,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Restart the same listing without a cursor, keeping its filters and scope consistent, then use only the next cursor returned by that listing. Do not invent or reuse an expired cursor; deduplicate items already processed.".to_string(),
            retry_after_ms: None,
        },
        // An HTTP route that is not registered at all. Distinct from
        // `not_found` because `Rediscover` is inert here to the point of being
        // misleading: no amount of listing or searching surfaces a route the
        // server never mounted. The usual cause is a feature slice or auth
        // prerequisite that was not met at startup, which is reported there.
        // The guidance must not name which service is missing — this kind is
        // reachable unauthenticated.
        "route_not_found" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReviseAndRetry,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Verify the request path and method. If the path is correct, the owning feature may not be mounted on this server; check the server startup logs for a skipped-service warning.".to_string(),
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
        | "service_unavailable" | "runtime_unavailable" | "provider_error"
        | "provider_unavailable" | "provider_timeout" | "unavailable"
        | "not_connected" | "connection_error" | "connection_refused" | "dns_error"
        | "relay_forwarder_init_failed" => {
            AgentRecoveryAdvice {
                action: AgentRecoveryAction::RetryLater,
                same_arguments: AgentSameArgumentsRetry::Conditional,
                guidance: "Retry after the dependency or transport recovers, but first verify whether the previous call may have committed partial effects.".to_string(),
                retry_after_ms,
            }
        }
        "upstream_credential_missing" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::InspectAndEscalate,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Have the operator restore the configured upstream credential in the selected installation, reload the upstream, and verify its connection before retrying. Do not remove the credential reference, disable authentication, or retry anonymously to bypass this failure.".to_string(),
            retry_after_ms: None,
        },
        "auth_failed" | "auth_required" | "oauth_needs_reauth" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Reauthenticate,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Repair or refresh authentication before retrying. For an upstream OAuth server, use the gateway OAuth start action for that upstream.".to_string(),
            retry_after_ms: None,
        },
        "oauth_state_invalid" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Reauthenticate,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Start a fresh OAuth authorization flow for the same upstream from an authenticated browser session. Complete the new flow in that session; do not reuse a callback URL, authorization code, or expired state from the failed flow.".to_string(),
            retry_after_ms: None,
        },
        "oauth_resource_mismatch" | "oauth_issuer_mismatch" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::InspectAndEscalate,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Compare the configured upstream resource and expected issuer with the server's discovery metadata. Have the operator correct the configuration or server metadata before starting a fresh OAuth flow. Do not bypass issuer or resource validation or send credentials to a different endpoint to make the retry succeed.".to_string(),
            retry_after_ms: None,
        },
        "oauth_unsupported_method" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::InspectAndEscalate,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Inspect the reported unsupported OAuth method and the authorization server's discovery metadata. The upstream must advertise and support S256 PKCE; have the operator correct or upgrade the server before restarting authorization. Do not downgrade to plain PKCE or disable verification.".to_string(),
            retry_after_ms: None,
        },
        "stale_suggestion" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Rediscover,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Refresh the upstream metadata and generate a new enrichment suggestion. Review it against the current metadata hash before applying; do not replace the hash on an old suggestion just to bypass the stale-state check.".to_string(),
            retry_after_ms: None,
        },
        "merge_write_conflict" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReviseAndRetry,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "The configuration changed since the merge was prepared. Re-read the current configuration and regenerate and review the merge before applying it. Preserve concurrent edits; do not force the stale draft over the current file.".to_string(),
            retry_after_ms: None,
        },
        "workspace_not_configured" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReviseAndRetry,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Have the operator set workspace.root to the intended existing directory on the Labby server and restart the serving process so it resolves the new root. Verify workspace access after startup before retrying; do not substitute an unrelated directory.".to_string(),
            retry_after_ms: None,
        },
        // Not `retry_later`: the store stays uninitialized until an operator
        // completes owner setup, so an unchanged retry can never succeed.
        "access_setup_required" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::StartDependency,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: format!(
                "Labby's durable access store is not set up, so no authorization decision can be made and nothing ran. Do not run setup yourself. {ACCESS_SETUP_OPERATOR_GUIDANCE} {} Retry only after setup succeeds; do not retry unchanged before then.",
                pending_access_bootstrap_operator_guidance("")
            ),
            retry_after_ms: None,
        },
        "restart_required" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::StartDependency,
            same_arguments: AgentSameArgumentsRetry::Conditional,
            guidance: "Inspect the reported protected-route or loadout dependency and any staged configuration. Apply the required configuration change through its supported staged action, then coordinate a Labby service restart with the operator. Verify the active configuration after restart before retrying; gateway reload does not replace startup-mounted routes.".to_string(),
            retry_after_ms: None,
        },
        "oauth_scope_upgrade_required" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Reauthenticate,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Start the gateway OAuth flow for this upstream and grant the reported missing Google scopes before retrying.".to_string(),
            retry_after_ms: None,
        },
        "oauth_account_ambiguous" | "oauth_client_mismatch" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReviseAndRetry,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Correct the shared Google credential account selector or OAuth client binding, then retry with the updated configuration.".to_string(),
            retry_after_ms: None,
        },
        "oauth_shared_credential_protected" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Confirm,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Do not clear this credential through a single upstream. Use gateway.oauth.google_revoke and obtain explicit confirmation because the credential is shared.".to_string(),
            retry_after_ms: None,
        },
        "confirmation_required" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Confirm,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Obtain explicit user confirmation and retry through the confirmed destructive-action path.".to_string(),
            retry_after_ms: None,
        },
        "result_too_large" | "response_too_large" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReduceWork,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "The response exceeded a size limit; the operation may already have completed. Check its status before repeating a mutation. For a read, use supported pagination, range, or field-selection parameters to request smaller results. Follow any returned resource or artifact reference; do not assume omitted bytes are stored or invent a continuation cursor.".to_string(),
            retry_after_ms: None,
        },
        "budget_exceeded" | "call_budget_exceeded" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReduceWork,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Inspect the execution trace and operation status for work already completed. Reduce fan-out or split the remaining work into smaller runs; do not replay the entire script or repeat completed mutations. Consult existing operation records and reuse an idempotency key only for the same operation and arguments when supported.".to_string(),
            retry_after_ms: None,
        },
        "quota_exceeded" | "artifact_too_large" | "content_too_large"
        | "snippet_budget_exceeded" | "snippet_resolve_limit" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::ReduceWork,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Reduce fan-out or payload size, split the work, or use an artifact before retrying.".to_string(),
            retry_after_ms: None,
        },
        // Skills verification failures (SEP-2640). The spec's own prescribed
        // recovery is to refresh the entry through `skills/get` — or the whole
        // catalog through `skills/list` — and proceed from the current
        // `resources` set, which, being different, revokes any content-bound
        // approval. Benign staleness (the skill changed after the listing was
        // fetched) is a normal cause, so this is `rediscover` rather than
        // `do_not_retry`; `never` still forbids replaying the identical read,
        // because an unchanged retry cannot succeed until the entry is
        // refreshed. The generic rediscover guidance is overridden here: it
        // points at actions, tools, prompts, and resources, none of which is
        // the method a caller needs.
        "skill_digest_mismatch" | "skill_manifest_stale" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::Rediscover,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "Refresh the skill entry with `skills/get` for this skill, or `skills/list` to refresh the catalog, then retry against the current `resources` set. A changed resource set revokes any approval bound to the previous content.".to_string(),
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
        "protocol_error" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::InspectAndEscalate,
            same_arguments: AgentSameArgumentsRetry::Discouraged,
            guidance: "The Agent execution provider or the pinned payload store returned bytes that do not satisfy the expected contract. Inspect the provider configuration and server diagnostics; an unchanged retry is unlikely to succeed until the provider or stored content is repaired.".to_string(),
            retry_after_ms: None,
        },
        "authority_changed" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::RetryLater,
            same_arguments: AgentSameArgumentsRetry::Conditional,
            guidance: "Authority for the owner scope changed while the Agent session or Task attempt was executing. Confirm the caller is still authorized, check whether the run committed partial work at its provider, then start a fresh run; the fenced session cannot be resumed.".to_string(),
            retry_after_ms: None,
        },
        "feature_not_compiled" => AgentRecoveryAdvice {
            action: AgentRecoveryAction::DoNotRetry,
            same_arguments: AgentSameArgumentsRetry::Never,
            guidance: "This build of Labby was compiled without the feature backing this action. Rediscovery will keep advertising it, so retrying cannot succeed — the operator must run a build that includes the feature.".to_string(),
            retry_after_ms: None,
        },
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
    fn agent_execution_kinds_are_classified() {
        let unavailable = metadata_for_kind("unavailable", None);
        assert_eq!(unavailable.origin, AgentErrorOrigin::UpstreamTransport);
        assert_eq!(unavailable.side_effects, AgentSideEffectRisk::Possible);
        assert_eq!(unavailable.recovery.action, AgentRecoveryAction::RetryLater);

        let protocol = metadata_for_kind("protocol_error", None);
        assert_eq!(protocol.origin, AgentErrorOrigin::UpstreamTransport);
        assert_eq!(protocol.side_effects, AgentSideEffectRisk::Possible);
        assert_eq!(
            protocol.recovery.action,
            AgentRecoveryAction::InspectAndEscalate
        );
        assert_eq!(
            protocol.recovery.same_arguments,
            AgentSameArgumentsRetry::Discouraged
        );

        // Authority moved under a run that may already have produced provider
        // effects; the caller must re-establish authority, not retry blindly.
        let authority = metadata_for_kind("authority_changed", None);
        assert_eq!(authority.origin, AgentErrorOrigin::Policy);
        assert_eq!(authority.side_effects, AgentSideEffectRisk::Possible);
        assert_eq!(authority.recovery.action, AgentRecoveryAction::RetryLater);
        assert_eq!(
            authority.recovery.same_arguments,
            AgentSameArgumentsRetry::Conditional
        );
    }

    #[test]
    fn missing_upstream_credential_requires_operator_repair_without_side_effects() {
        let metadata = metadata_for_kind("upstream_credential_missing", None);
        assert_eq!(metadata.origin, AgentErrorOrigin::Policy);
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::NoneExpected);
        assert_eq!(
            metadata.recovery.action,
            AgentRecoveryAction::InspectAndEscalate
        );
        assert_eq!(
            metadata.recovery.same_arguments,
            AgentSameArgumentsRetry::Never
        );
        assert!(metadata.recovery.guidance.contains("operator"));
        assert!(metadata.recovery.guidance.contains("anonymous"));
    }

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
    fn stale_cursor_requires_rediscovery_without_side_effects() {
        let metadata = metadata_for_kind("invalid_cursor", None);
        assert_eq!(metadata.origin, AgentErrorOrigin::Discovery);
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::NoneExpected);
        assert_eq!(metadata.recovery.action, AgentRecoveryAction::Rediscover);
        assert_eq!(
            metadata.recovery.same_arguments,
            AgentSameArgumentsRetry::Never
        );
    }

    #[test]
    fn unmounted_route_does_not_advise_useless_rediscovery() {
        // `not_found` advises Rediscover, which is actively misleading for an
        // HTTP path that is not registered: no listing or search will ever
        // surface a route the server did not mount. This kind exists so the
        // advice points somewhere true instead.
        let metadata = metadata_for_kind("route_not_found", None);
        assert_eq!(metadata.origin, AgentErrorOrigin::Discovery);
        assert_eq!(metadata.side_effects, AgentSideEffectRisk::NoneExpected);
        assert_eq!(
            metadata.recovery.action,
            AgentRecoveryAction::ReviseAndRetry
        );
        assert_eq!(
            metadata.recovery.same_arguments,
            AgentSameArgumentsRetry::Never
        );
        // Reachable unauthenticated, so the guidance must not name which
        // service is missing — only that the startup logs record it.
        assert!(metadata.recovery.guidance.contains("startup logs"));
        for leaked in ["gateway", "snippets", "skills", "palette", "fs"] {
            assert!(
                !metadata.recovery.guidance.contains(leaked),
                "guidance must not name a service; leaked {leaked}"
            );
        }
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
    fn google_broker_errors_have_actionable_recovery_metadata() {
        let scope = metadata_for_kind("oauth_scope_upgrade_required", None);
        assert_eq!(scope.origin, AgentErrorOrigin::Policy);
        assert_eq!(scope.recovery.action, AgentRecoveryAction::Reauthenticate);
        assert_eq!(scope.side_effects, AgentSideEffectRisk::NoneExpected);

        for kind in ["oauth_account_ambiguous", "oauth_client_mismatch"] {
            let metadata = metadata_for_kind(kind, None);
            assert_eq!(metadata.origin, AgentErrorOrigin::Validation, "kind={kind}");
            assert_eq!(
                metadata.recovery.action,
                AgentRecoveryAction::ReviseAndRetry,
                "kind={kind}"
            );
            assert_eq!(
                metadata.recovery.same_arguments,
                AgentSameArgumentsRetry::Never,
                "kind={kind}"
            );
        }

        let protected = metadata_for_kind("oauth_shared_credential_protected", None);
        assert_eq!(protected.origin, AgentErrorOrigin::Policy);
        assert_eq!(protected.recovery.action, AgentRecoveryAction::Confirm);
        assert_eq!(protected.side_effects, AgentSideEffectRisk::NoneExpected);
    }

    #[test]
    fn post_execution_limits_preserve_partial_effect_risk() {
        for kind in [
            "result_too_large",
            "response_too_large",
            "budget_exceeded",
            "call_budget_exceeded",
        ] {
            let value =
                build_agent_error_value(kind, "limit reached", None, &AgentErrorContext::default());
            assert_eq!(value["kind"], kind);
            assert_eq!(value["origin"], "budget");
            assert_eq!(value["side_effects"], "possible", "kind={kind}");
            assert_eq!(value["recovery"]["same_arguments"], "never");
            assert_eq!(value["recovery"]["action"], "reduce_work");
            assert!(
                value["recovery"]["guidance"]
                    .as_str()
                    .unwrap()
                    .contains("status")
            );
        }
        assert_eq!(
            metadata_for_kind("content_too_large", None).side_effects,
            AgentSideEffectRisk::NoneExpected
        );
    }

    #[test]
    fn size_limit_recovery_does_not_invent_retained_output() {
        let advice = recovery_for_kind("result_too_large", None, false);
        assert!(advice.guidance.contains("supported pagination"));
        assert!(
            advice
                .guidance
                .contains("do not assume omitted bytes are stored")
        );
    }

    #[test]
    fn stale_cursor_restarts_listing_and_tool_failure_checks_committed_work() {
        let cursor = recovery_for_kind("invalid_cursor", None, false);
        assert!(cursor.guidance.contains("without a cursor"));
        assert!(cursor.guidance.contains("deduplicate"));
        let tool = metadata_for_kind("tool_error", None);
        assert_eq!(tool.side_effects, AgentSideEffectRisk::Possible);
        assert!(tool.recovery.guidance.contains("partial effects committed"));
    }

    #[test]
    fn oauth_security_failures_recover_without_weakening_validation() {
        for (kind, phrase) in [
            ("oauth_state_invalid", "do not reuse a callback"),
            (
                "oauth_resource_mismatch",
                "Do not bypass issuer or resource validation",
            ),
            (
                "oauth_issuer_mismatch",
                "Do not bypass issuer or resource validation",
            ),
            ("oauth_unsupported_method", "Do not downgrade to plain PKCE"),
        ] {
            let value = metadata_for_kind(kind, None);
            assert_eq!(value.origin, AgentErrorOrigin::Policy, "{kind}");
            assert_eq!(value.side_effects, AgentSideEffectRisk::NoneExpected);
            assert_eq!(
                value.recovery.same_arguments,
                AgentSameArgumentsRetry::Never
            );
            assert!(value.recovery.guidance.contains(phrase), "{kind}");
        }
    }

    #[test]
    fn state_conflicts_identify_the_required_refresh_or_restart() {
        for (kind, action, phrase) in [
            (
                "stale_suggestion",
                AgentRecoveryAction::Rediscover,
                "current metadata hash",
            ),
            (
                "merge_write_conflict",
                AgentRecoveryAction::ReviseAndRetry,
                "Preserve concurrent edits",
            ),
            (
                "workspace_not_configured",
                AgentRecoveryAction::ReviseAndRetry,
                "workspace.root",
            ),
            (
                "restart_required",
                AgentRecoveryAction::StartDependency,
                "gateway reload does not",
            ),
        ] {
            let value = metadata_for_kind(kind, None);
            assert_eq!(value.origin, AgentErrorOrigin::Validation, "{kind}");
            assert_eq!(value.side_effects, AgentSideEffectRisk::NoneExpected);
            assert_eq!(value.recovery.action, action);
            assert!(value.recovery.guidance.contains(phrase), "{kind}");
        }
    }

    #[test]
    fn access_setup_required_is_a_deterministic_setup_gate_not_an_outage() {
        let value = metadata_for_kind("access_setup_required", None);
        assert_eq!(value.origin, AgentErrorOrigin::Validation);
        assert_eq!(value.side_effects, AgentSideEffectRisk::NoneExpected);
        assert_eq!(value.recovery.action, AgentRecoveryAction::StartDependency);
        assert_eq!(
            value.recovery.same_arguments,
            AgentSameArgumentsRetry::Never
        );
        assert_eq!(value.recovery.retry_after_ms, None);
        for phrase in [
            "Ask the operator of the Labby server",
            "`labby setup`",
            "restart the serving Labby process",
            "OAuth provider",
            "browser owner setup",
            "labby auth bootstrap consume",
            "labby auth bootstrap cleanup",
        ] {
            assert!(value.recovery.guidance.contains(phrase), "{phrase}");
        }
    }

    #[test]
    fn unknown_kind_preserves_identity_and_discourages_uncertain_replay() {
        let value = build_agent_error_value(
            "vendor_new_failure",
            "The upstream reported a new failure",
            Some(&json!({"cause": {"code": 17}})),
            &AgentErrorContext::default(),
        );
        assert_eq!(value["kind"], "vendor_new_failure");
        assert_eq!(value["cause"]["code"], 17);
        assert_eq!(value["side_effects"], "unknown");
        assert_eq!(value["recovery"]["action"], "inspect_and_escalate");
        assert_eq!(value["recovery"]["same_arguments"], "discouraged");
        assert!(
            value["recovery"]["guidance"]
                .as_str()
                .unwrap()
                .contains("side effects are uncertain")
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
