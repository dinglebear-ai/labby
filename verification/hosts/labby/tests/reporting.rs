//! Retained real-binary evidence uses shared renderers without changing gates.

use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_labby-verify"))
}

#[test]
fn usage_names_every_command_and_the_actual_report_subcommand() {
    let output = command().output().unwrap();
    let usage = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    for name in ["t0", "t1", "replay", "report-t0", "report-t1"] {
        assert!(usage.contains(name), "{usage}");
    }
    for name in ["report-t0", "report-t1"] {
        let output = command().arg(name).output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .starts_with(&format!("usage: labby-verify {name} "))
        );
    }
}

#[test]
fn actual_binary_reports_all_formats_and_retains_source_failure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../formal");
    let replay = command().arg("t0").arg(root).output().unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    fs::write(path.join("replay.json"), &replay.stdout).unwrap();
    fs::write(
        path.join("revision"),
        "1111111111111111111111111111111111111111",
    )
    .unwrap();
    fs::write(path.join("dirty"), " M uncommitted-source").unwrap();
    fs::write(
        path.join("binary"),
        "2222222222222222222222222222222222222222222222222222222222222222  host",
    )
    .unwrap();
    let render = |format: &str| {
        command()
            .arg("report-t0")
            .arg(path.join("replay.json"))
            .arg(path.join("revision"))
            .arg(path.join("dirty"))
            .arg(path.join("binary"))
            .arg(format)
            .arg("fixture-source")
            .output()
            .unwrap()
    };
    for format in ["json", "text", "markdown", "html"] {
        let result = render(format);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.stdout.is_empty());
        if format == "json" {
            let result: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(result["scenarios"]["total"], 13);
            assert_eq!(result["scenarios"]["by_status"]["active"], 13);
            assert_eq!(result["observations"][0]["sources"][0]["dirty"], true);
            assert_eq!(result["observations"][0]["verdict"], "passed");
        }
    }
    let mut failed: Value = serde_json::from_slice(&replay.stdout).unwrap();
    failed["gate_failure"] = json!(true);
    fs::write(path.join("replay.json"), failed.to_string()).unwrap();
    let result = render("json");
    assert!(result.status.success()); // Rendering is not re-execution.
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["observations"][0]["verdict"], "error");
    let mut missing_backend: Value = serde_json::from_slice(&replay.stdout).unwrap();
    missing_backend["uncovered_backend_ids"] = json!(["LABBY-REQ-001"]);
    fs::write(path.join("replay.json"), missing_backend.to_string()).unwrap();
    let result = render("json");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        result["coverage"]["unconfigured_backend_ids"],
        json!(["LABBY-REQ-001"])
    );
    assert_eq!(
        result["coverage"]["backend_uncovered_ids"]
            .as_array()
            .unwrap()
            .len(),
        9
    );
    let mut empty: Value = serde_json::from_slice(&replay.stdout).unwrap();
    empty["reports"] = json!([]);
    empty["golden_coverage"] = json!([]);
    fs::write(path.join("replay.json"), empty.to_string()).unwrap();
    assert!(!render("json").status.success());
    let mut inconsistent: Value = serde_json::from_slice(&replay.stdout).unwrap();
    inconsistent["reports"][0]["replay"]["gate_failure"] = json!(true);
    fs::write(path.join("replay.json"), inconsistent.to_string()).unwrap();
    assert!(!render("json").status.success());
    fs::write(path.join("revision"), "not-a-revision").unwrap();
    assert!(!render("json").status.success());
}

#[test]
fn actual_binary_t1_is_bounded_and_rejects_wrong_catalog() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../formal");
    let result = command().arg("t1").arg(root).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["lane"], "model_checking");
    assert_eq!(result["universal_proof"], false);
    let reports = result["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 9);
    assert!(
        reports
            .iter()
            .all(|report| report["verdict"]["verdict"] == "bounded")
    );
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("invariants.toml"), "schema=9").unwrap();
    assert!(
        !command()
            .arg("t1")
            .arg(directory.path())
            .output()
            .unwrap()
            .status
            .success()
    );
}
