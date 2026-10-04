//! Failure scenarios run in the product QuickJS subprocess, without a gateway.
use super::*;
use serde_json::json;
use std::time::Duration;

async fn run(name: &str, source: &str, fixture: Value) -> (bool, Value, String) {
    let _product = PRODUCT_EXECUTION.lock().await;
    let home = if cfg!(target_os = "macos") {
        tempfile::Builder::new()
            .prefix("lsf-")
            .tempdir_in("/private/tmp")
    } else {
        tempfile::tempdir()
    }
    .expect("isolated short home");
    store::create_user_snippet(
        home.path(),
        name,
        source,
        Some("Offline failure regression"),
        false,
    )
    .expect("install workflow");
    let path = home.path().join("scenario.test.json");
    std::fs::write(&path, serde_json::to_vec(&fixture).expect("fixture JSON"))
        .expect("fixture file");
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
            .args(["--json", "snippet", "test", name, "--fixture"])
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
    .expect("bounded product fixture")
    .expect("product execution");
    let success = output.status.success();
    let diagnostic = format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (success, report, diagnostic)
}

#[tokio::test]
async fn mixed_batch_retains_two_successes_and_typed_timeout_rejection() {
    let (success, report, diagnostic) = run("mixed-batch", r#"async () => {
        const batch = await codemode.batch([
            () => callTool("fixture::first", {}),
            () => callTool("fixture::slow", {}),
            () => callTool("fixture::last", {})
        ]);
        return {ok: false, successes: batch.ok.map(row => ({index: row.i, value: row.value})),
            failures: batch.failed.map(row => ({index: row.i, kind: row.error.kind, message: row.error.message}))};
    }"#, json!({
        "calls": [
            {"tool": "fixture::first", "result": {"value": 1}},
            {"tool": "fixture::slow", "error": {"kind": "timeout", "message": "Synthetic upstream deadline"}},
            {"tool": "fixture::last", "result": {"value": 3}}
        ],
        "expect": {"/ok": false, "/successes": [{"index": 0, "value": {"value": 1}}, {"index": 2, "value": {"value": 3}}],
            "/failures": [{"index": 1, "kind": "timeout", "message": "Synthetic upstream deadline"}]},
        "budgets": {"tool_calls": 3}
    })).await;
    assert!(success, "{report} {diagnostic}");
    assert_eq!(report["metrics"]["tool_calls"], 3);
    assert_eq!(report["metrics"]["max_in_flight"], 3);
    assert_eq!(report["calls"][1]["ok"], false);
}

#[tokio::test]
async fn missing_tool_in_batch_cannot_hide_behind_other_successes() {
    let (success, report, diagnostic) = run("missing-batch-tool", r#"async () => {
        const batch = await codemode.batch([() => callTool("fixture::known", {}), () => callTool("fixture::missing", {})]);
        return {ok: true, successes: batch.ok.length, failures: batch.failed.length};
    }"#, json!({"calls": [{"tool": "fixture::known", "result": {"value": 1}}],
        "expect": {"/successes": 1, "/failures": 1}})).await;
    assert!(!success, "{report} {diagnostic}");
    assert_eq!(report["result"]["successes"], 1);
    assert_eq!(report["result"]["failures"], 1);
    assert!(
        report["failures"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|value| value
                .as_str()
                .is_some_and(|message| message.contains("unexpected"))),
        "{report}"
    );
}

#[tokio::test]
async fn unresolved_promise_hits_real_runner_deadline() {
    let (success, report, diagnostic) = run(
        "pending-timeout",
        "async () => { await new Promise(() => {}); return { ok: true }; }",
        json!({"budgets": {"wall_clock_ms": 300, "tool_calls": 0}}),
    )
    .await;
    assert!(!success, "{report} {diagnostic}");
    assert!(
        diagnostic.contains("timeout"),
        "stable timeout contract: {diagnostic}"
    );
}

#[tokio::test]
async fn synthetic_artifact_utf8_boundary_and_containment_are_enforced() {
    let source = r#"async (input) => {
        const artifact = await writeArtifact("report/utf8.txt", "é".repeat(input.count), {contentType: "text/plain"});
        return {ok: true, artifact};
    }"#;
    for (count, passed) in [(262_144, true), (262_145, false)] {
        let (success, report, diagnostic) = run("artifact-byte-boundary", source, json!({
            "params": {"count": count}, "artifacts": [{"path": "report/utf8.txt", "content_type": "text/plain", "contains": ["é"]}],
            "expect": {"/ok": true, "/artifact/mode": "mock"}, "budgets": {"tool_calls": 1}
        })).await;
        assert_eq!(success, passed, "count={count} {report} {diagnostic}");
        if !passed {
            assert!(
                report["failures"]
                    .as_array()
                    .expect("artifact diagnostics")
                    .iter()
                    .any(|value| value.as_str().is_some_and(
                        |message| message.contains("over-budget fixture artifact write")
                    )),
                "oversized UTF-8 content must fail the artifact byte check: {report}"
            );
        }
    }
    let (success, report, diagnostic) = run(
        "artifact-path-escape",
        r#"async () => {
        try { await writeArtifact("../escape.txt", "synthetic", {}); } catch (_) {}
        return { ok: true };
    }"#,
        json!({"artifacts": [{"path": "report/allowed.txt"}], "expect": {"/ok": true}}),
    )
    .await;
    assert!(!success, "{report} {diagnostic}");
    assert_eq!(report["calls"][0]["ok"], false);
    assert!(
        report["failures"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|value| value
                .as_str()
                .is_some_and(|message| message.contains("unexpected"))),
        "{report}"
    );
}
