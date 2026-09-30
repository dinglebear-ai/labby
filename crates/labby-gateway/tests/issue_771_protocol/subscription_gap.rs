//! Explicit characterization of an unresolved SDK gap, NOT conformance success.
//! Set LABBY_LANE_A_REQUIRE_TASK_SUBSCRIPTIONS=1 for the strict failing oracle.
//! Repair belongs in a coordinated SDK patch and Lane D integration, not pool glue.

use super::fixture::{DEADLINE, finish, metadata, task_stream};
use rmcp::model::SubscriptionFilter;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

#[test]
fn known_sdk_gap_subscription_filter_discards_task_ids() {
    let filter: SubscriptionFilter =
        serde_json::from_value(json!({"taskIds": ["task-a"]})).unwrap();
    assert!(
        serde_json::to_value(filter)
            .unwrap()
            .get("taskIds")
            .is_none(),
        "SDK now preserves taskIds: replace this gap characterization with conformance coverage"
    );
}

#[tokio::test]
async fn known_sdk_gap_raw_task_subscription_is_acknowledged_without_capability() {
    let (stream, server) = task_stream(true, false);
    let mut wire = BufReader::new(stream);
    // Establish discovery first; pre-init request handling is a separate lifecycle concern.
    let mut discovery = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 70, "method": "server/discover",
        "params": {"_meta": metadata(false)}
    }))
    .unwrap();
    discovery.push(b'\n');
    timeout(DEADLINE, wire.get_mut().write_all(&discovery))
        .await
        .unwrap()
        .unwrap();
    let mut discovered = String::new();
    timeout(DEADLINE, wire.read_line(&mut discovered))
        .await
        .unwrap()
        .unwrap();
    let discovered: Value = serde_json::from_str(&discovered).unwrap();
    assert_eq!(discovered["id"], 70);
    assert!(discovered["result"].is_object());
    // Raw JSON bypasses the client SDK model, proving loss at the server boundary.
    let mut request = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 71, "method": "subscriptions/listen",
        "params": {"notifications": {"taskIds": ["task-a"]}, "_meta": metadata(false)}
    }))
    .unwrap();
    request.push(b'\n');
    timeout(DEADLINE, wire.get_mut().write_all(&request))
        .await
        .unwrap()
        .unwrap();
    let mut line = String::new();
    let count = timeout(DEADLINE, wire.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(count > 0, "server must produce a response");
    let observed: Value = serde_json::from_str(&line).unwrap();
    println!("task_subscription_observed={observed}");
    if std::env::var_os("LABBY_LANE_A_REQUIRE_TASK_SUBSCRIPTIONS").is_some() {
        assert_eq!(
            observed["error"]["code"], -32021,
            "Tasks specification requires capability rejection, not an empty subscription ack"
        );
    } else {
        assert_eq!(
            observed["method"],
            "notifications/subscriptions/acknowledged"
        );
        assert!(
            observed["params"]["notifications"].get("taskIds").is_none(),
            "SDK behavior changed: requalify and remove the documented gap"
        );
    }
    drop(wire);
    finish(server).await;
}
