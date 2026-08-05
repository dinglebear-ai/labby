//! Analysis and model-facing enrichment for completed MCP tool failures.

use labby_runtime::agent_error::{
    AgentErrorContext, AgentErrorOrigin, AgentSideEffectRisk, build_agent_error_value,
    metadata_for_kind_with_retry_safety, retry_after_ms_from_object, sanitize_error_text,
    tool_execution_message,
};
use labby_runtime::redact::redact_trace_value;
use rmcp::model::{CallToolResult, ContentBlock, MetaObject, ToolAnnotations};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const LABBY_ERROR_META_KEY: &str = "ai.dinglebear.labby/error";
const MAX_ERROR_CONTENT_BLOCKS: usize = 16;
const MAX_ERROR_TEXT_CHARS: usize = 8 * 1024;
const MAX_ERROR_CONTENT_BYTES: usize = 4 * 1024;
const MAX_ERROR_STRUCTURED_BYTES: usize = 8 * 1024;
const MAX_PARSED_ERROR_BYTES: usize = 4 * 1024;
/// Text blocks larger than this are never candidates for structured-error
/// JSON parsing — parsing multi-megabyte upstream payloads to maybe find a
/// `{kind, message}` object is wasted work on the error path.
const MAX_PARSE_CANDIDATE_BYTES: usize = 64 * 1024;

/// MCP tool annotations that informed retry and side-effect guidance.
///
/// Alias of the canonical `labby_runtime` definition shared with Code Mode's
/// `CodeModeToolSafetyHints`.
pub type McpToolSafetyHints = labby_runtime::agent_error::ToolSafetyHints;

/// Sanitized evidence preserved from a completed upstream MCP tool result.
///
/// Alias of the canonical `labby_runtime` definition shared with Code Mode's
/// `CodeModeErrorEvidence`.
pub type McpToolErrorEvidence = labby_runtime::agent_error::ToolErrorEvidence;

