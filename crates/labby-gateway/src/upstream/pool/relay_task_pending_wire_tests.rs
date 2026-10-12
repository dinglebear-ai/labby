//! Independent pinned-model byte oracle for borrowed notification sizing.

use std::collections::BTreeMap;
use std::time::Duration;

use rmcp::model::{DetailedTask, TaskStatus};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::super::{PendingTaskBuffer, PendingTaskLimits, notification_json_size};
use super::*;

fn object(value: Value) -> JsonObject {
    serde_json::from_value(value).unwrap()
}

fn params(payload: TaskPayload) -> TaskStatusNotificationParams {
    TaskStatusNotificationParams::new(DetailedTask::new(
        Task::new("id-\"-é", TaskStatus::Working, "created\n", "updated\t"),
        payload,
    ))
}

fn input_requests() -> InputRequests {
    serde_json::from_value(json!({
        "ask-\"-é": {
            "method": "elicitation/create",
            "params": {"mode": "url", "message": "é\n\"", "url": "https://example.test/",
                "elicitationId": "ask"}
        }
    }))
    .unwrap()
}

fn payloads() -> [TaskPayload; 5] {
    [
        TaskPayload::Working,
        TaskPayload::Cancelled,
        TaskPayload::Completed {
            result: object(json!({"content": [
                {"type": "text", "text": "é\n\"", "_meta": {"item": [1, "é"]}}
            ], "_meta": {"result": true}, "isError": false})),
        },
        TaskPayload::Failed {
            error: object(json!({"code": -32603, "message": "é\n\"",
                "data": {"_meta": {"item": true}, "values": [null, false, 4]}})),
        },
        TaskPayload::InputRequired {
            input_requests: input_requests(),
        },
    ]
}

fn equivalent(params: &TaskStatusNotificationParams) {
    // The model oracle intentionally uses its serializer on tiny test values.
    let expected = serde_json::to_vec(params).unwrap();
    let actual = serde_json::to_vec(&TaskParamsWire::new(params).unwrap()).unwrap();
    assert_eq!(
        actual,
        expected,
        "wire mismatch for {:?}",
        params.task.status()
    );
    assert_eq!(
        notification_json_size(params, expected.len()),
        Some(expected.len())
    );
    assert!(notification_json_size(params, expected.len() - 1).is_none());
}

#[test]
fn pending_task_borrowed_wire_matches_model_for_all_states_and_item_metadata() {
    for payload in payloads() {
        for metadata in 0..3 {
            let mut event = params(payload.clone());
            if metadata > 0 {
                event.meta = Some(NotificationMetaObject::new());
                event.task.task.status_message = Some("é\n\"\\".to_owned());
                event.task.task.ttl_ms = Some(321);
                event.task.task.poll_interval_ms = Some(17);
            }
            if metadata == 2 {
                event.meta = Some(NotificationMetaObject::from(object(json!({
                    "notification": {"nested": ["é\n\"", null]},
                    "io.modelcontextprotocol/subscriptionId": 77
                }))));
            }
            equivalent(&event);
        }
    }
    for payload in [
        TaskPayload::Completed {
            result: JsonObject::new(),
        },
        TaskPayload::Failed {
            error: JsonObject::new(),
        },
        TaskPayload::InputRequired {
            input_requests: InputRequests::new(),
        },
    ] {
        let mut event = params(payload);
        // The model serializer preserves the actual base status even if a
        // caller mutates it after DetailedTask::new; sizing must match it.
        event.task.task.status = TaskStatus::Working;
        equivalent(&event);
    }
}

fn extension_meta(extensions: &mut Extensions, mask: u8) {
    if mask & 2 != 0 {
        extensions.insert(MetaObject(object(
            json!({"legacy": 2, "conflict": "legacy"}),
        )));
    }
    if mask & 4 != 0 {
        extensions.insert(RequestMetaObject::from(object(json!({
            "typed": 3, "conflict": "typed", "legacy": "typed-wins"
        }))));
    }
}

fn request_meta(request: &mut InputRequest, mask: u8) {
    let meta = (mask & 1 != 0).then(|| {
        RequestMetaObject::from(object(json!({
            "params": 1, "conflict": "params", "nested": {"item": "é\n\""}
        })))
    });
    match request {
        InputRequest::CreateMessage(request) => {
            *request.params.meta_mut() = meta;
            extension_meta(&mut request.extensions, mask);
        }
        InputRequest::Elicitation(request) => {
            *request.params.meta_mut() = meta;
            extension_meta(&mut request.extensions, mask);
            if let ElicitRequestParams::FormElicitationParams {
                requested_schema, ..
            } = &mut request.params
            {
                // Match SDK's ordered conversion with a duplicate/missing
                // entry and a property omitted from the explicit order.
                requested_schema.property_order =
                    Some(vec!["z".to_owned(), "missing".to_owned(), "z".to_owned()]);
            }
        }
        InputRequest::ListRoots(request) => extension_meta(&mut request.extensions, mask),
        _ => unreachable!("fixture variants are fixed"),
    }
}

