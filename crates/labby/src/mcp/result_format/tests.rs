//! Tests for result/envelope formatting + error-info extraction + token
//! estimation. Distributed from `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    error_result_from_envelope, estimate_tokens, estimate_tokens_args, estimate_tokens_value,
    extract_error_info, format_dispatch_result, tool_error_envelope,
};
use crate::dispatch::error::ToolError;
use crate::mcp::error::{DispatchError, canonical_kind};
use serde_json::Value;

#[test]
fn estimate_tokens_uses_chars_div_four_heuristic() {
    assert_eq!(estimate_tokens(""), 0);
    // 4 chars → 1 token.
    assert_eq!(estimate_tokens("abcd"), 1);
    // 5 chars → 2 tokens (ceiling).
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens("hello world"), 3);
}

#[test]
fn auth_errors_carry_mcp_reauthentication_metadata() {
    let envelope = crate::mcp::envelope::build_error(
        "gateway",
        "status.list",
        "auth_failed",
        "sign in required",
    );
    let result = error_result_from_envelope(envelope);
    assert_eq!(
        result
            .meta
            .expect("authentication errors must provide protocol recovery metadata")
            .0["mcp/www_authenticate"],
        serde_json::json!([
            "Bearer error=\"invalid_token\", error_description=\"sign in required\", scope=\"lab:read\""
        ])
    );
}

#[test]
fn forbidden_and_non_auth_errors_publish_only_applicable_challenges() {
    let forbidden = crate::mcp::envelope::build_error_extra(
        "gateway",
        "write",
        "forbidden",
        "need write",
        &serde_json::json!({"required_scopes": ["lab:admin"]}),
    );
    let challenge = error_result_from_envelope(forbidden).meta.unwrap().0;
    assert_eq!(
        challenge["mcp/www_authenticate"],
        serde_json::json!([
            "Bearer error=\"insufficient_scope\", error_description=\"need write\", scope=\"lab:admin\""
        ])
    );

    let other = crate::mcp::envelope::build_error("gateway", "read", "not_found", "missing");
    assert!(error_result_from_envelope(other).meta.is_none());
}

#[test]
fn estimate_tokens_value_serializes_first() {
    // Value's serialized form is `{"a":1}` (7 chars) → 2 tokens.
    let v = serde_json::json!({"a": 1});
    assert_eq!(estimate_tokens_value(&v), 2);
}

#[test]
fn estimate_tokens_args_handles_empty_and_populated_maps() {
    let empty: serde_json::Map<String, Value> = serde_json::Map::new();
    // "{}" → 2 chars → 1 token.
    assert_eq!(estimate_tokens_args(&empty), 1);

    let mut populated = serde_json::Map::new();
    populated.insert("name".into(), Value::String("code_mode".into()));
    // `{"name":"code_mode"}` is 20 chars → 5 tokens.
    assert_eq!(estimate_tokens_args(&populated), 5);
}

#[test]
fn format_dispatch_result_preserves_text_and_structured_content() {
    let payload = serde_json::json!({"kind": "server_logs", "entries": []});

    let (result, outcome) = format_dispatch_result(
        Ok(payload),
        "server_logs",
        "server_logs.query",
        12,
        "subject",
        None,
        2,
    );

    assert!(matches!(
        outcome,
        crate::mcp::logging::DispatchLogOutcome::Success
    ));
    assert_eq!(result.content.len(), 1);
    let structured = result
        .structured_content
        .expect("dispatch success should expose structured content");
    assert_eq!(structured["service"], "server_logs");
    assert_eq!(structured["action"], "server_logs.query");
    assert_eq!(structured["data"]["kind"], "server_logs");
}

#[test]
fn format_dispatch_error_preserves_text_and_structured_content() {
    let (result, outcome) = format_dispatch_result(
        Err(anyhow::Error::from(ToolError::UnknownAction {
            message: "unknown action `server_logs.bad`".to_string(),
            valid: vec!["help".to_string(), "server_logs.query".to_string()],
            hint: None,
        })),
        "server_logs",
        "bad",
        7,
        "subject",
        None,
        2,
    );

    assert!(matches!(
        outcome,
        crate::mcp::logging::DispatchLogOutcome::Failure { .. }
    ));
    assert_eq!(result.content.len(), 1);
    let text = result.content[0]
        .as_text()
        .expect("error result should include text content")
        .text
        .as_str();
    assert!(text.contains("\"ok\":false"));
    let structured = result
        .structured_content
        .expect("dispatch error should expose structured content");
    assert_eq!(structured["ok"], false);
    assert_eq!(structured["service"], "server_logs");
    assert_eq!(structured["error"]["kind"], "unknown_action");
}

