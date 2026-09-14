//! Actual-binary coverage for Labby's bounded model replay host.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};
use tempfile::TempDir;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_labby-verify"))
}

fn formal(catalog: &str) -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("invariants.toml"), catalog).unwrap();
    fs::create_dir_all(directory.path().join("scenarios/browser_request")).unwrap();
    directory
}

fn scenario(invariant: &str, model: &str, steps: Value, expect: &str, status: &str) -> Value {
    json!({
        "schema": 1,
        "project": "labby",
        "model": model,
        "invariant": invariant,
        "origin": {"kind": "manual"},
        "initial": {},
        "steps": steps,
        "expect": expect,
        "status": status
    })
}

fn write_scenario(root: &Path, name: &str, value: &Value) {
    fs::write(
        root.join("scenarios/browser_request").join(name),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}

fn complete_corpus(root: &Path) {
    let witnesses = [
        (
            "LABBY-REQ-001",
            json!([
                {"action":"connect","generation":"g1"},
                {"action":"admit","request":"r1"},
                {"action":"dispatch","request":"r1"},
                {"action":"complete_success","request":"r1","generation":"g1"}
            ]),
        ),
        (
            "LABBY-REQ-002",
            json!([
                {"action":"connect","generation":"g1"},
                {"action":"admit","request":"r1"},
                {"action":"dispatch","request":"r1"},
                {"action":"timeout","request":"r1"}
            ]),
        ),
        (
            "LABBY-REQ-003",
            json!([
                {"action":"connect","generation":"g1"},
                {"action":"admit","request":"r1"},
                {"action":"dispatch","request":"r1"},
                {"action":"cancel","request":"r1"},
                {"action":"complete_success","request":"r1","generation":"g1"}
            ]),
        ),
        (
            "LABBY-REQ-004",
            json!([
                {"action":"connect","generation":"current"},
                {"action":"admit","request":"r1"},
                {"action":"dispatch","request":"r1"},
                {"action":"complete_success","request":"r1","generation":"old"},
                {"action":"complete_success","request":"r1","generation":"current"}
            ]),
        ),
        (
            "LABBY-REQ-005",
            json!([
                {"action":"connect","generation":"g1"},
                {"action":"admit","request":"r1"},
                {"action":"cancel","request":"r1"},
                {"action":"dispatch","request":"r1"}
            ]),
        ),
    ];
    for (index, (invariant, steps)) in witnesses.into_iter().enumerate() {
        write_scenario(
            root,
            &format!("golden-{index}.json"),
            &scenario(
                invariant,
                "browser_request",
                steps,
                "invariant_holds",
                "active",
            ),
        );
    }
}

#[test]
fn empty_traces_replay_but_do_not_count_as_t0_semantic_coverage() {
    let directory = formal(labby_model::CATALOG_TOML);
    for (index, invariant) in [
        "LABBY-REQ-001",
        "LABBY-REQ-002",
        "LABBY-REQ-003",
        "LABBY-REQ-004",
        "LABBY-REQ-005",
    ]
    .into_iter()
    .enumerate()
    {
        write_scenario(
            directory.path(),
            &format!("empty-{index}.json"),
            &scenario(
                invariant,
                "browser_request",
                json!([]),
                "invariant_holds",
                "active",
            ),
        );
    }
    let result = command()
        .args(["t0"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["reports"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["replay"]["verdict"] == "invariant_holds")
    );
    assert!(
        report["golden_coverage"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["active_holds_traces"] == 0)
    );
}

#[test]
fn rejected_only_actions_do_not_count_as_semantic_coverage() {
    let directory = formal(labby_model::CATALOG_TOML);
    for (index, invariant) in [
        "LABBY-REQ-001",
        "LABBY-REQ-002",
        "LABBY-REQ-003",
        "LABBY-REQ-004",
        "LABBY-REQ-005",
    ]
    .into_iter()
    .enumerate()
    {
        write_scenario(
            directory.path(),
            &format!("rejected-{index}.json"),
            &scenario(
                invariant,
                "browser_request",
                json!([{"action":"complete_success","request":"missing","generation":"stale"}]),
                "invariant_holds",
                "active",
            ),
        );
    }
    let result = command()
        .args(["t0"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["golden_coverage"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["active_holds_traces"] == 0)
    );
}

#[test]
fn req004_requires_wrong_and_owned_completions_for_the_same_live_request() {
    for weak_witness in [
        json!([
            {"action":"connect","generation":"old"},
            {"action":"replace_connection","generation":"current"},
            {"action":"admit","request":"r1"},
            {"action":"dispatch","request":"r1"},
            {"action":"disconnect","generation":"old"},
            {"action":"complete_success","request":"r1","generation":"current"}
        ]),
        json!([
            {"action":"connect","generation":"current"},
            {"action":"admit","request":"stale-target"},
            {"action":"dispatch","request":"stale-target"},
            {"action":"complete_success","request":"stale-target","generation":"old"},
            {"action":"admit","request":"owned-target"},
            {"action":"dispatch","request":"owned-target"},
            {"action":"complete_success","request":"owned-target","generation":"current"}
        ]),
    ] {
        let directory = formal(labby_model::CATALOG_TOML);
        complete_corpus(directory.path());
        write_scenario(
            directory.path(),
            "golden-3.json",
            &scenario(
                "LABBY-REQ-004",
                "browser_request",
                weak_witness,
                "invariant_holds",
                "active",
            ),
        );
        let result = command()
            .args(["t0"])
            .arg(directory.path())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert!(
            report["golden_coverage"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |item| item["invariant"] == "LABBY-REQ-004" && item["active_holds_traces"] == 0
                )
        );
    }
}

#[test]
fn normalized_identity_collapses_consistent_renames_but_preserves_sources() {
    let directory = formal(labby_model::CATALOG_TOML);
    complete_corpus(directory.path());
    write_scenario(
        directory.path(),
        "renamed-001.json",
        &scenario(
            "LABBY-REQ-001",
            "browser_request",
            json!([
                {"action":"connect","generation":"browser-z"},
                {"action":"admit","request":"call-z"},
                {"action":"dispatch","request":"call-z"},
                {"action":"complete_success","request":"call-z","generation":"browser-z"}
            ]),
            "invariant_holds",
            "active",
        ),
    );
    let result = command()
        .args(["t0"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(result.status.success());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    let relevant: Vec<_> = report["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["path"] == "scenarios/browser_request/golden-0.json"
                || item["path"] == "scenarios/browser_request/renamed-001.json"
        })
        .collect();
    assert_eq!(relevant.len(), 2);
    assert_ne!(
        relevant[0]["source_fingerprint"],
        relevant[1]["source_fingerprint"]
    );
    assert_eq!(
        relevant[0]["canonical_fingerprint"],
        relevant[1]["canonical_fingerprint"]
    );
    assert_eq!(
        relevant[0]["replay"]["fingerprint"],
        relevant[0]["canonical_fingerprint"]
    );
}

#[test]
fn actual_binary_t0_reports_complete_model_only_coverage() {
    let directory = formal(labby_model::CATALOG_TOML);
    complete_corpus(directory.path());
    let result = command()
        .args(["t0"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["schema"], 1);
    assert_eq!(report["lane"], "model_replay");
    assert_eq!(report["catalog"]["project"], "labby");
    assert_eq!(report["catalog"]["model"], "browser_request");
    assert_eq!(report["universal_proof"], false);
    assert_eq!(report["reports"].as_array().unwrap().len(), 5);
    assert!(
        report["golden_coverage"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["active_holds_traces"] == 1)
    );
}

#[test]
fn checked_in_formal_corpus_matches_the_embedded_catalog_and_passes_t0() {
    let formal = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../formal");
    let result = command().arg("t0").arg(formal).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["reports"].as_array().unwrap().len(), 9);
}

#[test]
fn t0_rejects_missing_and_empty_corpora() {
    let missing = tempfile::tempdir().unwrap();
    let result = command().args(["t0"]).arg(missing.path()).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("invariants.toml"));

    let empty = formal(labby_model::CATALOG_TOML);
    let result = command().args(["t0"]).arg(empty.path()).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("corpus is empty"));
}

#[test]
fn t0_rejects_malformed_catalog_and_scenario() {
    let malformed_catalog = formal("schema = nope");
    write_scenario(malformed_catalog.path(), "one.json", &json!({}));
    assert_eq!(
        command()
            .args(["t0"])
            .arg(malformed_catalog.path())
            .status()
            .unwrap()
            .code(),
        Some(2)
    );

    let malformed_scenario = formal(labby_model::CATALOG_TOML);
    fs::write(
        malformed_scenario
            .path()
            .join("scenarios/browser_request/bad.json"),
        b"{not-json",
    )
    .unwrap();
    assert_eq!(
        command()
            .args(["t0"])
            .arg(malformed_scenario.path())
            .status()
            .unwrap()
            .code(),
        Some(2)
    );
}

#[test]
fn active_expectation_mismatch_gates() {
    let directory = formal(labby_model::CATALOG_TOML);
    complete_corpus(directory.path());
    write_scenario(
        directory.path(),
        "mismatch.json",
        &scenario(
            "LABBY-REQ-001",
            "browser_request",
            json!([]),
            "invariant_violated",
            "active",
        ),
    );
    let result = command()
        .args(["t0"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["reports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["replay"]["gate_failure"] == true)
    );
}

#[test]
fn unknown_target_and_invariant_fail_closed() {
    for value in [
        scenario(
            "LABBY-REQ-001",
            "unknown_model",
            json!([]),
            "invariant_holds",
            "active",
        ),
        scenario(
            "LABBY-REQ-999",
            "browser_request",
            json!([]),
            "invariant_holds",
            "active",
        ),
    ] {
        let directory = formal(labby_model::CATALOG_TOML);
        complete_corpus(directory.path());
        write_scenario(directory.path(), "unknown.json", &value);
        let result = command()
            .args(["t0"])
            .arg(directory.path())
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("unknown project, model, or invariant")
        );
    }
}

#[test]
fn missing_active_golden_coverage_gates_even_with_nongating_evidence() {
    let directory = formal(labby_model::CATALOG_TOML);
    for (index, invariant) in [
        "LABBY-REQ-001",
        "LABBY-REQ-002",
        "LABBY-REQ-003",
        "LABBY-REQ-004",
    ]
    .into_iter()
    .enumerate()
    {
        write_scenario(
            directory.path(),
            &format!("golden-{index}.json"),
            &scenario(
                invariant,
                "browser_request",
                json!([]),
                "invariant_holds",
                "active",
            ),
        );
    }
    write_scenario(
        directory.path(),
        "nongating.json",
        &scenario(
            "LABBY-REQ-005",
            "browser_request",
            json!([]),
            "invariant_holds",
            "quarantined",
        ),
    );
    let result = command()
        .args(["t0"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["golden_coverage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["invariant"] == "LABBY-REQ-005" && item["active_holds_traces"] == 0)
    );
}

#[test]
fn nonactive_malformed_target_step_is_reported_without_waiving_or_failing_gates() {
    let directory = formal(labby_model::CATALOG_TOML);
    complete_corpus(directory.path());
    write_scenario(
        directory.path(),
        "quarantined-malformed-step.json",
        &scenario(
            "LABBY-REQ-001",
            "browser_request",
            json!([{"action": "not_a_browser_action"}]),
            "invariant_holds",
            "quarantined",
        ),
    );
    let result = command()
        .args(["t0"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(result.status.success());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        report["reports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["status"] == "quarantined" && item["replay"]["verdict"] == "error")
    );
}

#[test]
fn replay_subcommand_uses_embedded_labby_registry() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scenario.json");
    fs::write(
        &path,
        serde_json::to_vec(&scenario(
            "LABBY-REQ-001",
            "browser_request",
            json!([]),
            "invariant_holds",
            "active",
        ))
        .unwrap(),
    )
    .unwrap();
    let result = command().arg("replay").arg(path).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["verdict"], "invariant_holds");
    assert_eq!(report["gate_failure"], false);
}
