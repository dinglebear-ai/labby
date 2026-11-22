use super::*;
use crate::dispatch::codemode_notices::{CodeModeNotice, NoticeLevel};
use serde_json::json;
fn result(error: bool) -> CallToolResult {
    let text = if error {
        json!({"error":{"kind":"execution_error","message":"boom"}})
    } else {
        json!({"result":{"ok":true},"calls":[],"logs":[]})
    };
    let mut value = if error {
        CallToolResult::error(vec![ContentBlock::text(text.to_string())])
    } else {
        CallToolResult::success(vec![ContentBlock::text(text.to_string())])
    };
    value.structured_content = Some(
        json!({"kind":"code_mode_execute_trace","result":{"ok":true},"call_count":0,"calls":[],"result_shape":{"type":"object"},"logs_count":0}),
    );
    value.meta = Some(rmcp::model::MetaObject(
        json!({"ui":{"resourceUri":"ui://test/example"}})
            .as_object()
            .unwrap()
            .clone(),
    ));
    value
}
fn batch() -> NoticeBatch {
    NoticeBatch {
        notifications: vec![CodeModeNotice {
            id: format!("notice_{}", ulid::Ulid::new()),
            source: "test".into(),
            level: NoticeLevel::Warning,
            message: "Indexing finished.".into(),
            message_id: None,
            delivery_attempt: 1,
            expires_at_unix_ms: 10_000,
        }],
        notifications_remaining: 0,
        notifications_are_advisory: true,
    }
}
#[test]
fn notice_production_text_structured_schema_and_metadata() {
    let mut before = result(false);
    before.content[0] = serde_json::from_value(json!({
        "type": "text",
        "text": before.content[0].as_text().unwrap().text,
        "annotations": {"priority": 0.5},
        "_meta": {"source": "preserve-me"}
    }))
    .unwrap();
    let original_block = serde_json::to_value(&before.content[0]).unwrap();
    let batch = batch();
    let response = decorated(&before, &Map::new(), Some(&batch), 24_000).unwrap();
    let text: Value = serde_json::from_str(&response.content[0].as_text().unwrap().text).unwrap();
    let structured = response.structured_content.as_ref().unwrap();
    assert_eq!(text["result"], json!({"ok":true}));
    assert_eq!(text["notifications"], structured["notifications"]);
    let decorated_block = serde_json::to_value(&response.content[0]).unwrap();
    assert_eq!(
        decorated_block["annotations"],
        original_block["annotations"]
    );
    assert_eq!(decorated_block["_meta"], original_block["_meta"]);
    assert_eq!(response.meta, before.meta);
    assert_eq!(response.is_error, before.is_error);
    let schema =
        Value::Object((*crate::mcp::handlers_tools::code_mode_trace_output_schema()).clone());
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(structured)
        .unwrap();
}
#[test]
fn notice_production_budget_malformed_collisions_and_error_preserved() {
    let mut original = result(true);
    let before = serde_json::to_value(&original).unwrap();
    let batch = batch();
    assert!(decorated(&original, &Map::new(), Some(&batch), 1).is_none());
    assert!(decorated(&original, &Map::new(), Some(&batch), 4).is_none());
    assert_eq!(serde_json::to_value(&original).unwrap(), before);
    let decorated = decorated(&original, &Map::new(), Some(&batch), 24_000).unwrap();
    assert_eq!(decorated.is_error, Some(true));
    assert!(
        decorated.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("boom")
    );
    original.structured_content.as_mut().unwrap()["notifications"] = json!(["existing"]);
    assert!(super::decorated(&original, &Map::new(), Some(&batch), 24_000).is_none());
    original.content[0] = ContentBlock::text("not JSON");
    assert!(super::decorated(&original, &Map::new(), Some(&batch), 24_000).is_none());
}
#[test]
fn notice_production_control_schema_and_empty_inbox() {
    let fields=json!({"notification_inbox":{"id":format!("inbox_{}",ulid::Ulid::new()),"scope":"authenticated_client","delivery":"at_least_once_until_ack_or_expiry","expires_at_unix_ms":10_000},"acknowledged_notifications":[]}).as_object().unwrap().clone();
    let response = decorated(&result(false), &fields, None, 24_000).unwrap();
    assert!(
        response
            .structured_content
            .as_ref()
            .unwrap()
            .get("notifications")
            .is_none()
    );
    assert!(decorated(&result(false), &Map::new(), None, 24_000).is_none());
    let schema =
        Value::Object((*crate::mcp::handlers_tools::code_mode_trace_output_schema()).clone());
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(response.structured_content.as_ref().unwrap())
        .unwrap();
    let input = Value::Object((*crate::mcp::handlers_tools::code_mode_execute_schema()).clone());
    let validator = jsonschema::validator_for(&input).unwrap();
    validator
        .validate(&json!({"code":"async () => 1","notification_inbox":true,"ack_notifications":[]}))
        .unwrap();
    assert!(
        validator
            .validate(&json!({"code":"async () => 1","notification_inbox":"bad"}))
            .is_err()
    );
}
#[test]
fn notice_production_unsettled_bridge_controls_are_typed_not_an_error() {
    let schema =
        Value::Object((*crate::mcp::handlers_tools::code_mode_trace_output_schema()).clone());
    let validator = jsonschema::validator_for(&schema).unwrap();
    for (error, kind, retryable) in [
        (NoticeError::Busy, "rate_limited", true),
        (NoticeError::Deadline, "timeout", true),
        (NoticeError::Forbidden, "forbidden", false),
    ] {
        let mut marked = result(false);
        mark_unsettled(&mut marked, error, 24_000);
        let expected = json!({"status":"unsettled","kind":kind,"retryable":retryable});
        let structured = marked.structured_content.as_ref().unwrap();
        assert_eq!(structured["notification_controls"], expected);
        let text: Value = serde_json::from_str(&marked.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(text["notification_controls"], expected);
        assert_eq!(
            text["result"],
            json!({"ok":true}),
            "the script result is kept"
        );
        assert_ne!(
            marked.is_error,
            Some(true),
            "a control failure is not a script error"
        );
        validator.validate(structured).unwrap();
    }

    // Like any decoration, it never replaces a colliding field or breaks the budget.
    let mut colliding = result(false);
    colliding.structured_content.as_mut().unwrap()["notification_controls"] = json!("script");
    let before = serde_json::to_value(&colliding).unwrap();
    mark_unsettled(&mut colliding, NoticeError::Busy, 24_000);
    assert_eq!(serde_json::to_value(&colliding).unwrap(), before);
    let mut tight = result(false);
    let before = serde_json::to_value(&tight).unwrap();
    mark_unsettled(&mut tight, NoticeError::Busy, 1);
    assert_eq!(serde_json::to_value(&tight).unwrap(), before);
}

#[test]
fn explicit_controls_take_budget_before_optional_upstream_content() {
    let trace = json!({"result":7,"calls":[{"upstream_content":{"content":[{"type":"text","text":"x".repeat(4000)}],"omitted_blocks":0,"omitted_bytes":0}}]});
    let mut result = CallToolResult::success(vec![ContentBlock::text(trace.to_string())]);
    result.structured_content = Some(trace);
    let fields = Map::from_iter([("acknowledged_notifications".into(), json!(1))]);
    let limited = decorated(&result, &fields, None, 1000).unwrap();
    let body = limited.structured_content.as_ref().unwrap();
    assert_eq!(body["result"], 7);
    assert_eq!(body["acknowledged_notifications"], 1);
    assert_eq!(body["calls"][0]["upstream_content"]["omitted_blocks"], 1);
    let text: Value = serde_json::from_str(&limited.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(text, *body);
}
