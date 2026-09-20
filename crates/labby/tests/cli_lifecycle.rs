#![cfg(feature = "gateway")]
//! Lifecycle process contracts use an observed mock gateway, not a fake success flag.
use serde_json::{Value, json};
use std::{
    process::{Output, Stdio},
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

async fn daemon() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/gateway/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"name":"gateway.reload"}])))
        .mount(&server)
        .await;
    action(&server,"gateway.mcp.list",json!([{"name":"one","enabled":true},{"name":"two","enabled":false},{"name":"three","enabled":true}])).await;
    server
}
async fn action(server: &MockServer, action: &str, value: Value) {
    Mock::given(method("POST"))
        .and(path("/v1/gateway"))
        .and(body_partial_json(json!({"action":action})))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .mount(server)
        .await;
}
async fn run(server: &MockServer, args: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .args(["--json", "--server", &server.uri()])
        .args(args)
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path().join(".labby"))
        .env("NO_COLOR", "1")
        .current_dir(home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("lifecycle must finish within its outer test deadline")
        .unwrap();
    if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout)
        && let Some(results) = value["results"].as_array()
    {
        assert!(
            !results.is_empty(),
            "lifecycle fixture must exercise a real selection"
        );
        let request_id = value["request_id"]
            .as_str()
            .expect("partial outcomes need a correlation ID");
        let mut events = Vec::new();
        for entry in std::fs::read_dir(home.path().join(".local/share/labby/logs")).unwrap() {
            for line in std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
            {
                let event: Value = serde_json::from_str(line).unwrap();
                if event["fields"]["action"] == value["action"] {
                    events.push(event);
                }
            }
        }
        assert_eq!(
            events.len(),
            results.len(),
            "every target outcome must be logged at default verbosity"
        );
        for result in results {
            let event = events
                .iter()
                .find(|event| event["fields"]["upstream"] == result["name"])
                .expect("missing lifecycle target log");
            assert_eq!(event["fields"]["request_id"], request_id);
            assert_eq!(event["fields"]["status"], result["status"]);
            assert!(event["fields"]["elapsed_ms"].is_u64());
            if result["status"] == "failed" {
                assert_eq!(event["level"], "WARN");
                assert_eq!(event["fields"]["kind"], result["error"]["kind"]);
            }
        }
    }
    output
}
async fn mutations(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|request| serde_json::from_slice::<Value>(&request.body).ok())
        .filter(|body| {
            body["action"]
                .as_str()
                .is_some_and(|action| action != "gateway.mcp.list")
        })
        .collect()
}

#[tokio::test]
async fn restart_requires_observed_completion_and_never_replays_an_accepted_operation() {
    let server = daemon().await;
    action(&server, "gateway.mcp.restart", json!({"completed":false})).await;
    let output = run(&server, &["server", "restart", "one", "--timeout", "2s"]).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["failed"], 1);
    assert_eq!(result["results"][0]["error"]["kind"], "timeout");
    assert_eq!(result["results"][0]["status"], "failed");
    let calls = mutations(&server).await;
    assert_eq!(calls.len(), 1, "must not replay an accepted restart");
    assert_eq!(calls[0]["params"]["wait_ms"], 2000);
}

#[tokio::test]
async fn explicit_no_wait_reports_acceptance_not_completion() {
    let server = daemon().await;
    action(&server, "gateway.mcp.restart", json!({"completed":false})).await;
    let output = run(&server, &["server", "restart", "one", "--no-wait"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["results"][0]["status"], "accepted");
    let calls = mutations(&server).await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["params"]["wait_ms"], 0);
}

#[tokio::test]
async fn restart_does_not_claim_a_disconnected_replacement_is_healthy() {
    let server = daemon().await;
    action(&server,"gateway.mcp.restart",json!({"completed":true,"gateway":{"config":{"enabled":true},"runtime":{"connected":false}}})).await;
    let output = run(&server, &["server", "restart", "one"]).await;
    assert_eq!(output.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["results"][0]["error"]["kind"], "upstream_error");
    assert_eq!(mutations(&server).await.len(), 1);
}

#[tokio::test]
async fn bulk_restart_preflights_targets_and_reports_each_actual_outcome() {
    let server = daemon().await;
    for (name, connected) in [("one", true), ("three", false)] {
        Mock::given(method("POST")).and(path("/v1/gateway")).and(body_partial_json(json!({"action":"gateway.mcp.restart","params":{"name":name}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completed":true,"gateway":{"config":{"name":name,"enabled":true},"runtime":{"connected":connected}}}))).mount(&server).await;
    }
    let output = run(&server, &["server", "restart", "--all"]).await;
    assert_eq!(output.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["selected"], 2);
    assert_eq!(result["failed"], 1);
    assert_eq!(result["results"][0]["status"], "completed");
    assert_eq!(result["results"][1]["status"], "failed");
    let calls = mutations(&server).await;
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|call| call["params"]["name"] != "two"));
}

#[tokio::test]
async fn invalid_selection_and_preview_never_mutate_any_server() {
    let server = daemon().await;
    for args in [
        vec!["server", "restart"],
        vec!["server", "restart", "one", "one"],
        vec!["server", "restart", "one", "missing"],
        vec!["server", "restart", "two"],
        vec!["server", "restart", "one", "--all"],
    ] {
        let output = run(&server, &args).await;
        assert!(
            !output.status.success(),
            "unexpected selection accepted: {args:?}"
        );
        assert!(mutations(&server).await.is_empty());
    }
    let output = run(&server, &["server", "disable", "--all", "--dry-run"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["executed"], false);
    assert_eq!(preview["params"]["targets"].as_array().unwrap().len(), 3);
    assert!(mutations(&server).await.is_empty());
}
