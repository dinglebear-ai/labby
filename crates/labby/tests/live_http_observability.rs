#![cfg(feature = "gateway")]

#[path = "support/lib.rs"]
mod support;

use std::time::{Duration, Instant};

use reqwest::{Client, StatusCode};
use support::LiveLabbyBuilder;

#[tokio::test]
async fn matched_route_observability_records_template_group_and_handler() {
    let guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("start live Labby");
    let request_id = "route-observability-health";
    let response = Client::new()
        .get(format!("{}/health", guard.connection().base_url))
        .header("x-request-id", request_id)
        .send()
        .await
        .expect("health request");
    assert_eq!(response.status(), StatusCode::OK);

    let deadline = Instant::now() + Duration::from_secs(2);
    let evidence = loop {
        let stdout = std::fs::read_to_string(guard.root().join("stdout.log")).unwrap_or_default();
        let stderr = std::fs::read_to_string(guard.root().join("stderr.log")).unwrap_or_default();
        let evidence = format!("{stdout}\n{stderr}");
        if evidence.contains(request_id) && evidence.contains("matched_route") {
            break evidence;
        }
        assert!(
            Instant::now() < deadline,
            "matched-route evidence absent: {evidence}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    for expected in [request_id, "/health", "route_group", "health", "handler"] {
        assert!(
            evidence.contains(expected),
            "matched-route evidence omitted {expected}: {evidence}"
        );
    }

    let cleanup = guard.finish().await;
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
}
