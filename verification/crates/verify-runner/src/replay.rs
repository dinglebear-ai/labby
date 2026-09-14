use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use verify_core::{InvariantResult, Kind, ScenarioTarget, StepOutcome};
use verify_scenario::{Expectation, Status, ValidatedScenario};

/// Bounded execution settings for trusted, synchronous, project-owned targets.
/// Deadlines are cooperative: checked around calls, not a way to interrupt a
/// hung target. Run untrusted or potentially hanging targets in a bounded process.
#[derive(Debug, Clone)]
pub struct ReplayLimits {
    /// Maximum transitions to apply.
    pub max_steps: usize,
    /// Elapsed-time budget, checked after decoding/init/check/apply calls.
    pub timeout: Duration,
}

impl Default for ReplayLimits {
    fn default() -> Self {
        Self {
            max_steps: verify_scenario::MAX_STEPS,
            timeout: Duration::from_secs(5),
        }
    }
}

/// Aggregate of a finite trace, deliberately distinct from backend Verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceVerdict {
    /// Every observation holds for the supported finite predicate semantics.
    InvariantHolds,
    /// At least one concrete violation; later recovery cannot erase it.
    InvariantViolated,
    /// The trace or temporal obligation could not be completely evaluated.
    Incomplete,
    /// Invalid input, target lookup, or harness failure; not a counterexample.
    Error,
}

/// Initial-state or post-transition observation (position zero is initial).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// Zero for initialization; one-based position for a completed transition.
    pub position: usize,
    /// Absent for the initial state; legitimate rejection is retained.
    pub outcome: Option<StepOutcome>,
    /// Target's predicate observation.
    pub result: InvariantResult,
}

/// Machine-readable replay result; never mutates the input corpus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReport {
    /// Content identity of the input scenario.
    pub fingerprint: String,
    /// Aggregate finite-trace result.
    pub verdict: TraceVerdict,
    /// First violated observation, zero for an initially invalid state.
    pub first_violation: Option<usize>,
    /// Completed observations, including legitimate step rejections.
    pub observations: Vec<Observation>,
    /// Redacted failure/incompleteness explanation supplied by the harness.
    pub diagnostic: Option<String>,
    /// Whether the finite result equals the scenario expectation.
    pub matches_expectation: bool,
    /// Only active mismatches fail the scenario lane.
    pub gate_failure: bool,
    /// Matching unreproduced evidence deserves review, not automatic mutation.
    pub suggested_promotion: bool,
    /// Original scenario lifecycle, so non-gating evidence remains visible.
    pub status: Status,
}

impl ReplayReport {
    pub(crate) fn new(scenario: &ValidatedScenario, verdict: TraceVerdict) -> Self {
        let mut result = Self {
            fingerprint: scenario.fingerprint(),
            verdict,
            first_violation: None,
            observations: Vec::new(),
            diagnostic: None,
            matches_expectation: false,
            gate_failure: false,
            suggested_promotion: false,
            status: scenario.scenario().status,
        };
        result.finish(scenario);
        result
    }

    pub(crate) fn fail(
        mut self,
        scenario: &ValidatedScenario,
        verdict: TraceVerdict,
        diagnostic: String,
    ) -> Self {
        self.verdict = verdict;
        self.diagnostic = Some(diagnostic);
        self.finish(scenario);
        self
    }

    fn finish(&mut self, scenario: &ValidatedScenario) {
        self.matches_expectation = matches!(
            (self.verdict, scenario.scenario().expect),
            (TraceVerdict::InvariantHolds, Expectation::InvariantHolds)
                | (
                    TraceVerdict::InvariantViolated,
                    Expectation::InvariantViolated
                )
        );
        self.gate_failure = self.status == Status::Active && !self.matches_expectation;
        self.suggested_promotion = self.status == Status::Unreproduced && self.matches_expectation;
    }
}

pub(crate) fn execute<T: ScenarioTarget>(
    target: &T,
    scenario: &ValidatedScenario,
    kind: Kind,
    limits: &ReplayLimits,
) -> ReplayReport {
    let started = Instant::now();
    let mut report = ReplayReport::new(scenario, TraceVerdict::InvariantHolds);
    let raw = scenario.scenario();
    // M2 has no temporal monitor or refinement observation-relation adapter.
    if !matches!(kind, Kind::Safety | Kind::Security) {
        return report.fail(
            scenario,
            TraceVerdict::Incomplete,
            "finite replay requires an explicit temporal/refinement monitor for this kind".into(),
        );
    }
    if raw.steps.len() > limits.max_steps || limits.timeout.is_zero() {
        return report.fail(
            scenario,
            TraceVerdict::Incomplete,
            "replay budget exhausted before execution".into(),
        );
    }
    let steps: Vec<T::Step> = match raw
        .steps
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect()
    {
        Ok(steps) => steps,
        Err(error) => {
            return report.fail(
                scenario,
                TraceVerdict::Error,
                format!("invalid target step: {error}"),
            );
        }
    };
    let initial = serde_json::Value::Object(raw.initial.clone());
    if started.elapsed() >= limits.timeout {
        return report.fail(
            scenario,
            TraceVerdict::Incomplete,
            "replay deadline exceeded during decoding".into(),
        );
    }
    let mut state = match target.init(&initial) {
        Ok(state) => state,
        Err(error) => return report.fail(scenario, TraceVerdict::Error, error.to_string()),
    };
    for position in 0..=steps.len() {
        if started.elapsed() >= limits.timeout {
            return report.fail(
                scenario,
                TraceVerdict::Incomplete,
                "replay deadline exceeded".into(),
            );
        }
        let outcome = if position == 0 {
            None
        } else {
            match target.apply(&mut state, &steps[position - 1]) {
                Ok(outcome) => Some(outcome),
                Err(error) => return report.fail(scenario, TraceVerdict::Error, error.to_string()),
            }
        };
        if started.elapsed() >= limits.timeout {
            return report.fail(
                scenario,
                TraceVerdict::Incomplete,
                "replay deadline exceeded during transition".into(),
            );
        }
        let result = match target.check(&raw.invariant, &state) {
            Ok(result) => result,
            Err(error) => return report.fail(scenario, TraceVerdict::Error, error.to_string()),
        };
        match &result {
            InvariantResult::Violated { .. } => {
                report.first_violation.get_or_insert(position);
                report.verdict = TraceVerdict::InvariantViolated;
            }
            InvariantResult::Incomplete { .. } if report.first_violation.is_none() => {
                report.verdict = TraceVerdict::Incomplete;
            }
            _ => {}
        }
        report.observations.push(Observation {
            position,
            outcome,
            result,
        });
        if started.elapsed() >= limits.timeout {
            return report.fail(
                scenario,
                TraceVerdict::Incomplete,
                "replay deadline exceeded".into(),
            );
        }
    }
    report.finish(scenario);
    report
}