#[tokio::test]
async fn extract_error_info_preserves_unknown_action_from_real_dispatch_downcast() {
    let err = crate::dispatch::lab_admin::dispatch("definitely.unknown", serde_json::json!({}))
        .await
        .expect_err("unknown lab_admin action should fail");
    let dispatch_error = DispatchError::from(err);
    let anyhow_error = anyhow::Error::from(dispatch_error);

    let (kind, message, extra) = extract_error_info(&anyhow_error);

    assert_eq!(kind, "unknown_action");
    assert_eq!(message, "unknown action `lab_admin.definitely.unknown`");
    let extra = extra.expect("unknown_action should preserve valid action extras");
    assert_eq!(extra["valid"][0], "help");
    assert_eq!(extra["param"], Value::Null);
    assert_eq!(extra["hint"], Value::Null);
}

#[test]
fn extract_error_info_preserves_unknown_action_from_json_fallback() {
    let serialized = serde_json::json!({
        "kind": "unknown_action",
        "message": "unknown action `status.gt` for service `gateway_alpha`",
        "valid": ["status.get", "status.update"],
        "hint": "status.get"
    })
    .to_string();
    let anyhow_error = anyhow::anyhow!(serialized);

    let (kind, message, extra) = extract_error_info(&anyhow_error);

    assert_eq!(kind, "unknown_action");
    assert_eq!(
        message,
        "unknown action `status.gt` for service `gateway_alpha`"
    );
    let extra = extra.expect("json fallback should preserve structured extras");
    assert_eq!(
        extra["valid"],
        serde_json::json!(["status.get", "status.update"])
    );
    assert_eq!(extra["param"], Value::Null);
    assert_eq!(extra["hint"], serde_json::json!("status.get"));
}

/// Every kind that `ToolError::kind()` can return must have an explicit arm
/// in `canonical_kind()`.  If a new variant or SDK kind is added to `ToolError`
/// without a matching arm here, this test will catch the silent downgrade to
/// `"internal_error"`.
#[test]
fn canonical_kind_round_trips_all_tool_error_kinds() {
    // Fixed-variant kinds — produced by the named ToolError variants.
    let fixed_variants: &[ToolError] = &[
        ToolError::UnknownAction {
            message: String::new(),
            valid: vec![],
            hint: None,
        },
        ToolError::MissingParam {
            message: String::new(),
            param: "p".into(),
        },
        ToolError::InvalidParam {
            message: String::new(),
            param: "p".into(),
        },
        ToolError::UnknownInstance {
            message: String::new(),
            valid: vec![],
        },
    ];

    for err in fixed_variants {
        let kind = err.kind();
        assert_eq!(
            canonical_kind(kind),
            kind,
            "canonical_kind({kind:?}) should round-trip but returns \"{}\"",
            canonical_kind(kind),
        );
    }

    // SDK-promoted kinds — every stable kind tag that `ApiError::kind()` can
    // return and that `ToolError::Sdk` promotes to the top-level `kind` field.
    let sdk_kinds: &[&str] = &[
        "unknown_action",
        "unknown_subaction",
        "missing_param",
        "invalid_param",
        "unknown_instance",
        "auth_failed",
        "not_found",
        "rate_limited",
        "validation_failed",
        "network_error",
        "server_error",
        "decode_error",
        "confirmation_required",
        "http_only",
    ];

    for &sdk_kind in sdk_kinds {
        let err = ToolError::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message: String::new(),
        };
        let kind = err.kind();
        assert_eq!(
            canonical_kind(kind),
            kind,
            "canonical_kind({kind:?}) should round-trip but returns \"{}\"",
            canonical_kind(kind),
        );
    }
}

