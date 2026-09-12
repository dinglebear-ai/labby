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
fn a_safety_violation_followed_by_a_rejected_step_still_counts() {
    // Dispatch is rejected because the request is already dispatched. The
    // rejected step must not clear the earlier recorded violation.
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

struct VerdictModel(verify_core::verdict::Verdict);
impl verify_core::target::ScenarioTarget for VerdictModel {
    type State = ();
    type Step = serde_json::Value;
    fn init(&self, _: &serde_json::Value) -> Result<(), verify_core::target::ScenarioError> {
        Ok(())
    }
    fn apply(
        &self,
        (): &mut (),
        _: &Self::Step,
    ) -> Result<verify_core::target::StepOutcome, verify_core::target::ScenarioError> {
        Ok(verify_core::target::StepOutcome::Applied)
    }
    fn check(
        &self,
        id: &verify_core::InvariantId,
        (): &(),
    ) -> Result<verify_core::InvariantResult, verify_core::target::ScenarioError> {
        Ok(verify_core::InvariantResult::new(
            id.clone(),
            self.0.clone(),
        ))
    }
}

#[test]
fn unavailable_verdicts_never_pass_as_holding_invariants() {
    use verify_core::verdict::Verdict;
    for verdict in [
        Verdict::Error {
            reason: "failed".into(),
        },
        Verdict::Skipped {
            reason: "missing".into(),
        },
        Verdict::Uncovered,
    ] {
        let targets = TargetRegistry::new().with(
            TargetKey::new("fixture", "request"),
            Box::new(VerdictModel(verdict)),
        );
        for steps in [json!([]), json!([{}])] {
            let report = replay(
                &scenario(&steps, "invariant_holds"),
                Kind::Safety,
                &targets,
                "unavailable",
            );
            assert!(matches!(report.outcome, Outcome::Malformed { .. }));
            assert!(report.failed());
        }
    }
}

#[test]
fn empty_final_state_traces_judge_the_initial_state() {
    let targets = TargetRegistry::new().with(
        TargetKey::new("fixture", "request"),
        Box::new(VerdictModel(verify_core::verdict::Verdict::Falsified)),
    );
    for kind in [Kind::Liveness, Kind::Refinement] {
        let report = replay(
            &scenario(&json!([]), "invariant_violated"),
            kind,
            &targets,
            "initial-only",
        );
        assert_eq!(report.outcome, Outcome::Matched);
        assert_eq!(report.first_violation, Some(0));
    }
}

struct AlternatingModel(std::cell::Cell<bool>);
impl verify_core::target::ScenarioTarget for AlternatingModel {
    type State = ();
    type Step = serde_json::Value;
    fn init(&self, _: &serde_json::Value) -> Result<(), verify_core::target::ScenarioError> {
        Ok(())
    }
    fn apply(
        &self,
        (): &mut (),
        _: &Self::Step,
    ) -> Result<verify_core::target::StepOutcome, verify_core::target::ScenarioError> {
        let previous = self.0.replace(!self.0.get());
        Ok(if previous {
            verify_core::target::StepOutcome::Applied
        } else {
            verify_core::target::StepOutcome::rejected("unavailable")
        })
    }
    fn check(
        &self,
        id: &verify_core::InvariantId,
        (): &(),
    ) -> Result<verify_core::InvariantResult, verify_core::target::ScenarioError> {
        Ok(verify_core::InvariantResult::new(
            id.clone(),
            verify_core::verdict::Verdict::Verified,
        ))
    }
}
#[test]
fn determinism_checks_step_outcomes_even_when_final_verdicts_match() {
    let targets = TargetRegistry::new().with(
        TargetKey::new("fixture", "request"),
        Box::new(AlternatingModel(std::cell::Cell::new(false))),
    );
    let scenario = scenario(&json!([{}]), "invariant_holds");
    assert!(!check_determinism(&scenario, Kind::Safety, &targets, 3).stable);
    assert_eq!(
        verify_runner::normalize(&scenario, Kind::Safety, &targets, 3).status,
        ScenarioStatus::Quarantined
    );
}

struct LiteralModel(bool);
impl verify_core::target::ScenarioTarget for LiteralModel {
    fn allows_identifier_renaming(&self) -> bool {
        self.0
    }
    type State = bool;
    type Step = String;
    fn init(&self, _: &serde_json::Value) -> Result<bool, verify_core::target::ScenarioError> {
        Ok(false)
    }
    fn apply(
        &self,
        state: &mut bool,
        step: &String,
    ) -> Result<verify_core::target::StepOutcome, verify_core::target::ScenarioError> {
        *state = step == "literal_7";
        Ok(verify_core::target::StepOutcome::Applied)
    }
    fn check(
        &self,
        id: &verify_core::InvariantId,
        state: &bool,
    ) -> Result<verify_core::InvariantResult, verify_core::target::ScenarioError> {
        Ok(verify_core::InvariantResult::new(
            id.clone(),
            if *state {
                verify_core::verdict::Verdict::Falsified
            } else {
                verify_core::verdict::Verdict::Verified
            },
        ))
    }
}
#[test]
fn normalization_discards_identifier_rewriting_that_changes_the_verdict() {
    let targets = TargetRegistry::new().with(
        TargetKey::new("fixture", "request"),
        Box::new(LiteralModel(true)),
    );
    let scenario = scenario(&json!(["literal_7"]), "invariant_violated");
    let normalized = verify_runner::normalize(&scenario, Kind::Safety, &targets, 3);
    assert_eq!(
        normalized.steps,
        json!(["literal_7"]).as_array().unwrap().clone()
    );
    assert_eq!(normalized.status, ScenarioStatus::Active);
    assert_eq!(
        replay(&normalized, Kind::Safety, &targets, "guarded").outcome,
        Outcome::Matched
    );
}

#[test]
fn normalization_preserves_literal_payloads_without_identifier_opt_in() {
    let targets = TargetRegistry::new().with(
        TargetKey::new("fixture", "request"),
        Box::new(LiteralModel(false)),
    );
    let scenario = scenario(&json!(["literal_8"]), "invariant_holds");
    let normalized = verify_runner::normalize(&scenario, Kind::Safety, &targets, 3);
    assert_eq!(normalized.steps, vec![json!("literal_8")]);
}

#[test]
fn repaired_violation_counts_for_safety_but_not_final_state_properties() {
    let targets = TargetRegistry::new().with(
        TargetKey::new("fixture", "request"),
        Box::new(LiteralModel(false)),
    );
    let scenario = scenario(&json!(["literal_7", "repaired"]), "invariant_violated");
    assert_eq!(
        replay(&scenario, Kind::Safety, &targets, "safety").outcome,
        Outcome::Matched
    );
    assert_eq!(
        replay(&scenario, Kind::Liveness, &targets, "liveness").outcome,
        Outcome::Mismatched {
            expected: Expect::InvariantViolated,
            observed: Expect::InvariantHolds
        }
    );
}
