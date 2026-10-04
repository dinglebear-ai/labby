//! Built-in examples must retain bounded dependencies and real offline scenarios.
#![cfg(feature = "gateway")]
use labby_codemode::snippet::{harness::SnippetFixture, store};
use serde_json::Value;
use std::path::PathBuf;

// Product subprocess startup is expensive; keep this suite's runner demand bounded.
static PRODUCT_EXECUTION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn every_builtin_declares_dependencies_and_passes_offline_fixture() {
    let _product = PRODUCT_EXECUTION.lock().await;
    let home = tempfile::tempdir().expect("isolated home");
    let builtin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/snippets");
    let snippets = store::list_snippets(home.path(), &builtin).expect("builtin catalog");
    assert_eq!(
        snippets.len(),
        11,
        "review fixture coverage when adding examples"
    );
    for snippet in &snippets {
        let tools = snippet.tools.as_ref().expect("builtin must declare tools");
        assert!(
            !tools.as_slice().is_empty(),
            "{} has no dependencies",
            snippet.name
        );
        let fixture_path = snippet.path.with_extension("test.json");
        let fixture: SnippetFixture = serde_json::from_slice(
            &std::fs::read(&fixture_path).expect("adjacent builtin fixture"),
        )
        .expect("valid fixture JSON");
        fixture.validate().expect("bounded fixture");
        assert!(
            !fixture.calls.is_empty() || !fixture.snippets.is_empty(),
            "{} must exercise workflow calls",
            snippet.name
        );
        assert!(
            !fixture.expect.is_empty(),
            "{} needs output assertions",
            snippet.name
        );
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_mins(3),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
            .args(["--json", "snippet", "test", "--all"])
            .env_clear()
            .env("HOME", home.path())
            .env("LABBY_HOME", home.path())
            .env("LABBY_CODE_MODE_RUNNER_BACKEND", "process")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("bounded fixture suite")
    .expect("offline CLI execution");
    assert!(
        output.status.success(),
        "fixture suite failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON fixture report");
    assert_eq!(report["passed"], true, "{report}");
    assert_eq!(
        report["results"].as_array().expect("fixture results").len(),
        snippets.len()
    );
}

#[tokio::test]
async fn fixture_input_precedence_and_synthetic_writes_are_enforced() {
    let _product = PRODUCT_EXECUTION.lock().await;
    let home = tempfile::tempdir().expect("isolated home");
    store::create_user_snippet(home.path(), "synthetic-workflow", r#"async (input) => {
        const child = await codemode.run("fixture-child", {alias: input.alias});
        const artifact = await writeArtifact("report/child.json", JSON.stringify(child), {contentType: "application/json"});
        return {alias: input.alias, child, artifact};
    }"#, Some("Fixture boundary regression"), false).expect("create synthetic workflow");
    let fixture_path = home.path().join("synthetic.test.json");
    std::fs::write(&fixture_path, serde_json::to_vec(&serde_json::json!({
        "params": {"alias": "fixture-default"},
        "snippets": [{"name": "fixture-child", "match": {"alias": "caller-override"}, "result": {"hostname": "fixture-host"}}],
        "artifacts": [{"path": "report/child.json", "content_type": "application/json", "contains": ["fixture-host"]}],
        "expect": {"/alias": "caller-override", "/child/hostname": "fixture-host", "/artifact/mode": "mock"},
        "budgets": {"tool_calls": 2}
    })).expect("fixture JSON")).expect("fixture file");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
            .args([
                "--json",
                "snippet",
                "test",
                "synthetic-workflow",
                "--param",
                "alias=caller-override",
                "--fixture",
            ])
            .arg(&fixture_path)
            .env_clear()
            .env("HOME", home.path())
            .env("LABBY_HOME", home.path())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("bounded synthetic test")
    .expect("offline CLI");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["passed"], true, "{report}");
    assert!(
        !home.path().join("report/child.json").exists(),
        "synthetic artifact must not touch disk"
    );
}

#[tokio::test]
async fn swallowed_synthetic_operation_errors_still_fail_fixture() {
    let _product = PRODUCT_EXECUTION.lock().await;
    let home = tempfile::tempdir().expect("isolated home");
    store::create_user_snippet(home.path(), "swallowed-fixture-errors", r#"async () => {
        try { await codemode.run("unexpected-child", {}); } catch (_) {}
        try { await writeArtifact("report/result.json", "wrong content", {contentType: "application/json"}); } catch (_) {}
        return {ok: true};
    }"#, Some("Synthetic errors must remain visible"), false).expect("create workflow");
    let fixture_path = home.path().join("swallowed.test.json");
    std::fs::write(
        &fixture_path,
        serde_json::to_vec(&serde_json::json!({
            "artifacts": [{"path": "report/result.json", "contains": ["required evidence"]}],
            "expect": {"/ok": true}
        }))
        .expect("fixture JSON"),
    )
    .expect("fixture file");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
            .args([
                "--json",
                "snippet",
                "test",
                "swallowed-fixture-errors",
                "--fixture",
            ])
            .arg(&fixture_path)
            .env_clear()
            .env("HOME", home.path())
            .env("LABBY_HOME", home.path())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("bounded negative test")
    .expect("offline CLI");
    assert!(!output.status.success(), "swallowed errors must fail");
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["passed"], false, "{report}");
    let failures = report["failures"].as_array().expect("failure details");
    assert!(
        failures.iter().any(|failure| failure
            .as_str()
            .is_some_and(|text| text.contains("unexpected"))),
        "{report}"
    );
    assert!(
        failures.iter().any(|failure| failure
            .as_str()
            .is_some_and(|text| text.contains("not fully consumed"))),
        "{report}"
    );
}

#[path = "builtin_snippet_fixtures/failure_paths.rs"]
mod failure_paths;
