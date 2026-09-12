//! What one scenario replay produced.

use serde::{Deserialize, Serialize};
use verify_core::verdict::InvariantResult;
use verify_scenario::{Expect, ScenarioStatus};

/// The result of replaying one scenario, and the reason for it.
///
/// This is the type the exit-code contract is derived from, so its variants
/// are the distinctions that matter operationally — not a bool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
#[non_exhaustive]
pub enum Outcome {
    /// Replay observed what `expect` said it should.
    Matched,
    /// Replay did not. On an `active` scenario this is the real failure.
    Mismatched { expected: Expect, observed: Expect },
    /// An `unreproduced` scenario started matching. The model has grown the
    /// step it was missing; the runner surfaces this rather than silently
    /// rewriting the file.
    Promotable,
    /// The scenario named a `(project, model)` with no registered target.
    /// Deliberately distinct from a mismatch: "nobody registered the target"
    /// must never be readable as "the invariant holds".
    NoTarget { project: String, model: String },
    /// The scenario or its steps could not be interpreted.
    Malformed { reason: String },
}

impl Outcome {
    /// Whether this outcome should fail CI for a scenario with this status.
    pub const fn fails(&self, status: ScenarioStatus) -> bool {
        match self {
            Self::Matched | Self::Promotable => false,
            Self::NoTarget { .. } | Self::Malformed { .. } => true,
            // Quarantined and unreproduced scenarios are reported, not gated.
            Self::Mismatched { .. } => matches!(status, ScenarioStatus::Active),
        }
    }
}

/// One step's effect during replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub index: usize,
    /// Whether the model applied or legally refused the step.
    pub applied: bool,
    /// The invariant's verdict after this step.
    pub result: InvariantResult,
}

/// A full replay report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub scenario: String,
    pub project: String,
    pub model: String,
    pub invariant: String,
    pub expect: Expect,
    pub status: ScenarioStatus,
    pub outcome: Outcome,
    /// Index of the first step at which the invariant was false, when it was.
    /// A safety property violated mid-trace and later repaired still counts,
    /// and this is where a reader looks first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_violation: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepRecord>,
}

impl ReplayReport {
    pub const fn failed(&self) -> bool {
        self.outcome.fails(self.status)
    }
}

/// Aggregate across a corpus.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub total: usize,
    pub matched: usize,
    pub mismatched: usize,
    pub promotable: usize,
    pub no_target: usize,
    pub malformed: usize,
    /// How many of the above actually fail CI, which is not the same as how
    /// many did not match.
    pub failing: usize,
}

impl Summary {
    pub const fn accumulate(&mut self, report: &ReplayReport) {
        self.total += 1;
        match report.outcome {
            Outcome::Matched => self.matched += 1,
            Outcome::Mismatched { .. } => self.mismatched += 1,
            Outcome::Promotable => self.promotable += 1,
            Outcome::NoTarget { .. } => self.no_target += 1,
            Outcome::Malformed { .. } => self.malformed += 1,
        }
        if report.failed() {
            self.failing += 1;
        }
    }
}