#[test]
fn extract_error_info_preserves_http_only_from_json_fallback() {
    let serialized = serde_json::json!({
        "kind": "http_only",
        "message": "fs.preview is not available on the MCP surface; use GET /v1/fs/preview"
    })
    .to_string();
    let anyhow_error = anyhow::anyhow!(serialized);

    let (kind, message, extra) = extract_error_info(&anyhow_error);

    assert_eq!(kind, "http_only");
    assert_eq!(
        message,
        "fs.preview is not available on the MCP surface; use GET /v1/fs/preview"
    );
    assert!(extra.is_none());
}

#[cfg(feature = "gateway")]
#[test]
fn code_mode_error_envelope_preserves_refined_metadata() {
    use labby_codemode::{
        CodeModeCallError, CodeModeErrorEvidence, CodeModeErrorOrigin, CodeModeToolSafetyHints,
    };

    // `rate_limited` recomputed from the bare kind would give origin
    // `budget`; the tool_execution constructor refines it to `tool_execution`
    // and carries a retry hint. The envelope must preserve both.
    let error = CodeModeCallError::tool_execution(
        "alpha::demo",
        "rate_limited",
        None,
        "slow down",
        CodeModeErrorEvidence::default(),
        CodeModeToolSafetyHints {
            read_only_hint: Some(true),
            ..CodeModeToolSafetyHints::default()
        },
        Some(1500),
    );
    assert_eq!(error.origin, CodeModeErrorOrigin::ToolExecution);

    let envelope = super::code_mode_error_envelope("codemode", "call_tool", &error);

    assert_eq!(envelope["error"]["kind"], "rate_limited");
    assert_eq!(envelope["error"]["origin"], "tool_execution");
    assert_eq!(envelope["error"]["side_effects"], "none_expected");
    assert_eq!(envelope["error"]["recovery"]["retry_after_ms"], 1500);
    assert_eq!(envelope["error"]["safety"]["read_only_hint"], true);
}

#[test]
fn tool_error_envelope_preserves_structured_extras() {
    let err = ToolError::MissingParam {
        message: "query is required".to_string(),
        param: "query".to_string(),
    };

    let envelope = tool_error_envelope("codemode", "call_tool", &err);

    assert_eq!(
        envelope.pointer("/error/kind"),
        Some(&Value::from("missing_param"))
    );
    assert_eq!(
        envelope.pointer("/error/param"),
        Some(&Value::from("query"))
    );
}

#[cfg(feature = "gateway")]
#[test]
fn tool_error_envelope_preserves_contract_variant_metadata() {
    use labby_codemode::{
        CodeModeCallError, CodeModeErrorEvidence, CodeModeErrorOrigin, CodeModeToolSafetyHints,
    };

    // The snippets.run dispatch path collapses a CodeModeCallError into
    // `ToolError::Contract`; the generic product envelope builder must render
    // the refined metadata (not a kind-recomputed downgrade) plus evidence.
    let error = CodeModeCallError::tool_execution(
        "alpha::demo",
        "rate_limited",
        Some("429".to_string()),
        "slow down",
        CodeModeErrorEvidence {
            content: vec![serde_json::json!({"type":"text","text":"slow down"})],
            ..CodeModeErrorEvidence::default()
        },
        CodeModeToolSafetyHints {
            read_only_hint: Some(true),
            ..CodeModeToolSafetyHints::default()
        },
        Some(1500),
    );
    assert_eq!(error.origin, CodeModeErrorOrigin::ToolExecution);
    let err = error.into_contract_tool_error();

    let envelope = tool_error_envelope("snippets", "snippets.run", &err);

    assert_eq!(envelope["service"], "snippets");
    assert_eq!(envelope["action"], "snippets.run");
    assert_eq!(envelope["error"]["kind"], "rate_limited");
    assert_eq!(envelope["error"]["origin"], "tool_execution");
    assert_eq!(envelope["error"]["side_effects"], "none_expected");
    assert_eq!(envelope["error"]["recovery"]["retry_after_ms"], 1500);
    assert_eq!(envelope["error"]["safety"]["read_only_hint"], true);
    assert_eq!(envelope["error"]["tool"], "alpha::demo");
    assert_eq!(envelope["error"]["original_kind"], "429");
    assert_eq!(
        envelope["error"]["evidence"]["content"][0]["text"],
        "slow down"
    );
}
