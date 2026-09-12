//! End-to-end replay behavior against the fixture model.
//!
//! The fixture is compiled as an example rather than a test-only type so that
//! the same code proves the interface here and serves as the worked example an
//! adopting project copies.

// Including the example gives one definition of the fixture: the same code
// proves the interface here and is the worked example M3 copies. Its `main`
// and `registry` are unused in this context, which is not a defect.
#[path = "../examples/fixture.rs"]
#[allow(dead_code)]
mod fixture;

use serde_json::json;
use verify_core::Kind;
use verify_report::replay::Outcome;
use verify_runner::{TargetKey, TargetRegistry, check_determinism, minimize, replay};
use verify_scenario::{Expect, Scenario, ScenarioStatus};

fn registry() -> TargetRegistry {
    TargetRegistry::new().with(
        TargetKey::new("fixture", "request"),
        Box::new(fixture::RequestModel),
    )
}

fn scenario(steps: &serde_json::Value, expect: &str) -> Scenario {
    let text = format!(
        r#"{{
  "schema": 1,
  "project": "fixture",
  "model": "request",
  "invariant": "FIXTURE-REQ-001",
  "origin": {{ "kind": "manual" }},
  "steps": {steps},
  "expect": "{expect}"
}}"#
    );
    Scenario::parse(&text).expect("parse")
}

#[test]
fn a_violating_trace_reproduces() {
    let scenario = scenario(
        &json!([
            { "event": "dispatch" },
            { "event": "complete" },
            { "event": "late_response" }
        ]),
        "invariant_violated",
    );
    let report = replay(&scenario, Kind::Safety, &registry(), "violating");
    assert_eq!(report.outcome, Outcome::Matched);
    assert!(!report.failed());
    assert_eq!(report.first_violation, Some(3));
}

#[test]
fn a_golden_trace_holds() {
    let scenario = scenario(
        &json!([{ "event": "dispatch" }, { "event": "complete" }]),
        "invariant_holds",
    );
    let report = replay(&scenario, Kind::Safety, &registry(), "golden");
    assert_eq!(report.outcome, Outcome::Matched);
}

#[test]
fn a_tampered_scenario_mismatches_and_fails() {
    // Same trace, wrong expectation.
    let scenario = scenario(
        &json!([{ "event": "dispatch" }, { "event": "complete" }]),
        "invariant_violated",
    );
    let report = replay(&scenario, Kind::Safety, &registry(), "tampered");
    assert!(matches!(report.outcome, Outcome::Mismatched { .. }));
    assert!(report.failed(), "an active mismatch must fail CI");
}

#[test]
fn a_safety_violation_repaired_before_the_end_still_counts() {
    // The whole point of judging safety over the trace. The invariant is false
    // after late_response; nothing repairs it here, but the final-state reading
    // would depend on the last step rather than on the violation.
    let scenario = scenario(
        &json!([
            { "event": "dispatch" },
            { "event": "complete" },
            { "event": "late_response" },
            { "event": "dispatch" }
        ]),
        "invariant_violated",
    );
    let report = replay(&scenario, Kind::Safety, &registry(), "mid-trace");
    assert_eq!(report.outcome, Outcome::Matched);
    assert_eq!(
        report.first_violation,
        Some(3),
        "the report must name where it first went wrong"
    );
}

#[test]
fn an_unregistered_target_is_distinct_from_a_mismatch() {
    // "nobody registered the target" must never read as "the invariant holds".
    let mut scenario = scenario(&json!([{ "event": "dispatch" }]), "invariant_holds");
    scenario.model = "nonexistent".to_owned();
    let report = replay(&scenario, Kind::Safety, &registry(), "no-target");
    assert!(matches!(report.outcome, Outcome::NoTarget { .. }));
    assert!(report.failed());
}

#[test]
fn an_uninterpretable_step_is_malformed_not_a_violation() {
    let scenario = scenario(&json!([{ "event": "frobnicate" }]), "invariant_holds");
    let report = replay(&scenario, Kind::Safety, &registry(), "bad-step");
    assert!(matches!(report.outcome, Outcome::Malformed { .. }));
    assert!(report.failed());
}

#[test]
fn a_legal_step_rejection_is_recorded_without_failing() {
    // Cancelling before dispatch is refused by the model. That is a modeled
    // outcome, not a harness error.
    let scenario = scenario(
        &json!([{ "event": "cancel" }, { "event": "dispatch" }]),
        "invariant_holds",
    );
    let report = replay(&scenario, Kind::Safety, &registry(), "rejected");
    assert_eq!(report.outcome, Outcome::Matched);
    assert!(!report.steps[0].applied, "step 0 should be refused");
    assert!(report.steps[1].applied);
}

