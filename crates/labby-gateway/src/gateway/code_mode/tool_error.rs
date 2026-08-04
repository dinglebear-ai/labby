//! MCP `CallToolResult(isError=true)` to Code Mode error conversion.
//!
//! The upstream result is a successful MCP protocol response. This module
//! therefore preserves it as a model-facing tool-execution error without
//! poisoning upstream health, while sanitizing and capping the untrusted
//! evidence before it enters the JavaScript sandbox or an LLM context.

use labby_codemode::{
    CodeModeCallError, CodeModeErrorEvidence, CodeModeToolSafetyHints, redact_trace_value,
    sanitize_error_text,
};
use rmcp::model::{CallToolResult, ContentBlock, ToolAnnotations};
use serde_json::{Map, Value, json};

use crate::upstream::types::UpstreamTool;

const MAX_ERROR_CONTENT_BLOCKS: usize = 16;
const MAX_ERROR_TEXT_CHARS: usize = 8 * 1024;
const MAX_ERROR_CONTENT_BYTES: usize = 4 * 1024;
const MAX_ERROR_STRUCTURED_BYTES: usize = 8 * 1024;
const MAX_PARSED_ERROR_BYTES: usize = 4 * 1024;

/// Convert one completed MCP tool error into Labby's stable Code Mode contract.
#[must_use]
pub(super) fn completed_tool_error(
    id: &str,
    result: &CallToolResult,
    safety: CodeModeToolSafetyHints,
) -> CodeModeCallError {
    let parsed_error = parsed_error_object(result);
    let raw_kind = parsed_error
        .as_ref()
        .and_then(error_object)
        .and_then(|object| object.get("kind"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let canonical_kind = raw_kind
        .as_deref()
        .map(canonical_error_kind)
        .unwrap_or("tool_error");
    let retry_after_ms = parsed_error
        .as_ref()
        .and_then(error_object)
        .and_then(retry_after_ms_from_object);
    let cause = error_cause(result, parsed_error.as_ref());
    let evidence = error_evidence(result, parsed_error);

    CodeModeCallError::tool_execution(
        id,
        canonical_kind,
        raw_kind,
        cause,
        evidence,
        safety,
        retry_after_ms,
    )
}

/// Extract the standard MCP safety hints from a discovered upstream tool.
#[must_use]
pub(super) fn upstream_tool_safety(tool: &UpstreamTool) -> CodeModeToolSafetyHints {
    safety_from_annotations(tool.tool.annotations.as_ref())
}

#[must_use]
fn safety_from_annotations(annotations: Option<&ToolAnnotations>) -> CodeModeToolSafetyHints {
    let Some(annotations) = annotations else {
        return CodeModeToolSafetyHints::default();
    };
    CodeModeToolSafetyHints {
        read_only_hint: annotations.read_only_hint,
        destructive_hint: annotations.destructive_hint,
        idempotent_hint: annotations.idempotent_hint,
        open_world_hint: annotations.open_world_hint,
    }
}

/// Canonicalize an upstream-local tool error kind without confusing it with
/// the health of Labby's transport to that upstream.
#[must_use]
fn canonical_error_kind(kind: &str) -> &'static str {
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
        "not_found" => "not_found",
        "rate_limited" => "rate_limited",
        "validation_failed" => "validation_failed",
        // These names would describe infrastructure if Labby itself emitted
        // them. Inside a completed MCP `isError` result they describe the
        // upstream tool's execution and must remain non-health-poisoning.
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
    if !text.is_empty() {
        return sanitize_error_text(&text, MAX_ERROR_TEXT_CHARS);
    }

    format!(
        "upstream tool returned {} non-text error content block(s)",
        result.content.len()
    )
}

fn error_evidence(result: &CallToolResult, parsed_error: Option<Value>) -> CodeModeErrorEvidence {
    let content = result
        .content
        .iter()
        .take(MAX_ERROR_CONTENT_BLOCKS)
        .map(sanitized_content_block)
        .collect::<Vec<_>>();
    let omitted_content_blocks = result.content.len().saturating_sub(content.len());
    let structured_content = result
        .structured_content
        .as_ref()
        .map(|value| redact_trace_value(value, MAX_ERROR_STRUCTURED_BYTES));
    let parsed_error = parsed_error
        .as_ref()
        .map(|value| redact_trace_value(value, MAX_PARSED_ERROR_BYTES));

    CodeModeErrorEvidence {
        content,
        structured_content,
        parsed_error,
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
        _ => json!({
            "type": "unknown",
            "content_omitted": true,
        }),
    }
}

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
        .filter_map(|content| serde_json::from_str::<Value>(&content.text).ok())
        .find_map(|value| normalize_error_object(&value))
}

