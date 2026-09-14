//! Determinism compares evidence for repeated identical inputs, not just verdicts.
#[allow(dead_code)]
#[path = "fixtures/counter.rs"]
mod counter;

use std::cell::Cell;

use serde_json::Value;
use verify_core::{InvariantId, InvariantResult, ScenarioError, ScenarioTarget, StepOutcome};
use verify_runner::{NormalizationOptions, ReplayLimits, TargetRegistry, TraceVerdict};
use verify_scenario::Status;

struct ChangingEvidence {
    next: Cell<bool>,
    initial: bool,
    after_renaming: bool,
}

impl ScenarioTarget for ChangingEvidence {
    type State = (bool, bool);
    type Step = counter::Step;

    fn init(&self, initial: &Value) -> Result<Self::State, ScenarioError> {
        let changing = !self.after_renaming || initial.get("renamed") == Some(&Value::Bool(true));
        let alternate = changing && self.next.replace(!self.next.get());
        Ok((alternate, false))
    }

    fn apply(&self, state: &mut Self::State, _: &Self::Step) -> Result<StepOutcome, ScenarioError> {
        state.1 = true;
        Ok(if !self.initial && state.0 {
            StepOutcome::Rejected {
                reason: "alternating outcome".into(),
            }
        } else {
            StepOutcome::Applied {}
        })
    }

    fn check(
        &self,
        _: &InvariantId,
        state: &Self::State,
    ) -> Result<InvariantResult, ScenarioError> {
        Ok(if self.initial && !state.1 {
            InvariantResult::Violated {
                reason: format!("initial evidence {}", state.0),
            }
        } else {
            InvariantResult::Holds {}
        })
    }

    fn canonicalize(
        &self,
        initial: &Value,
        steps: &[Self::Step],
    ) -> Result<(Value, Vec<Self::Step>), ScenarioError> {
        let mut initial = initial.clone();
        if self.after_renaming {
            initial["renamed"] = Value::Bool(true);
        }
        Ok((initial, steps.to_vec()))
    }
}

fn assert_quarantined(initial: bool, after_renaming: bool) {
    let mut runner = TargetRegistry::default();
    runner
        .register(
            &counter::catalog("safety"),
            "counter",
            ChangingEvidence {
                next: Cell::new(false),
                initial,
                after_renaming,
            },
        )
        .unwrap();
    // A golden expectation prevents shrinking from erasing the observed step.
    let trace = counter::scenario(&[0], "invariant_holds", "active");
    let first = runner.replay(&trace, &ReplayLimits::default());
    let second = runner.replay(&trace, &ReplayLimits::default());
    assert_eq!(first.verdict, second.verdict);
    assert_eq!(
        first.verdict,
        if initial {
            TraceVerdict::InvariantViolated
        } else {
            TraceVerdict::InvariantHolds
        }
    );
    if !after_renaming {
        assert_ne!(first.observations, second.observations);
    }
    let normalized = runner
        .normalize(&trace, &NormalizationOptions::default())
        .unwrap();
    assert_eq!(normalized.scenario.scenario().status, Status::Quarantined);
}

#[test]
fn changing_initial_evidence_with_the_same_verdict_is_quarantined() {
    for after_renaming in [false, true] {
        assert_quarantined(true, after_renaming);
    }
}

#[test]
fn changing_step_outcomes_with_the_same_verdict_are_quarantined() {
    for after_renaming in [false, true] {
        assert_quarantined(false, after_renaming);
    }
}
