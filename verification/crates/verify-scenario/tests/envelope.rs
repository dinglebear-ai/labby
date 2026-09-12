//! Scenario envelope, fingerprint, and syntactic normalization behavior.

use serde_json::json;
use verify_scenario::{Expect, Scenario, ScenarioLoadError, ScenarioStatus, fingerprint};

fn scenario_json(extra: &str) -> String {
    format!(
        r#"{{
  "schema": 1,
  "project": "labby",
  "model": "gateway",
  "invariant": "LABBY-REQ-001",
  "origin": {{ "kind": "stateright" }},
  "steps": [{{ "event": "dispatch" }}],
  "expect": "invariant_violated"{extra}
}}"#
    )
}

#[test]
fn parses_a_minimal_scenario() {
    let scenario = Scenario::parse(&scenario_json("")).expect("parse");
    assert_eq!(scenario.project, "labby");
    assert_eq!(scenario.expect, Expect::InvariantViolated);
    // Status defaults to active, so an author who omits it gets the gating one.
    assert_eq!(scenario.status, ScenarioStatus::Active);
    assert!(scenario.gates_ci());
}

#[test]
fn absent_initial_normalizes_to_an_empty_object() {
    let scenario = Scenario::parse(&scenario_json("")).expect("parse");
    assert_eq!(scenario.initial_value(), json!({}));
}

#[test]
fn a_literal_null_initial_is_rejected_rather_than_coerced() {
    // serde would read `"initial": null` as None, conflating a deliberate null
    // with an absent key. The spec rejects the former.
    let text = scenario_json(",\n  \"initial\": null");
    assert_eq!(
        Scenario::parse(&text),
        Err(ScenarioLoadError::InitialNotAnObject)
    );
}

#[test]
fn a_scalar_initial_is_rejected() {
    let text = scenario_json(",\n  \"initial\": 7");
    assert_eq!(
        Scenario::parse(&text),
        Err(ScenarioLoadError::InitialNotAnObject)
    );
}

#[test]
fn a_future_schema_is_refused_rather_than_read_optimistically() {
    let text = scenario_json("").replace("\"schema\": 1", "\"schema\": 2");
    assert_eq!(
        Scenario::parse(&text),
        Err(ScenarioLoadError::Schema { found: 2 })
    );
}

#[test]
fn an_unknown_field_is_rejected() {
    let text = scenario_json(",\n  \"expcet\": \"invariant_holds\"");
    assert!(matches!(
        Scenario::parse(&text),
        Err(ScenarioLoadError::Malformed { .. })
    ));
}

#[test]
fn only_active_scenarios_gate_ci() {
    for (status, gates) in [
        (ScenarioStatus::Active, true),
        (ScenarioStatus::Quarantined, false),
        (ScenarioStatus::Unreproduced, false),
    ] {
        let mut scenario = Scenario::parse(&scenario_json("")).expect("parse");
        scenario.status = status;
        assert_eq!(
            scenario.gates_ci(),
            gates,
            "{status:?} should {}gate",
            if gates { "" } else { "not " }
        );
    }
}

#[test]
fn fingerprint_is_prefixed_and_stable_across_key_order() {
    let a = Scenario::parse(&scenario_json(
        ",\n  \"initial\": { \"alpha\": 1, \"beta\": 2 }",
    ))
    .expect("parse");
    let b = Scenario::parse(&scenario_json(
        ",\n  \"initial\": { \"beta\": 2, \"alpha\": 1 }",
    ))
    .expect("parse");
    let fa = fingerprint(&a);
    assert!(fa.starts_with("s256:"), "{fa}");
    assert_eq!(fa, fingerprint(&b), "key order must not change the hash");
}

#[test]
fn fingerprint_ignores_provenance_and_status() {
    // The same trace found by two backends is the same scenario. If provenance
    // changed the hash, the corpus would keep both copies forever.
    let mut a = Scenario::parse(&scenario_json("")).expect("parse");
    let mut b = a.clone();
    b.origin.kind = verify_scenario::OriginKind::Fuzz;
    b.origin.seed = Some(99);
    b.status = ScenarioStatus::Quarantined;
    a.fingerprint = Some("s256:stale".to_owned());
    assert_eq!(fingerprint(&a), fingerprint(&b));
}

#[test]
fn fingerprint_changes_when_the_trace_changes() {
    let a = Scenario::parse(&scenario_json("")).expect("parse");
    let mut b = a.clone();
    b.steps.push(json!({ "event": "cancel" }));
    assert_ne!(fingerprint(&a), fingerprint(&b));

    let mut c = a.clone();
    c.expect = Expect::InvariantHolds;
    assert_ne!(
        fingerprint(&a),
        fingerprint(&c),
        "expect is part of what the scenario asserts"
    );
}
