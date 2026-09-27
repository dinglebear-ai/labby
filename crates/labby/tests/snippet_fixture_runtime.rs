//! End-to-end fixture tests using the compiled product and its real QuickJS runner.
//! Every invocation gets an empty home and environment; no gateway is configured.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn invoke(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .args(["--json", "snippet"])
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("LABBY_HOME", home.join(".labby"))
        .env("NO_COLOR", "1")
        .env("TMPDIR", home)
        .stdin(Stdio::null())
        .current_dir(home);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command.output().expect("run compiled labby")
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .map_err(|error| {
            format!(
                "invalid JSON report: {error}; stdout={}; stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .expect("valid JSON report")
}

fn install(home: &Path, name: &str, code: &str, fixture: Option<Value>) {
    let dir = home.join(".labby/snippets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.js")), code).unwrap();
    if let Some(fixture) = fixture {
        std::fs::write(
            dir.join(format!("{name}.test.json")),
            serde_json::to_vec(&fixture).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn fast_triage_uses_real_runner_with_ten_synthetic_calls_and_no_gateway() {
    let home = tempfile::tempdir().unwrap();
    let output = invoke(home.path(), &["test", "unraid-linear-pr-triage"]);
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["passed"], true);
    assert_eq!(value["mode"], "mock");
    assert_eq!(value["metrics"]["tool_calls"], 10);
    assert_eq!(value["result"]["summary"]["issueCount"], 26);
    assert!(value["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
    for issue in value["result"]["issues"].as_array().unwrap() {
        assert!(issue.get("historicalPRs").is_none());
        assert!(issue.get("handoff").is_none());
    }
}

#[test]
fn deep_triage_uses_explicit_fixture_and_compact_handoffs() {
    let home = tempfile::tempdir().unwrap();
    let fixture = root().join("docs/snippets/unraid-linear-pr-triage.deep.test.json");
    let output = invoke(
        home.path(),
        &[
            "test",
            "unraid-linear-pr-triage",
            "--fixture",
            fixture.to_str().unwrap(),
            "--param",
            "deep=true",
        ],
    );
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["passed"], true);
    assert_eq!(value["mode"], "mock");
    assert_eq!(value["result"]["summary"]["handoffCalls"], 26);
    assert!(value["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
}

#[test]
fn missing_fixture_never_falls_back_to_live_execution() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "no-fixture",
        "async () => ({ ok: true })",
        None,
    );
    let output = invoke(home.path(), &["test", "no-fixture"]);
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("fixture"), "{text}");
    assert!(text.contains("live"), "{text}");
}

#[test]
fn swallowed_unexpected_mock_calls_still_fail_the_cli_exit_status() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "unexpected",
        "async () => { try { await callTool('synthetic::missing', {}); } catch (_) {} return { ok: true }; }",
        Some(json!({"calls": []})),
    );
    let output = invoke(home.path(), &["test", "unexpected"]);
    let value = report(&output);
    assert!(!output.status.success());
    assert_eq!(value["passed"], false);
    assert!(
        value["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("unexpected"))
    );
}

#[test]
fn global_bridge_escape_is_denied_and_reported_without_writing_artifacts() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "escape",
        "async () => { try { await globalThis.callTool('synthetic::escape', {}); } catch (_) {} return { ok: true }; }",
        Some(json!({"calls": []})),
    );
    let output = invoke(home.path(), &["test", "escape"]);
    let value = report(&output);
    assert!(!output.status.success());
    assert_eq!(value["passed"], false);
    assert!(!home.path().join(".labby/code-mode-artifacts").exists());
}
