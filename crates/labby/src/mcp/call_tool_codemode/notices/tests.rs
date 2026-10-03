use super::*;
use crate::notifications::codemode::{CodeModeNotice, NoticeLevel};
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
    let response = decorated(&before, &Map::new(), Some(&batch), 24_576, 6000).unwrap();
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
    assert!(decorated(&original, &Map::new(), Some(&batch), 1, 6000).is_none());
    assert!(decorated(&original, &Map::new(), Some(&batch), 24_576, 1).is_none());
    assert_eq!(serde_json::to_value(&original).unwrap(), before);
    let decorated = decorated(&original, &Map::new(), Some(&batch), 24_576, 6000).unwrap();
    assert_eq!(decorated.is_error, Some(true));
    assert!(
        decorated.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("boom")
    );
    original.structured_content.as_mut().unwrap()["notifications"] = json!(["existing"]);
    assert!(super::decorated(&original, &Map::new(), Some(&batch), 24_576, 6000).is_none());
    original.content[0] = ContentBlock::text("not JSON");
    assert!(super::decorated(&original, &Map::new(), Some(&batch), 24_576, 6000).is_none());
}
#[test]
fn notice_production_control_schema_and_empty_inbox() {
    let fields=json!({"notification_inbox":{"id":format!("inbox_{}",ulid::Ulid::new()),"scope":"authenticated_client","delivery":"at_least_once_until_ack_or_expiry","expires_at_unix_ms":10_000},"acknowledged_notifications":[]}).as_object().unwrap().clone();
    let response = decorated(&result(false), &fields, None, 24_576, 6000).unwrap();
    assert!(
        response
            .structured_content
            .as_ref()
            .unwrap()
            .get("notifications")
            .is_none()
    );
    assert!(decorated(&result(false), &Map::new(), None, 24_576, 6000).is_none());
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
fn notice_production_full_credential_binding_and_reauthorization_boundary() {
    let static_identity = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "primary",
    )
    .unwrap();
    let (actor, binding) = credential_binding(&static_identity, None).unwrap();
    assert_eq!(actor.len(), 64);
    assert_eq!(binding.len(), 64);
    assert_eq!(
        credential_binding(&static_identity, None).unwrap(),
        (actor.clone(), binding.clone())
    );
    let other = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "secondary",
    )
    .unwrap();
    assert_ne!(credential_binding(&other, None).unwrap().1, binding);
    let product = labby_auth::VerifiedIdentity::local_credential_with_issuer(
        labby_auth::Authenticator::ProductCredential,
        "test-issuer",
        "cred",
    )
    .unwrap();
    assert!(credential_binding(&product, None).is_none());
    let mut grant = labby_primitives::product_credential::BoundAccessGrant {
        installation_id: "install".into(),
        issuer: "test-issuer".into(),
        subject: "subject".into(),
        principal_id: "principal".into(),
        organization_id: "org".into(),
        project_id: "project".into(),
        loadout_id: "loadout".into(),
        loadout_generation: 1,
        assignment_generation: 1,
        catalog_generation: 1,
        route_id: "route".into(),
        route_generation: 1,
        membership_epoch: 1,
        organization_policy_epoch: 1,
        project_policy_epoch: 1,
        credential_id: "cred".into(),
        credential_generation: 1,
        scopes: vec!["lab:read".into()],
        resource: "resource".into(),
        audience: "audience".into(),
        expires_at: 100,
        requires_admin: false,
        destructive: false,
    };
    let original = credential_binding(&product, Some(&grant)).unwrap();
    grant.expires_at += 1;
    grant.catalog_generation += 1;
    assert_eq!(
        credential_binding(&product, Some(&grant)).unwrap(),
        original,
        "renewal/catalog freshness are not a new authority"
    );
    grant.project_id = "another".into();
    assert_ne!(
        credential_binding(&product, Some(&grant)).unwrap(),
        original,
        "project rebinding must isolate inboxes"
    );
    grant.credential_id = "foreign".into();
    assert!(credential_binding(&product, Some(&grant)).is_none());
}
