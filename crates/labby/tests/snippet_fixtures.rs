//! End-to-end fixture tests use the real binary and production sandbox protocol.
#![cfg(feature = "gateway")]
use serde_json::{Value, json};
use std::process::{Command, Output};

fn run(body: &str, fixture: &Value) -> Output {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("snippets")).unwrap();
    std::fs::write(home.path().join("snippets/fixture-case.js"), body).unwrap();
    let path = home.path().join("fixture.json");
    std::fs::write(&path, serde_json::to_vec(fixture).unwrap()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path());
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command
        .args(["snippet", "test", "fixture-case", "--json", "--fixture"])
        .arg(path)
        .output()
        .unwrap()
}
fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
#[test]
fn offline_fixture_runs_in_real_sandbox() {
    let output = run(
        r#"async () => await callTool("demo::read", {id:1})"#,
        &json!({"calls":[{"tool":"demo::read","params":{"id":1},"response":{"ok":true,"n":7}}],"expect":{"/n":7}}),
    );
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["passed"], true);
    assert_eq!(value["mode"], "mock");
    assert_eq!(value["metrics"]["tool_calls"], 1);
}
#[test]
fn failed_assertions_set_a_failing_process_status() {
    let output = run("async () => ({n:1})", &json!({"expect":{"/n":2}}));
    assert!(!output.status.success());
    assert_eq!(report(&output)["passed"], false);
}
#[test]
fn swallowed_wrong_arguments_cannot_turn_into_a_pass() {
    let output = run(
        r#"async () => { try { await callTool("demo::read", {id:2}); } catch (_) {} return {ok:true}; }"#,
        &json!({"calls":[{"tool":"demo::read","params":{"id":1},"response":{}}]}),
    );
    assert!(!output.status.success());
    assert_eq!(report(&output)["passed"], false);
}
#[test]
fn batch_settles_both_mock_success_and_simulated_failure() {
    let output = run(
        r#"async () => { const r = await codemode.batch([
        () => callTool("demo::read", {id:1}), () => callTool("demo::read", {id:2})]);
        return {successes:r.ok.length, failures:r.failed.length}; }"#,
        &json!({"calls":[{"tool":"demo::read","params":{"id":1},"response":{}},
            {"tool":"demo::read","params":{"id":2},"error":{"kind":"timeout","message":"synthetic timeout"}}],
            "expect":{"/successes":1,"/failures":1}}),
    );
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["metrics"]["failed_calls"], 1);
}
#[test]
fn output_budget_uses_raw_bytes_and_omits_oversized_result() {
    let output = run(
        "async () => ({text:'x'.repeat(1000)})",
        &json!({"budgets":{"output_bytes":100}}),
    );
    assert!(!output.status.success());
    let value = report(&output);
    assert!(value.get("result").is_none());
    assert!(value["metrics"]["output_bytes"].as_u64().unwrap() > 100);
}
#[test]
fn unscoped_local_provider_cannot_escape_fixture_authority() {
    let output = run(
        r#"async () => { try { await callTool("state::writeFile", {}); } catch (_) {} return {ok:true}; }"#,
        &json!({}),
    );
    assert!(!output.status.success());
    assert_eq!(report(&output)["passed"], false);
}
#[test]
fn live_execution_requires_explicit_opt_in() {
    let output = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["snippet", "test", "fixture-case", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture"));
}

#[test]
fn checked_in_triage_fixtures_satisfy_budgets() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("snippets")).unwrap();
    std::fs::write(
        home.path().join("snippets/unraid-linear-pr-triage.md"),
        include_str!("../../../docs/snippets/unraid-linear-pr-triage.md"),
    )
    .unwrap();
    for (name, input_override) in [
        ("triage-fast.json", None),
        ("triage-deep.json", None),
        ("triage-partial-failure.json", None),
        ("triage-pagination.json", None),
        (
            "triage-deep.json",
            Some(json!({"includeHistory":true,"includeHandoffs":true,"chunkSize":4})),
        ),
        (
            "triage-deep.json",
            Some(json!({"include_history":true,"include_handoffs":true,"chunk_size":4})),
        ),
    ] {
        let mut fixture: Value = serde_json::from_slice(
            &std::fs::read(workspace.join("tests/snippets").join(name)).unwrap(),
        )
        .unwrap();
        if let Some(input) = input_override {
            fixture["input"] = input;
        }
        let fixture_path = home.path().join("fixture.json");
        std::fs::write(&fixture_path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
        command
            .env_clear()
            .env("HOME", home.path())
            .env("LABBY_HOME", home.path());
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        let output = command
            .args([
                "snippet",
                "test",
                "unraid-linear-pr-triage",
                "--json",
                "--fixture",
            ])
            .arg(fixture_path)
            .output()
            .unwrap();
        let value = report(&output);
        assert!(output.status.success(), "{name}: {value}");
        assert_eq!(value["passed"], true, "{name}: {value}");
    }
}
