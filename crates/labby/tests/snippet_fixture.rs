//! Real CLI coverage for isolated snippet fixtures and fail-closed defaults.
#![cfg(feature = "gateway")]

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/snippets/tests/unraid-linear-pr-triage")
}

async fn invoke(home: &Path, args: &[&str]) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env("LABBY_HOME", home)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .args(["snippet", "test"])
        .args(args)
        .arg("--json")
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("fixture process exceeded deadline")
        .expect("start CLI")
}

#[tokio::test]
async fn triage_fixtures_use_production_runner_without_live_calls() {
    let home = tempfile::tempdir().unwrap();
    for name in [
        "fast",
        "deep",
        "repo-override",
        "identity-failure",
        "rate-limit",
        "partial-search",
        "large-output",
    ] {
        let path = fixtures().join(format!("{name}.json"));
        let output = invoke(
            home.path(),
            &[
                "unraid-linear-pr-triage",
                "--fixture",
                path.to_str().unwrap(),
            ],
        )
        .await;
        assert!(
            output.status.success(),
            "{name}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["passed"], true, "{name}: {report}");
        assert_eq!(report["mode"], "mock");
        assert_eq!(report["metrics"]["external_calls"], 0);
        assert!(report["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
    }
}

#[tokio::test]
async fn caught_unexpected_calls_fail_the_cli_exit_status() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("snippets")).unwrap();
    std::fs::write(home.path().join("snippets/caught-call.js"),
        "async () => { try { await callTool('github::get_me', {}); } catch (_) {} return {ok:true}; }").unwrap();
    let fixture = home.path().join("fixture.json");
    std::fs::write(
        &fixture,
        serde_json::to_vec(&json!({"expect":{"/ok":true}})).unwrap(),
    )
    .unwrap();
    let output = invoke(
        home.path(),
        &["caught-call", "--fixture", fixture.to_str().unwrap()],
    )
    .await;
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], false);
    assert_eq!(report["metrics"]["external_calls"], 0);
    assert!(
        report["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().contains("unexpected"))
    );
}

#[tokio::test]
async fn no_mode_and_conflicting_modes_fail_before_execution() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["unraid-linear-pr-triage"],
        vec![
            "unraid-linear-pr-triage",
            "--live",
            "--fixture",
            "missing.json",
        ],
    ] {
        let output = invoke(home.path(), &args).await;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("unknown upstream"));
    }
}

#[tokio::test]
async fn fixture_json_keys_remain_data_not_object_prototypes() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("snippets")).unwrap();
    std::fs::write(home.path().join("snippets/prototype-data.js"),
        "async input => ({ owns: Object.prototype.hasOwnProperty.call(input, '__proto__'), injected: input.injected === true })").unwrap();
    let fixture = home.path().join("fixture.json");
    std::fs::write(
        &fixture,
        serde_json::to_vec(&json!({
            "input": {"__proto__": {"injected": true}},
            "expect": {"/owns": true, "/injected": false},
            "budgets": {"tool_calls": 0}
        }))
        .unwrap(),
    )
    .unwrap();
    let output = invoke(
        home.path(),
        &["prototype-data", "--fixture", fixture.to_str().unwrap()],
    )
    .await;
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["metrics"]["external_calls"], 0);
}
