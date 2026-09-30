//! Lane A SDK wire fixtures, not durable-route/restart proof.
//! Existing Labby lifecycle/classifier tests are run separately without copying private modules.

#[path = "issue_771_protocol/fixture.rs"]
mod fixture;
#[path = "issue_771_protocol/http_headers.rs"]
mod http_headers;
#[path = "issue_771_protocol/lifecycle.rs"]
mod lifecycle;
#[path = "issue_771_protocol/subscription_gap.rs"]
mod subscription_gap;

use fixture::*;
use rmcp::model::{
    ClientCapabilities, CreateTaskResult, DetailedTask, GetTaskResult, ServerCapabilities,
    ServerResult, Task,
};
use serde_json::json;

#[test]
fn tasks_advertisement_uses_sdk_extension_helpers() {
    for caps in [
        serde_json::to_value(client_info().capabilities).unwrap(),
        serde_json::to_value(ServerCapabilities::builder().enable_tasks().build()).unwrap(),
    ] {
        assert_eq!(caps["extensions"][TASKS], json!({}));
        assert!(
            caps.get("tasks").is_none(),
            "not a legacy top-level capability"
        );
    }
    assert!(!ClientCapabilities::default().supports_tasks());
    assert!(!ServerCapabilities::default().supports_tasks());
}

#[test]
fn task_result_discriminators_and_status_payloads_are_flat() {
    for status in [
        "working",
        "input_required",
        "completed",
        "failed",
        "cancelled",
    ] {
        let task: DetailedTask = serde_json::from_value(detailed(status)).unwrap();
        let value = serde_json::to_value(GetTaskResult::new(task)).unwrap();
        assert_eq!(value["resultType"], "complete");
        assert_eq!(value["status"], status);
        assert!(value.get("task").is_none());
        assert_eq!(
            serde_json::from_value::<GetTaskResult>(value.clone())
                .unwrap()
                .task
                .task
                .task_id,
            "fixture-native-opaque-id"
        );
        match status {
            "completed" => assert!(value.get("result").is_some()),
            "failed" => assert!(value.get("error").is_some()),
            "input_required" => assert!(value.get("inputRequests").is_some()),
            _ => {}
        }
    }
    let task: Task = serde_json::from_value(detailed("working")).unwrap();
    let create = serde_json::to_value(CreateTaskResult::new(task)).unwrap();
    assert_eq!(create["resultType"], "task");
    assert!(matches!(
        serde_json::from_value::<ServerResult>(create).unwrap(),
        ServerResult::CreateTaskResult(_)
    ));
    assert!(
        serde_json::from_value::<CreateTaskResult>(
            json!({"resultType": "task", "task": detailed("working")})
        )
        .is_err()
    );
}

#[tokio::test]
async fn immediate_get_and_cooperative_acknowledgements_use_modern_wire_shapes() {
    let (mut wire, server) = task_wire(true, false);
    let discover = exchange(&mut wire, 1, "server/discover", json!({}), true).await;
    assert_eq!(
        discover["result"]["capabilities"]["extensions"][TASKS],
        json!({})
    );
    let create = exchange(&mut wire, 2, "tools/call", json!({"name": "task"}), true).await;
    assert_eq!(create["result"]["resultType"], "task");
    let id = create["result"]["taskId"].as_str().expect("task handle");
    let get = exchange(&mut wire, 3, "tasks/get", json!({"taskId": id}), true).await;
    assert_eq!(get["result"]["taskId"], id, "no retry before first get");
    assert_eq!(get["result"]["resultType"], "complete");
    for (request_id, method) in [(4, "tasks/update"), (5, "tasks/cancel")] {
        let mut params = json!({"taskId": id});
        if method == "tasks/update" {
            params["inputResponses"] = json!({});
        }
        let ack = exchange(&mut wire, request_id, method, params, true).await;
        assert_eq!(ack["result"], json!({"resultType": "complete"}));
    }
    let after_cancel = exchange(&mut wire, 6, "tasks/get", json!({"taskId": id}), true).await;
    assert_eq!(
        after_cancel["result"]["status"], "working",
        "ack is not terminal-state proof"
    );
    close(&mut wire, server).await;
}

#[tokio::test]
async fn tasks_capabilities_are_enforced_on_each_task_request() {
    let (mut wire, server) = task_wire(true, false);
    exchange(&mut wire, 1, "server/discover", json!({}), true).await;
    for (index, (method, params)) in [
        ("tools/call", json!({"name": "task"})),
        ("tasks/get", json!({"taskId": "unknown"})),
        (
            "tasks/update",
            json!({"taskId": "unknown", "inputResponses": {}}),
        ),
        ("tasks/cancel", json!({"taskId": "unknown"})),
    ]
    .into_iter()
    .enumerate()
    {
        let response = exchange(&mut wire, index as i64 + 2, method, params, false).await;
        assert_eq!(response["error"]["code"], -32021, "{method}: {response}");
        assert_eq!(
            response["error"]["data"]["requiredCapabilities"]["extensions"][TASKS],
            json!({})
        );
        assert!(response.get("result").is_none());
    }
    close(&mut wire, server).await;
}

#[tokio::test]
async fn absent_server_extension_and_removed_modern_task_methods_are_not_found() {
    for advertise in [false, true] {
        let (mut wire, server) = task_wire(advertise, false);
        exchange(&mut wire, 1, "server/discover", json!({}), true).await;
        let methods = if advertise {
            vec!["tasks/list", "tasks/result"]
        } else {
            vec!["tasks/get", "tasks/update", "tasks/cancel"]
        };
        for (index, method) in methods.into_iter().enumerate() {
            let response = exchange(
                &mut wire,
                index as i64 + 2,
                method,
                json!({"taskId": "unknown", "inputResponses": {}}),
                true,
            )
            .await;
            assert_eq!(response["error"]["code"], -32601, "{method}: {response}");
        }
        close(&mut wire, server).await;
    }
}

#[tokio::test]
async fn handler_failure_cannot_become_a_task_acknowledgement() {
    let (mut wire, server) = task_wire(true, true);
    exchange(&mut wire, 1, "server/discover", json!({}), true).await;
    let response = exchange(&mut wire, 2, "tools/call", json!({"name": "task"}), true).await;
    assert_eq!(response["error"]["code"], -32603);
    assert!(response.get("result").is_none());
    close(&mut wire, server).await;
}
