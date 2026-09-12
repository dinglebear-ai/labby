//! The replay-driven normalization passes.
//!
//! These live here rather than in `verify-scenario` because they must execute a
//! scenario to learn anything, and replay lives in this crate. Putting them
//! beside the syntactic passes would be a dependency cycle.

use verify_core::Kind;
use verify_report::replay::Outcome;
use verify_scenario::{Expect, Scenario};

use crate::registry::TargetRegistry;
use crate::replay::replay;

/// Shrink a failing scenario to the steps that actually matter.
///
/// Delta debugging by removal: drop each step in turn, keep the removal if the
/// scenario still reproduces. Quadratic in the number of steps, which is fine
/// for counterexamples — they are small by the time anyone commits them.
///
/// **Only `expect: invariant_violated` scenarios are minimized.** Verdict-
/// preserving minimization of a golden trace shrinks it to the empty trace,
/// which trivially still "holds" — satisfying the guard while destroying the
/// entire value of the scenario.
pub fn minimize(scenario: &Scenario, kind: Kind, registry: &TargetRegistry) -> Scenario {
    if scenario.expect != Expect::InvariantViolated {
        return scenario.clone();
    }
    if !reproduces(scenario, kind, registry) {
        // Never minimize something that does not reproduce in the first place:
        // every candidate would "still fail to reproduce" and the trace would
        // shrink to nothing.
        return scenario.clone();
    }

    let mut current = scenario.clone();
    let mut index = 0;
    while index < current.steps.len() {
        let mut candidate = current.clone();
        candidate.steps.remove(index);
        if reproduces(&candidate, kind, registry) {
            current = candidate;
            // Do not advance: the next step has shifted into this slot.
        } else {
            index += 1;
        }
    }
    current
}

fn reproduces(scenario: &Scenario, kind: Kind, registry: &TargetRegistry) -> bool {
    let report = replay(scenario, kind, registry, "minimize");
    matches!(report.outcome, Outcome::Matched | Outcome::Promotable)
}

/// What repeated replay of the same scenario found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeterminismReport {
    pub runs: usize,
    pub stable: bool,
}

/// Replay a scenario `runs` times and report whether it always agreed.
///
/// Be honest about what this buys: replaying a pure function proves nothing. It
/// catches exactly one thing — a target whose `apply` reads `HashMap` iteration
/// order, wall-clock time, or an RNG. That is a common way to write an
/// accidentally nondeterministic model and worth catching cheaply; it is not a
/// general safety net.
pub fn check_determinism(
    scenario: &Scenario,
    kind: Kind,
    registry: &TargetRegistry,
    runs: usize,
) -> DeterminismReport {
    let runs = runs.max(1);
    let first = replay(scenario, kind, registry, "determinism");
    let mut stable = true;
    for _ in 1..runs {
        let again = replay(scenario, kind, registry, "determinism");
        stable &= again == first;
    }
    DeterminismReport { runs, stable }
}

/// Normalize with replay guards before fingerprinting a scenario for the corpus.
///
/// Syntactic identifier rewriting is heuristic: if it changes the observed
/// verdict or makes a trace unreadable, retain the original trace instead.
/// Nondeterministic replays are quarantined, never silently made active.
pub fn normalize(
    scenario: &Scenario,
    kind: Kind,
    registry: &TargetRegistry,
    runs: usize,
) -> Scenario {
    struct TargetCommutes<'a>(&'a dyn crate::DynTarget);
    impl verify_scenario::Commutes for TargetCommutes<'_> {
        fn commutes(&self, a: &serde_json::Value, b: &serde_json::Value) -> bool {
            self.0.commutes_erased(a, b)
        }
    }
    let baseline = replay(scenario, kind, registry, "normalize");
    let mut out = scenario.clone();
    if matches!(
        baseline.outcome,
        Outcome::Matched | Outcome::Mismatched { .. } | Outcome::Promotable
    ) && let Some(target) =
        registry.get(&crate::TargetKey::new(&scenario.project, &scenario.model))
    {
        let candidate = if target.allows_identifier_renaming() {
            verify_scenario::normalize_syntactic(scenario, &TargetCommutes(target))
        } else {
            verify_scenario::normalize::normalize_commuting(scenario, &TargetCommutes(target))
        };
        if replay(&candidate, kind, registry, "normalize").outcome == baseline.outcome {
            out = minimize(&candidate, kind, registry);
        }
    }
    if !check_determinism(&out, kind, registry, runs).stable {
        out.status = verify_scenario::ScenarioStatus::Quarantined;
    }
    out.fingerprint = Some(verify_scenario::fingerprint(&out));
    out
}