fn request_fixtures() -> Vec<Value> {
    vec![
        json!({"method": "roots/list"}),
        json!({"method": "elicitation/create", "params": {"mode": "url",
            "message": "é\n\"", "url": "https://example.test/", "elicitationId": "url-id"}}),
        json!({"method": "elicitation/create", "params": {"mode": "form", "message": "é\n\"",
            "requestedSchema": {"$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object", "title": "é\n\"", "properties": {
                    "z": {"type": "string", "description": "é\n\""},
                    "a": {"type": "boolean"}}, "required": ["z"], "description": "form"}}}),
        json!({"method": "sampling/createMessage", "params": {"messages": [], "maxTokens": 42}}),
        json!({"method": "sampling/createMessage", "params": {
            "messages": [{"role": "user", "_meta": {"message": true}, "content": [
                {"type": "text", "text": "é\n\"", "_meta": {"item": [1, true]}}]}],
            "modelPreferences": {"hints": [{"name": "model"}], "costPriority": 0.3,
                "speedPriority": 0.6, "intelligencePriority": 0.9},
            "systemPrompt": "é\n\"", "includeContext": "thisServer", "temperature": 0.7,
            "maxTokens": 42, "stopSequences": ["é\n\""], "metadata": {"nested": [1, true]},
            "tools": [{"name": "tool", "inputSchema": {"type": "object"},
                "_meta": {"tool": true}}], "toolChoice": {"mode": "auto"}}}),
    ]
}

#[test]
fn pending_task_borrowed_wire_matches_input_requests_and_metadata_precedence() {
    for fixture in request_fixtures() {
        for mask in 0..8 {
            let mut request: InputRequest = serde_json::from_value(fixture.clone()).unwrap();
            request_meta(&mut request, mask);
            let inputs = BTreeMap::from([("input-\"-é".to_owned(), request)]);
            let event = params(TaskPayload::InputRequired {
                input_requests: inputs,
            });
            equivalent(&event);
        }
    }
}

#[test]
fn pending_task_borrowed_wire_rejects_oversized_payloads_without_retaining_them() {
    let limits = PendingTaskLimits {
        ids: 1,
        per_id: 1,
        count: 1,
        bytes: 512,
        ttl: Duration::from_secs(1),
    };
    let now = Instant::now();
    let mut buffer = PendingTaskBuffer::default();
    let large = "é\n\"".repeat(1_024);
    for payload in [
        TaskPayload::Completed {
            result: object(json!({"content": [{
            "type": "text", "text": large, "_meta": {"item": true}}]})),
        },
        TaskPayload::Failed {
            error: object(json!({"message": large,
            "data": {"_meta": {"item": true}}})),
        },
        TaskPayload::InputRequired {
            input_requests: serde_json::from_value(json!({
                "ask": {"method": "elicitation/create", "params": {"mode": "url",
                    "message": large, "url": "https://example.test/", "elicitationId": "ask"}}
            }))
            .unwrap(),
        },
    ] {
        let event = params(payload);
        assert!(notification_json_size(&event, limits.bytes).is_none());
        buffer.admit(event, limits, now);
        assert_eq!(buffer.count, 0);
        assert_eq!(buffer.bytes, 0);
        assert!(buffer.ids.is_empty());
    }
    let mut oversized_meta = params(TaskPayload::Working);
    oversized_meta.meta = Some(NotificationMetaObject::from(object(
        json!({"payload": large}),
    )));
    buffer.admit(oversized_meta, limits, now);
    assert!(buffer.ids.is_empty());
    buffer.admit(params(TaskPayload::Working), limits, now);
    assert_eq!(buffer.drain("id-\"-é", now, limits.ttl).len(), 1);
}

#[test]
fn pending_task_borrowed_view_references_original_task_metadata_and_payload() {
    for payload in payloads() {
        let mut event = params(payload);
        event.meta = Some(NotificationMetaObject::new());
        let wire = TaskParamsWire::new(&event).unwrap();
        assert!(std::ptr::eq(
            wire.task,
            std::ptr::from_ref(&event.task.task)
        ));
        assert!(std::ptr::eq(
            wire.meta.unwrap(),
            event.meta.as_ref().unwrap()
        ));
        match &event.task.payload {
            TaskPayload::Completed { result } => {
                assert!(std::ptr::eq(wire.result.unwrap(), result))
            }
            TaskPayload::Failed { error } => assert!(std::ptr::eq(wire.error.unwrap(), error)),
            TaskPayload::InputRequired { input_requests } => {
                assert!(std::ptr::eq(wire.input_requests.unwrap().0, input_requests));
            }
            _ => {}
        }
    }
}