/// Build safety hints from rmcp tool annotations.
///
/// A free function (not an inherent method) because `McpToolSafetyHints` is an
/// alias of the shared, rmcp-free `labby_runtime` type.
#[must_use]
pub fn safety_hints_from_annotations(annotations: Option<&ToolAnnotations>) -> McpToolSafetyHints {
    let Some(annotations) = annotations else {
        return McpToolSafetyHints::default();
    };
    McpToolSafetyHints {
        read_only_hint: annotations.read_only_hint,
        destructive_hint: annotations.destructive_hint,
        idempotent_hint: annotations.idempotent_hint,
        open_world_hint: annotations.open_world_hint,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletedToolErrorAnalysis {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_kind: Option<String>,
    pub cause: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    pub evidence: McpToolErrorEvidence,
}

#[must_use]
pub fn analyze_completed_tool_error(result: &CallToolResult) -> CompletedToolErrorAnalysis {
    let parsed_error = parsed_error_object(result);
    let raw_kind = parsed_error
        .as_ref()
        .and_then(error_object)
        .and_then(|object| object.get("kind"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let kind = raw_kind
        .as_deref()
        .map(canonicalize_untrusted_upstream_kind)
        .unwrap_or("tool_error")
        .to_string();
    let retry_after_ms = parsed_error
        .as_ref()
        .and_then(error_object)
        .and_then(retry_after_ms_from_object);
    let cause = error_cause(result, parsed_error.as_ref());
    let evidence = error_evidence(result, parsed_error);
    CompletedToolErrorAnalysis {
        kind,
        original_kind: raw_kind,
        cause,
        retry_after_ms,
        evidence,
    }
}

#[must_use]
pub fn agent_error_for_completed_tool_result(
    service: &str,
    action: &str,
    tool: &str,
    upstream: Option<&str>,
    result: &CallToolResult,
    safety: &McpToolSafetyHints,
) -> Value {
    let analysis = analyze_completed_tool_error(result);
    let mut metadata = metadata_for_kind_with_retry_safety(
        &analysis.kind,
        analysis.retry_after_ms,
        safety.exact_retry_is_hint_safe(),
    );
    metadata.origin = AgentErrorOrigin::ToolExecution;
    metadata.side_effects = if safety.read_only_hint == Some(true) {
        AgentSideEffectRisk::NoneExpected
    } else {
        AgentSideEffectRisk::Possible
    };
    let message = tool_execution_message(
        tool,
        &analysis.cause,
        &metadata.recovery.guidance,
        metadata.side_effects,
    );
    // Omit `original_kind` when absent (the published schema types it as a
    // string, never null) and skip empty safety/evidence objects to match the
    // Code Mode side's `skip_serializing_if` behavior.
    let mut extra = Map::new();
    if let Some(original_kind) = analysis.original_kind.as_deref() {
        extra.insert("original_kind".to_string(), json!(original_kind));
    }
    extra.insert("cause".to_string(), json!(analysis.cause));
    if !safety.is_empty() {
        extra.insert("safety".to_string(), json!(safety));
    }
    if !analysis.evidence.is_empty() {
        extra.insert("evidence".to_string(), json!(analysis.evidence));
    }
    let extra = Value::Object(extra);
    let mut context = AgentErrorContext::for_service_action(service, action);
    context.tool = Some(tool.to_string());
    context.upstream = upstream.map(ToOwned::to_owned);
    context.origin = Some(metadata.origin);
    context.recovery = Some(metadata.recovery);
    context.side_effects = Some(metadata.side_effects);
    build_agent_error_value(&analysis.kind, &message, Some(&extra), &context)
}

/// Prepend one Labby diagnostic block while retaining every upstream block,
/// wrap the upstream structured value, and attach the same contract in namespaced
/// result metadata. Only call this for a completed `isError: true` result.
#[must_use]
pub fn enrich_completed_tool_error_result(
    service: &str,
    action: &str,
    tool: &str,
    upstream: Option<&str>,
    mut result: CallToolResult,
    safety: &McpToolSafetyHints,
) -> (CallToolResult, &'static str) {
    let contract =
        agent_error_for_completed_tool_result(service, action, tool, upstream, &result, safety);
    let kind = contract
        .get("kind")
        .and_then(Value::as_str)
        .map(canonicalize_untrusted_upstream_kind)
        .unwrap_or("tool_error");
    let original_content = std::mem::take(&mut result.content);
    let mut content = Vec::with_capacity(original_content.len().saturating_add(1));
    content.push(ContentBlock::text(contract.to_string()));
    content.extend(original_content);
    result.content = content;

    let upstream_structured_content = result.structured_content.take();
    result.structured_content = Some(json!({
        "error": contract,
        "upstream_structured_content": upstream_structured_content,
    }));
    let mut meta = result.meta.take().unwrap_or_else(|| MetaObject(Map::new()));
    meta.0
        .insert(LABBY_ERROR_META_KEY.to_string(), contract.clone());
    result.meta = Some(meta);
    result.is_error = Some(true);
    (result, kind)
}

/// Map an UNTRUSTED upstream-supplied kind string onto Labby's stable kind
/// vocabulary.
///
/// Not to be confused with `crate::mcp::error::canonical_kind` in the `labby`
/// binary crate, which canonicalizes Labby's OWN `ToolError` kinds and falls
/// back to `internal_error`. Here the input comes from an arbitrary upstream
/// server, so infrastructure-looking kinds (`upstream_error`, `network_error`,
/// `server_error`, …) and every unrecognized value collapse to `tool_error` —
/// a completed `isError: true` result is a tool execution failure regardless
/// of what the upstream called it. The raw value is preserved separately as
/// `original_kind`.
#[must_use]
pub fn canonicalize_untrusted_upstream_kind(kind: &str) -> &'static str {
    match kind {
        "unknown_action" => "unknown_action",
        "unknown_subaction" => "unknown_subaction",
        "missing_param" => "missing_param",
        "invalid_param" => "invalid_param",
        "unknown_instance" => "unknown_instance",
        "confirmation_required" => "confirmation_required",
        "conflict" => "conflict",
        "forbidden" => "forbidden",
        "unknown_tool" => "unknown_tool",
        "route_scope_denied" => "route_scope_denied",
        "path_traversal" => "path_traversal",
        "permission_denied" => "permission_denied",
        "timeout" => "timeout",
        "budget_exceeded" => "budget_exceeded",
        "quota_exceeded" => "quota_exceeded",
        "invalid_code_mode_id" => "invalid_code_mode_id",
        "snippet_not_found" => "snippet_not_found",
        "artifact_too_large" => "artifact_too_large",
        "auth_failed" => "auth_failed",
        "oauth_needs_reauth" => "oauth_needs_reauth",
        "not_found" => "not_found",
        "rate_limited" => "rate_limited",
        "validation_failed" => "validation_failed",
        "tool_error" | "network_error" | "server_error" | "decode_error" | "internal_error"
        | "upstream_error" => "tool_error",
        "code_mode_timeout" => "code_mode_timeout",
        _ => "tool_error",
    }
}

fn error_cause(result: &CallToolResult, parsed_error: Option<&Value>) -> String {
    if let Some(message) = parsed_error
        .and_then(error_object)
        .and_then(|object| object.get("message"))
        .and_then(Value::as_str)
    {
        return sanitize_error_text(message, MAX_ERROR_TEXT_CHARS);
    }
    let text = result
        .content
        .iter()
        .filter_map(ContentBlock::as_text)
        .map(|content| content.text.as_str())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    if text.is_empty() {
        format!(
            "upstream tool returned {} non-text error content block(s)",
            result.content.len()
        )
    } else {
        sanitize_error_text(&text, MAX_ERROR_TEXT_CHARS)
    }
}

fn error_evidence(result: &CallToolResult, parsed_error: Option<Value>) -> McpToolErrorEvidence {
    let content = result
        .content
        .iter()
        .take(MAX_ERROR_CONTENT_BLOCKS)
        .map(sanitized_content_block)
        .collect::<Vec<_>>();
    let omitted_content_blocks = result.content.len().saturating_sub(content.len());
    McpToolErrorEvidence {
        content,
        structured_content: result
            .structured_content
            .as_ref()
            .map(|value| redact_trace_value(value, MAX_ERROR_STRUCTURED_BYTES)),
        parsed_error: parsed_error
            .as_ref()
            .map(|value| redact_trace_value(value, MAX_PARSED_ERROR_BYTES)),
        omitted_content_blocks,
    }
}

fn sanitized_content_block(content: &ContentBlock) -> Value {
    match content {
        ContentBlock::Text(text) => json!({
            "type": "text",
            "text": sanitize_error_text(&text.text, MAX_ERROR_TEXT_CHARS),
        }),
        ContentBlock::Image(image) => json!({
            "type": "image",
            "mime_type": image.mime_type,
            "encoded_bytes": image.data.len(),
            "data_omitted": true,
        }),
        ContentBlock::Audio(audio) => json!({
            "type": "audio",
            "mime_type": audio.mime_type,
            "encoded_bytes": audio.data.len(),
            "data_omitted": true,
        }),
        ContentBlock::Resource(_) | ContentBlock::ResourceLink(_) => {
            let serialized = serde_json::to_value(content)
                .unwrap_or_else(|_| json!({ "type": "resource", "serialization_failed": true }));
            redact_trace_value(&serialized, MAX_ERROR_CONTENT_BYTES)
        }
        _ => json!({ "type": "unknown", "content_omitted": true }),
    }
}

/// One of three best-effort structured-error recovery seams — keep behavior
/// aligned when changing any of them:
/// - here (upstream MCP content → parsed error object),
/// - `crates/labby-codemode/src/runner.rs` `extract_structured_error` (runner
///   rejection text → `CodeModeCallError`),
/// - `crates/labby/src/entrypoint.rs` `cli_error_value` (anyhow string → CLI
///   JSON error).
fn parsed_error_object(result: &CallToolResult) -> Option<Value> {
    if let Some(structured) = result.structured_content.as_ref()
        && let Some(error) = normalize_error_object(structured)
    {
        return Some(error);
    }
    result
        .content
        .iter()
        .filter_map(ContentBlock::as_text)
        .filter(|content| content.text.len() <= MAX_PARSE_CANDIDATE_BYTES)
        .filter_map(|content| serde_json::from_str::<Value>(&content.text).ok())
        .find_map(normalize_error_object_owned)
}

fn normalize_error_object(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let candidate = object
        .get("error")
        .and_then(Value::as_object)
        .unwrap_or(object);
    if candidate.contains_key("kind") || candidate.contains_key("message") {
        Some(Value::Object(candidate.clone()))
    } else {
        None
    }
}

/// Owned variant of [`normalize_error_object`] for values we just parsed —
/// moves the recognized error object out instead of deep-cloning it.
fn normalize_error_object_owned(value: Value) -> Option<Value> {
    let Value::Object(mut object) = value else {
        return None;
    };
    if let Some(Value::Object(inner)) = object.get("error") {
        if inner.contains_key("kind") || inner.contains_key("message") {
            return object.remove("error");
        }
        return None;
    }
    if object.contains_key("kind") || object.contains_key("message") {
        return Some(Value::Object(object));
    }
    None
}

fn error_object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_failure_after_success_block_and_structured_kind() {
        let mut result = CallToolResult::error(vec![
            ContentBlock::text("step one succeeded"),
            ContentBlock::text("ERROR at step 2/3"),
        ]);
        result.structured_content = Some(json!({
            "error": {"kind":"upstream_error","message":"Exit code 7"},
            "partial": true,
        }));
        let analysis = analyze_completed_tool_error(&result);
        assert_eq!(analysis.kind, "tool_error");
        assert_eq!(analysis.original_kind.as_deref(), Some("upstream_error"));
        assert_eq!(analysis.cause, "Exit code 7");
        assert_eq!(analysis.evidence.content.len(), 2);
    }

    #[test]
    fn enrichment_preserves_original_blocks_and_wraps_structured_content() {
        let mut result = CallToolResult::error(vec![ContentBlock::text("failed")]);
        result.structured_content = Some(json!({"partial":true}));
        let (result, kind) = enrich_completed_tool_error_result(
            "demo",
            "call_tool",
            "alpha::demo",
            Some("alpha"),
            result,
            &McpToolSafetyHints::default(),
        );
        assert_eq!(kind, "tool_error");
        assert_eq!(result.content.len(), 2);
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("recovery")
        );
        assert_eq!(
            result.structured_content.as_ref().unwrap()["upstream_structured_content"]["partial"],
            true
        );
        assert!(result.meta.unwrap().0.contains_key(LABBY_ERROR_META_KEY));
    }

    #[test]
    fn empty_optional_fields_are_omitted_from_the_contract() {
        // `original_kind: null` would violate the published schema (type
        // string), and empty safety/evidence objects must be skipped to match
        // the Code Mode envelope.
        let result = CallToolResult::error(vec![]);
        let contract = agent_error_for_completed_tool_result(
            "demo",
            "call_tool",
            "alpha::demo",
            Some("alpha"),
            &result,
            &McpToolSafetyHints::default(),
        );
        let object = contract.as_object().expect("contract object");
        assert!(!object.contains_key("original_kind"));
        assert!(!object.contains_key("safety"));
        assert!(!object.contains_key("evidence"));
        assert_eq!(contract["kind"], "tool_error");
    }

    #[test]
    fn populated_safety_and_evidence_are_preserved() {
        let result = CallToolResult::error(vec![ContentBlock::text("boom")]);
        let contract = agent_error_for_completed_tool_result(
            "demo",
            "call_tool",
            "alpha::demo",
            Some("alpha"),
            &result,
            &McpToolSafetyHints {
                read_only_hint: Some(true),
                ..McpToolSafetyHints::default()
            },
        );
        assert_eq!(contract["safety"]["read_only_hint"], true);
        assert_eq!(contract["evidence"]["content"][0]["text"], "boom");
        assert_eq!(contract["side_effects"], "none_expected");
    }

    #[test]
    fn oversized_text_blocks_are_not_json_parse_candidates() {
        // A structured error hidden inside a >64 KiB text block is skipped;
        // classification falls back to `tool_error` without parsing it.
        let huge = format!(
            "{}{}",
            " ".repeat(MAX_PARSE_CANDIDATE_BYTES),
            r#"{"kind":"rate_limited","message":"slow down"}"#
        );
        let result = CallToolResult::error(vec![ContentBlock::text(huge)]);
        let analysis = analyze_completed_tool_error(&result);
        assert_eq!(analysis.kind, "tool_error");
        assert!(analysis.evidence.parsed_error.is_none());

        // The same payload under the threshold IS recovered.
        let small = r#"{"kind":"rate_limited","message":"slow down"}"#;
        let result = CallToolResult::error(vec![ContentBlock::text(small)]);
        let analysis = analyze_completed_tool_error(&result);
        assert_eq!(analysis.kind, "rate_limited");
        assert_eq!(analysis.original_kind.as_deref(), Some("rate_limited"));
    }

    #[test]
    fn binary_evidence_is_described_not_embedded() {
        let result =
            CallToolResult::error(vec![ContentBlock::image("a".repeat(32_000), "image/png")]);
        let analysis = analyze_completed_tool_error(&result);
        assert_eq!(analysis.evidence.content[0]["data_omitted"], true);
        assert!(analysis.evidence.content[0].get("data").is_none());
    }
}
