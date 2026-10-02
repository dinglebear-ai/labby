//! Exact builder-generator output executes in the product QuickJS fixture runner.
#![cfg(feature = "gateway")]
use labby_codemode::snippet::store;
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn generated_workflow_batches_roots_maps_outputs_and_skips_failed_descendants() {
    let home = if cfg!(target_os = "macos") {
        tempfile::Builder::new()
            .prefix("lwf-")
            .tempdir_in("/private/tmp")
    } else {
        tempfile::tempdir()
    }
    .expect("isolated short home");
    let source = format!(
        "---\nname: workflow-builder\ndescription: Generated workflow integration\ninputs:\n  query:\n    type: string\n    required: true\ntools: [\"fixture::lookup\", \"fixture::independent\", \"fixture::consume\", \"fixture::blocked\"]\n---\n\n```javascript\n{}\n```\n",
        include_str!("fixtures/workflow-builder.js")
    );
    store::create_user_snippet(home.path(), "workflow-builder", &source, None, false)
        .expect("install exact generated source");
    let cases = [
        (
            json!({
                "calls": [
                    {"tool": "fixture::lookup", "match": {"query": "find-me"}, "result": {"items": [{"id": "resolved-id"}]}},
                    {"tool": "fixture::independent", "result": {"independent": true}},
                    {"tool": "fixture::consume", "match": {"id": "resolved-id"}, "result": {"consumed": true}},
                    {"tool": "fixture::blocked", "result": {"finished": true}}
                ],
                "expect": {"/ok": true, "/all_ok": true, "/steps/0/status": "succeeded", "/steps/1/status": "succeeded", "/steps/2/status": "succeeded", "/steps/3/status": "succeeded"},
                "budgets": {"tool_calls": 4}
            }),
            4,
        ),
        (
            json!({
                "calls": [
                    {"tool": "fixture::lookup", "match": {"query": "find-me"}, "error": {"kind": "upstream_error", "message": "Synthetic prerequisite failure"}},
                    {"tool": "fixture::independent", "result": {"independent": true}}
                ],
                "expect": {"/ok": false, "/all_ok": false, "/steps/0/status": "failed", "/steps/1/status": "succeeded", "/steps/2/status": "skipped", "/steps/2/dependencies": ["lookup"], "/steps/3/status": "skipped", "/steps/3/dependencies": ["consume"]},
                "budgets": {"tool_calls": 2}
            }),
            2,
        ),
        (
            json!({
                "calls": [
                    {"tool": "fixture::lookup", "result": {"items": []}},
                    {"tool": "fixture::independent", "result": {"independent": true}}
                ],
                "expect": {"/ok": false, "/all_ok": false, "/steps/0/status": "succeeded", "/steps/1/status": "succeeded", "/steps/2/status": "failed", "/steps/3/status": "skipped", "/steps/3/dependencies": ["consume"]},
                "budgets": {"tool_calls": 2}
            }),
            2,
        ),
    ];
    for (index, (fixture, calls)) in cases.into_iter().enumerate() {
        let path = home.path().join(format!("workflow-{index}.test.json"));
        std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
                .args([
                    "--json",
                    "snippet",
                    "test",
                    "workflow-builder",
                    "--param",
                    "query=find-me",
                    "--fixture",
                ])
                .arg(path)
                .env_clear()
                .env("HOME", home.path())
                .env("LABBY_HOME", home.path())
                .env("TMPDIR", home.path())
                .env("LABBY_CODE_MODE_RUNNER_BACKEND", "process")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("bounded product process")
        .expect("offline workflow execution");
        assert!(
            output.status.success(),
            "scenario {index}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).expect("fixture report");
        assert_eq!(report["passed"], true, "{report}");
        assert_eq!(report["metrics"]["tool_calls"], calls, "{report}");
        assert_eq!(
            report["metrics"]["max_in_flight"], 2,
            "independent roots must batch: {report}"
        );
        assert_eq!(report["calls"].as_array().unwrap().len(), calls as usize);
    }
}
