//! The replay engine.
//!
//! Resolve the target, deserialize the steps, initialise, apply each step while
//! evaluating the invariant, then compare what was observed against what the
//! scenario says to expect.
//!
//! Two rules do most of the work here, and both come from SPEC §8:
//!
//! - **Violated at any step**, for safety and security properties. A property
//!   that goes false mid-trace and is repaired before the end still failed, and
//!   judging only the final state would silently pass the most interesting
//!   counterexamples a model checker produces.
//! - **`expect` and `status` are separate axes.** A mismatch is a failure only
//!   on an `active` scenario; quarantined and unreproduced ones are reported.

use verify_core::verdict::Verdict;
use verify_core::{InvariantId, Kind};
use verify_report::replay::{Outcome, ReplayReport, StepRecord};
use verify_scenario::{Expect, Scenario, ScenarioStatus};

use crate::registry::{TargetKey, TargetRegistry};

/// Replay one scenario against the registry.
///
/// `kind` says how the invariant is judged; pass the value from the project's
/// catalog. Without a catalog the safe default is [`Kind::Safety`], which
/// judges over the whole trace and so cannot miss a violation.
pub fn replay(
    scenario: &Scenario,
    kind: Kind,
    registry: &TargetRegistry,
    label: &str,
) -> ReplayReport {
    let key = TargetKey::new(&scenario.project, &scenario.model);
    let base = |outcome: Outcome, first_violation, steps| ReplayReport {
        scenario: label.to_owned(),
        project: scenario.project.clone(),
        model: scenario.model.clone(),
        invariant: scenario.invariant.to_string(),
        expect: scenario.expect,
        status: scenario.status,
        outcome,
        first_violation,
        steps,
    };

    let Some(target) = registry.get(&key) else {
        return base(
            Outcome::NoTarget {
                project: scenario.project.clone(),
                model: scenario.model.clone(),
            },
            None,
            Vec::new(),
        );
    };

    let mut state = match target.init_erased(&scenario.initial_value()) {
        Ok(state) => state,
        Err(error) => {
            return base(
                Outcome::Malformed {
                    reason: error.to_string(),
                },
                None,
                Vec::new(),
            );
        }
    };

    let invariant: &InvariantId = &scenario.invariant;
    let (records, first_violation) = match run_steps(target, scenario, invariant, &mut state) {
        Ok(outcome) => outcome,
        Err(reason) => return base(Outcome::Malformed { reason }, None, Vec::new()),
    };

    let observed = observed_expectation(kind, first_violation, &records);
    let outcome = if observed == scenario.expect {
        if scenario.status == ScenarioStatus::Unreproduced {
            // It matches now. Surface the promotion rather than rewriting the
            // file underneath the author.
            Outcome::Promotable
        } else {
            Outcome::Matched
        }
    } else {
        Outcome::Mismatched {
            expected: scenario.expect,
            observed,
        }
    };

    base(outcome, first_violation, records)
}

/// Walk the trace, recording each step's effect and the invariant after it.
///
/// The invariant is evaluated before any step too: a scenario whose initial
/// state already violates it is a real finding, not a step-zero blind spot.
/// That is why a violation index of 0 means "already false on arrival".
fn run_steps(
    target: &dyn crate::dyn_target::DynTarget,
    scenario: &Scenario,
    invariant: &InvariantId,
    state: &mut crate::dyn_target::ErasedState,
) -> Result<(Vec<StepRecord>, Option<usize>), String> {
    let mut records = Vec::with_capacity(scenario.steps.len());
    let mut first_violation = None;

    let initial = target
        .check_erased(invariant, state)
        .map_err(|error| error.to_string())?;
    if is_violation(&initial.verdict) {
        first_violation = Some(0);
    }

    for (index, step) in scenario.steps.iter().enumerate() {
        let applied = target
            .apply_erased(state, step)
            .map_err(|error| format!("step {index}: {error}"))?
            .is_applied();
        let result = target
            .check_erased(invariant, state)
            .map_err(|error| format!("step {index}: {error}"))?;
        if is_violation(&result.verdict) && first_violation.is_none() {
            first_violation = Some(index + 1);
        }
        records.push(StepRecord {
            index,
            applied,
            result,
        });
    }
    Ok((records, first_violation))
}

/// What replay actually observed, expressed in the same vocabulary as `expect`.
fn observed_expectation(
    kind: Kind,
    first_violation: Option<usize>,
    records: &[StepRecord],
) -> Expect {
    let violated = if kind.violated_at_any_step() {
        first_violation.is_some()
    } else {
        // Liveness and refinement are judged at the end of the trace: an
        // intermediate state that has not yet satisfied the property is not a
        // counterexample, it is the middle of one.
        records
            .last()
            .is_some_and(|record| is_violation(&record.result.verdict))
    };
    if violated {
        Expect::InvariantViolated
    } else {
        Expect::InvariantHolds
    }
}

const fn is_violation(verdict: &Verdict) -> bool {
    matches!(verdict, Verdict::Falsified)
}