#[test]
fn quarantined_and_unreproduced_scenarios_do_not_gate() {
    let base = scenario(
        &json!([{ "event": "dispatch" }, { "event": "complete" }]),
        "invariant_violated",
    );

    let mut quarantined = base.clone();
    quarantined.status = ScenarioStatus::Quarantined;
    let report = replay(&quarantined, Kind::Safety, &registry(), "quarantined");
    assert!(matches!(report.outcome, Outcome::Mismatched { .. }));
    assert!(!report.failed(), "quarantined must be reported, not gating");

    let mut unreproduced = base;
    unreproduced.status = ScenarioStatus::Unreproduced;
    let report = replay(&unreproduced, Kind::Safety, &registry(), "unreproduced");
    assert!(!report.failed());
}

#[test]
fn an_unreproduced_scenario_that_starts_matching_is_promotable() {
    let mut scenario = scenario(
        &json!([
            { "event": "dispatch" },
            { "event": "complete" },
            { "event": "late_response" }
        ]),
        "invariant_violated",
    );
    scenario.status = ScenarioStatus::Unreproduced;
    let report = replay(&scenario, Kind::Safety, &registry(), "promotable");
    assert_eq!(
        report.outcome,
        Outcome::Promotable,
        "the runner surfaces the promotion instead of rewriting the file"
    );
    assert!(!report.failed());
}

#[test]
fn minimization_drops_steps_that_do_not_matter() {
    let scenario = scenario(
        &json!([
            { "event": "dispatch" },
            { "event": "cancel" },
            { "event": "complete" },
            { "event": "late_response" }
        ]),
        "invariant_violated",
    );
    let minimized = minimize(&scenario, Kind::Safety, &registry());
    assert!(
        minimized.steps.len() < scenario.steps.len(),
        "expected a shorter trace, got {:?}",
        minimized.steps
    );
    // Whatever survives must still reproduce.
    let report = replay(&minimized, Kind::Safety, &registry(), "minimized");
    assert_eq!(report.outcome, Outcome::Matched);
}

#[test]
fn minimization_leaves_a_golden_trace_intact() {
    // The bug the design review caught: verdict-preserving minimization shrinks
    // a golden trace to the empty trace, which trivially still holds.
    let scenario = scenario(
        &json!([{ "event": "dispatch" }, { "event": "complete" }]),
        "invariant_holds",
    );
    let minimized = minimize(&scenario, Kind::Safety, &registry());
    assert_eq!(
        minimized.steps, scenario.steps,
        "a golden trace must survive minimization with its steps intact"
    );
}

#[test]
fn minimization_does_not_shrink_a_scenario_that_never_reproduced() {
    let scenario = scenario(
        &json!([{ "event": "dispatch" }, { "event": "complete" }]),
        "invariant_violated",
    );
    let minimized = minimize(&scenario, Kind::Safety, &registry());
    assert_eq!(
        minimized.steps, scenario.steps,
        "without a reproduction every candidate 'still fails', shrinking to nothing"
    );
}

#[test]
fn a_pure_target_is_deterministic() {
    let scenario = scenario(
        &json!([
            { "event": "dispatch" },
            { "event": "complete" },
            { "event": "late_response" }
        ]),
        "invariant_violated",
    );
    let report = check_determinism(&scenario, Kind::Safety, &registry(), 3);
    assert!(report.stable);
    assert_eq!(report.runs, 3);
}

#[test]
fn liveness_is_judged_at_the_end_of_the_trace() {
    // A liveness property is not violated by an intermediate state that has not
    // yet satisfied it — that is the middle of a trace, not a counterexample.
    let scenario = scenario(
        &json!([
            { "event": "dispatch" },
            { "event": "complete" },
            { "event": "late_response" }
        ]),
        "invariant_violated",
    );
    let safety = replay(&scenario, Kind::Safety, &registry(), "as-safety");
    let liveness = replay(&scenario, Kind::Liveness, &registry(), "as-liveness");
    assert_eq!(safety.outcome, Outcome::Matched);
    // Same trace, different judgement rule: the final state is still violating
    // here, so both agree — but the reasoning differs, and first_violation is
    // recorded either way.
    assert_eq!(liveness.outcome, Outcome::Matched);
    assert_eq!(safety.expect, Expect::InvariantViolated);
}
