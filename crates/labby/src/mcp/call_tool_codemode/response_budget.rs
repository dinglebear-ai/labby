//! MCP adaptation of programmatic Code Mode response budgets.
//!
//! The budget request is read from `tools/call` `_meta`, never from tool
//! arguments, so a model cannot raise its own budget. Resolution and limits
//! live in [`labby_runtime::code_mode_response_budget`].

use labby_runtime::code_mode_response_budget::{
    CodeModeResponseBudget, RESPONSE_BUDGET_BYTES_FIELD, RESPONSE_BUDGET_META_KEY,
    ResponseBudgetError, requested_response_bytes,
};
use labby_runtime::gateway_config::CodeModeConfig;
use rmcp::model::{CallToolResult, MetaObject, RequestMetaObject};

use crate::mcp::envelope::build_error_extra;
use crate::mcp::result_format::error_result_from_envelope;

/// Resolve the budget for one call from its request metadata.
pub(super) fn resolve(
    config: &CodeModeConfig,
    meta: &RequestMetaObject,
) -> Result<CodeModeResponseBudget, ResponseBudgetError> {
    config.resolve_response_budget(requested_response_bytes(
        meta.get(RESPONSE_BUDGET_META_KEY),
    )?)
}

/// The `invalid_param` error result for a rejected request.
pub(super) fn error_result(
    service: &str,
    config: &CodeModeConfig,
    error: &ResponseBudgetError,
) -> CallToolResult {
    let envelope = build_error_extra(
        service,
        "call_tool",
        "invalid_param",
        &error.to_string(),
        &serde_json::json!({
            "param": ResponseBudgetError::PARAM,
            "max_allowed": config.max_programmatic_response_bytes,
            "default_bytes": config.max_response_bytes,
        }),
    );
    error_result_from_envelope(envelope)
}

/// Report a granted budget on the result so a caller can confirm this gateway
/// honored it. An older gateway ignores the request and omits this key.
pub(super) fn echo(result: &mut CallToolResult, budget: CodeModeResponseBudget) {
    if !budget.requested {
        return;
    }
    result.meta.get_or_insert_with(MetaObject::default).insert(
        RESPONSE_BUDGET_META_KEY.to_string(),
        serde_json::json!({ RESPONSE_BUDGET_BYTES_FIELD: budget.max_response_bytes }),
    );
}

/// Dedup-key suffix: runs under different budgets must not share a result.
pub(super) fn dedup_suffix(budget: CodeModeResponseBudget) -> String {
    if budget.requested {
        format!("|budget={}", budget.max_response_bytes)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn meta(value: Value) -> RequestMetaObject {
        let mut meta = RequestMetaObject::default();
        meta.insert(RESPONSE_BUDGET_META_KEY.to_string(), value);
        meta
    }

    #[test]
    fn tool_arguments_cannot_request_a_budget() {
        // Only `_meta` is read; an agent-controlled argument has no channel here.
        let config = CodeModeConfig::default();
        let budget = resolve(&config, &RequestMetaObject::default()).unwrap();
        assert!(!budget.requested);
        assert_eq!(budget.max_response_bytes, config.max_response_bytes);
        assert_eq!(dedup_suffix(budget), "");
    }

    #[test]
    fn granted_budget_is_echoed_and_keys_dedup() {
        let config = CodeModeConfig::default();
        let budget = resolve(&config, &meta(json!({ "maxResponseBytes": 400_000 }))).unwrap();
        assert_eq!(budget.max_response_bytes, 400_000);
        assert_eq!(dedup_suffix(budget), "|budget=400000");
        let mut result = CallToolResult::success(Vec::new());
        echo(&mut result, budget);
        assert_eq!(
            result.meta.unwrap().get(RESPONSE_BUDGET_META_KEY),
            Some(&json!({ "maxResponseBytes": 400_000 }))
        );
    }

    #[test]
    fn rejected_budget_is_a_structured_invalid_param() {
        let config = CodeModeConfig::default();
        let error = resolve(
            &config,
            &meta(json!({ "maxResponseBytes": 64 * 1024 * 1024 })),
        )
        .unwrap_err();
        let result = error_result("codemode_read", &config, &error);
        assert_eq!(result.is_error, Some(true));
        let text = serde_json::to_string(&result).unwrap();
        assert!(text.contains("invalid_param"), "{text}");
        assert!(text.contains("max_allowed"), "{text}");
        assert!(text.contains("1048576"), "{text}");
    }
}
