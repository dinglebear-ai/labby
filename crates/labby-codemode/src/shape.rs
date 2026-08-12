use serde::Serialize;
use serde_json::Value;

use labby_runtime::CodeModeResultShapePolicy;

const MIN_SHAPED_RESULT_BYTES: usize = 256;
const SOFT_WARNING_DIVISOR: usize = 3;
const MIN_SOFT_WARNING_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeModeResultShapeMetadata {
    pub policy: CodeModeResultShapePolicy,
    pub changed: bool,
    pub truncated: bool,
    pub original_size_bytes: usize,
    pub shaped_size_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ShapedResult {
    pub result: Option<Value>,
    pub metadata: CodeModeResultShapeMetadata,
}

pub(crate) fn shape_final_result(
    result: Option<Value>,
    policy: CodeModeResultShapePolicy,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> ShapedResult {
    let original_size = result
        .as_ref()
        .and_then(|value| serde_json::to_vec(value).ok())
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    let budget = effective_result_budget(
        max_response_bytes,
        max_response_tokens,
        token_estimate_divisor,
    );
    let warning = large_result_warning(original_size, budget);

    match (policy, result) {
        (CodeModeResultShapePolicy::Off, result) => {
            unchanged(result, policy, original_size, warning)
        }
        (CodeModeResultShapePolicy::Truncate, None) => unchanged(None, policy, original_size, None),
        (CodeModeResultShapePolicy::Truncate, Some(value)) => {
            shape_truncate(value, policy, original_size, budget, warning)
        }
    }
}

fn effective_result_budget(
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> usize {
    let token_budget_bytes = max_response_tokens
        .max(1)
        .saturating_mul(token_estimate_divisor.max(1) as usize);
    max_response_bytes
        .min(token_budget_bytes)
        .max(MIN_SHAPED_RESULT_BYTES)
}

fn large_result_warning(original_size_bytes: usize, budget: usize) -> Option<String> {
    let threshold = (budget / SOFT_WARNING_DIVISOR).max(MIN_SOFT_WARNING_BYTES);
    if threshold >= budget || original_size_bytes < threshold || original_size_bytes > budget {
        return None;
    }
    Some(format!(
        "large result: {original_size_bytes} bytes (soft threshold {threshold}, hard budget {budget}); project fields, filter rows, or slice arrays before returning"
    ))
}

fn unchanged(
    result: Option<Value>,
    policy: CodeModeResultShapePolicy,
    original_size_bytes: usize,
    warning: Option<String>,
) -> ShapedResult {
    ShapedResult {
        result,
        metadata: CodeModeResultShapeMetadata {
            policy,
            changed: false,
            truncated: false,
            original_size_bytes,
            shaped_size_bytes: original_size_bytes,
            warning,
        },
    }
}

fn shape_truncate(
    value: Value,
    policy: CodeModeResultShapePolicy,
    original_size_bytes: usize,
    budget: usize,
    warning: Option<String>,
) -> ShapedResult {
    if original_size_bytes <= budget {
        return unchanged(Some(value), policy, original_size_bytes, warning);
    }

    let serialized = match &value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
    };
    let marker_prefix = format!(
        "[code mode result truncated]\noriginal_size_bytes={original_size_bytes}, max_size_bytes={budget}\n"
    );
    let room = budget.saturating_sub(marker_prefix.len());
    let preview = utf8_prefix_by_bytes(&serialized, room);
    let marker = format!("{marker_prefix}{preview}");
    let shaped_size_bytes = serde_json::to_vec(&Value::String(marker.clone()))
        .map(|bytes| bytes.len())
        .unwrap_or_else(|_| marker.len());

    ShapedResult {
        result: Some(Value::String(marker)),
        metadata: CodeModeResultShapeMetadata {
            policy,
            changed: true,
            truncated: true,
            original_size_bytes,
            shaped_size_bytes,
            warning: None,
        },
    }
}

fn utf8_prefix_by_bytes(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let end = value
        .char_indices()
        .map(|(idx, _)| idx)
        .take_while(|idx| *idx <= max_bytes)
        .last()
        .unwrap_or(0);
    &value[..end]
}

#[cfg(test)]
mod tests {
    use labby_runtime::CodeModeResultShapePolicy;
    use serde_json::Value;

    use super::shape_final_result;

    const MAX_BYTES: usize = 24 * 1024;
    const MAX_TOKENS: usize = 6000;
    const TOKEN_DIVISOR: u32 = 4;

    #[test]
    fn small_result_has_no_warning() {
        let shaped = shape_final_result(
            Some(Value::String("small".to_string())),
            CodeModeResultShapePolicy::Truncate,
            MAX_BYTES,
            MAX_TOKENS,
            TOKEN_DIVISOR,
        );

        assert!(!shaped.metadata.changed);
        assert!(shaped.metadata.warning.is_none());
    }

    #[test]
    fn large_result_is_preserved_with_soft_warning() {
        // JSON string serialization adds two quote bytes, landing exactly on
        // the 8,000-byte threshold derived from the effective 24,000-byte cap.
        let value = Value::String("x".repeat(7998));
        let shaped = shape_final_result(
            Some(value.clone()),
            CodeModeResultShapePolicy::Truncate,
            MAX_BYTES,
            MAX_TOKENS,
            TOKEN_DIVISOR,
        );

        assert_eq!(shaped.result, Some(value));
        assert!(!shaped.metadata.changed);
        assert!(!shaped.metadata.truncated);
        assert_eq!(shaped.metadata.original_size_bytes, 8000);
        assert!(
            shaped
                .metadata
                .warning
                .as_deref()
                .is_some_and(|warning| warning.contains("soft threshold 8000"))
        );
    }

    #[test]
    fn over_budget_result_uses_hard_marker_without_soft_warning() {
        let shaped = shape_final_result(
            Some(Value::String("x".repeat(MAX_BYTES + 1000))),
            CodeModeResultShapePolicy::Truncate,
            MAX_BYTES,
            MAX_TOKENS,
            TOKEN_DIVISOR,
        );

        assert!(shaped.metadata.changed);
        assert!(shaped.metadata.truncated);
        assert!(shaped.metadata.warning.is_none());
        assert!(
            shaped
                .result
                .as_ref()
                .and_then(Value::as_str)
                .is_some_and(|value| value.starts_with("[code mode result truncated]"))
        );
    }

    /// FR-5 (issue #210, lab-41e7m.2): under a non-`Off` policy, an
    /// over-budget STRUCTURED result becomes exactly one marker string whose
    /// preview derives from the pretty-printed JSON — nothing else in the
    /// shaped result is re-serialized or double-encoded.
    #[test]
    fn over_budget_structured_result_becomes_single_marker_string() {
        let shaped = shape_final_result(
            Some(serde_json::json!({"rows": vec!["r".repeat(64); 4000]})),
            CodeModeResultShapePolicy::Truncate,
            MAX_BYTES,
            MAX_TOKENS,
            TOKEN_DIVISOR,
        );

        assert!(shaped.metadata.truncated);
        let marker = shaped
            .result
            .as_ref()
            .and_then(Value::as_str)
            .expect("marker is one string");
        assert!(marker.starts_with("[code mode result truncated]"));
        assert!(
            marker.contains("\"rows\""),
            "preview shows the pretty-printed structured value, not a re-stringified escape soup"
        );
        assert!(
            !marker.contains("\\\"rows\\\""),
            "the structured value must not be double-encoded"
        );
    }

    #[test]
    fn hard_marker_stays_on_utf8_boundary() {
        let shaped = shape_final_result(
            Some(Value::String("🦂".repeat(MAX_BYTES))),
            CodeModeResultShapePolicy::Truncate,
            MAX_BYTES,
            MAX_TOKENS,
            TOKEN_DIVISOR,
        );

        assert!(shaped.metadata.truncated);
        let marker = shaped
            .result
            .and_then(|value| value.as_str().map(str::to_owned));
        assert!(marker.is_some());
    }
}