fn normalize_error_object(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let candidate = object
        .get("error")
        .and_then(Value::as_object)
        .unwrap_or(object);
    if candidate.contains_key("kind") || candidate.contains_key("message") {
        return Some(Value::Object(candidate.clone()));
    }
    None
}

fn error_object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn retry_after_ms_from_object(object: &Map<String, Value>) -> Option<u64> {
    object
        .get("retry_after_ms")
        .or_else(|| object.get("retryAfterMs"))
        .and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_codemode::{CodeModeErrorOrigin, CodeModeRecoveryAction, CodeModeSideEffectRisk};
    use rmcp::model::ContentBlock;

    #[test]
    fn preserves_all_text_blocks_and_uses_structured_failure_message() {
        let mut result = CallToolResult::error(vec![
            ContentBlock::text("step one succeeded"),
            ContentBlock::text("ERROR at step 2/3: exit 7"),
        ]);
        result.structured_content = Some(json!({
            "error": {
                "kind": "upstream_error",
                "message": "Exit code 7",
                "retry_after_ms": 250,
            },
            "partial": true,
        }));

        let error = completed_tool_error(
            "claude-dookie::Bash",
            &result,
            CodeModeToolSafetyHints::default(),
        );

        assert_eq!(error.kind, "tool_error");
        assert_eq!(error.original_kind.as_deref(), Some("upstream_error"));
        assert_eq!(error.origin, CodeModeErrorOrigin::ToolExecution);
        assert_eq!(error.side_effects, CodeModeSideEffectRisk::Possible);
        assert_eq!(
            error.recovery.action,
            CodeModeRecoveryAction::ReviseAndRetry
        );
        assert_eq!(error.cause.as_deref(), Some("Exit code 7"));
        let evidence = error.evidence.expect("preserved evidence");
        assert_eq!(evidence.content.len(), 2);
        assert_eq!(evidence.content[0]["text"], "step one succeeded");
        assert_eq!(evidence.content[1]["text"], "ERROR at step 2/3: exit 7");
        assert_eq!(evidence.structured_content.unwrap()["partial"], true);
    }

    #[test]
    fn stable_model_correctable_kinds_survive() {
        for kind in [
            "forbidden",
            "unknown_tool",
            "permission_denied",
            "timeout",
            "budget_exceeded",
            "rate_limited",
        ] {
            let result = CallToolResult::error(vec![ContentBlock::text(
                json!({ "kind": kind, "message": format!("{kind} message") }).to_string(),
            )]);
            let error =
                completed_tool_error("demo::tool", &result, CodeModeToolSafetyHints::default());
            assert_eq!(error.kind, kind);
            assert_eq!(error.cause, Some(format!("{kind} message")));
        }
    }

    #[test]
    fn infrastructure_named_completed_errors_remain_tool_errors() {
        for kind in [
            "upstream_error",
            "network_error",
            "server_error",
            "decode_error",
            "internal_error",
        ] {
            let result = CallToolResult::error(vec![ContentBlock::text(
                json!({ "kind": kind, "message": "tool-local failure" }).to_string(),
            )]);
            let error =
                completed_tool_error("demo::tool", &result, CodeModeToolSafetyHints::default());
            assert_eq!(error.kind, "tool_error");
            assert_eq!(error.original_kind.as_deref(), Some(kind));
        }
    }

    #[test]
    fn non_text_binary_evidence_is_described_not_embedded() {
        let result =
            CallToolResult::error(vec![ContentBlock::image("a".repeat(32_000), "image/png")]);
        let error = completed_tool_error(
            "vision::render",
            &result,
            CodeModeToolSafetyHints::default(),
        );
        let block = &error.evidence.unwrap().content[0];
        assert_eq!(block["type"], "image");
        assert_eq!(block["data_omitted"], true);
        assert!(block.get("data").is_none());
    }

    #[test]
    fn evidence_redacts_secret_shaped_text() {
        let result = CallToolResult::error(vec![ContentBlock::text(format!(
            "failed with {}",
            "sk-abcdefghijklmnopqrstuvwxyz123456"
        ))]);
        let error = completed_tool_error("demo::tool", &result, CodeModeToolSafetyHints::default());
        assert!(error.message.contains("[REDACTED]"));
        assert!(!error.message.contains("sk-abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn read_only_annotations_reduce_side_effect_warning() {
        let annotations = ToolAnnotations::from_raw(None, Some(true), None, None, None);
        let safety = safety_from_annotations(Some(&annotations));
        let result = CallToolResult::error(vec![ContentBlock::text("bad query")]);
        let error = completed_tool_error("search::query", &result, safety);
        assert_eq!(error.side_effects, CodeModeSideEffectRisk::NoneExpected);
    }
}
