use std::fmt;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::InvariantId;

/// A modeled transition outcome, distinct from a harness failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepOutcome {
    /// The transition was applied.
    Applied {},
    /// The model legitimately rejected the transition.
    Rejected {
        /// Model-defined explanation, safe for reporting.
        reason: String,
    },
}

/// One observation of a property, not a proof over an entire execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvariantResult {
    /// The predicate holds at this observation.
    Holds {},
    /// A concrete violation was observed.
    Violated {
        /// Project-supplied, redacted diagnostic.
        reason: String,
    },
    /// A temporal obligation or observation is not yet decidable.
    Incomplete {
        /// Why this observation cannot establish the property.
        reason: String,
    },
}

/// Invalid input or failed harness operation; never a modeled violation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScenarioError {
    /// The initial state could not be decoded or accepted.
    #[error("invalid initial state: {0}")]
    InvalidInitial(String),
    /// The requested property is not implemented by this target.
    #[error("unknown invariant: {0}")]
    UnknownInvariant(InvariantId),
    /// An operation failed outside the modeled transition semantics.
    #[error("scenario harness failed: {0}")]
    Harness(String),
}

/// Project-owned deterministic state machine consumed by the replay runner.
pub trait ScenarioTarget {
    /// State retained between steps.
    type State: Clone + fmt::Debug;
    /// Opaque, project-specific step encoding.
    type Step: Serialize + DeserializeOwned + Clone + fmt::Debug;

    /// Construct initial state; the runner supplies an object, never JSON null.
    fn init(&self, initial: &serde_json::Value) -> Result<Self::State, ScenarioError>;
    /// Apply a step, distinguishing rejection from harness failure.
    fn apply(
        &self,
        state: &mut Self::State,
        step: &Self::Step,
    ) -> Result<StepOutcome, ScenarioError>;
    /// Evaluate a property; unknown IDs and harness failures must not mean Holds.
    fn check(
        &self,
        id: &InvariantId,
        state: &Self::State,
    ) -> Result<InvariantResult, ScenarioError>;
    /// Optional project-owned canonical identifier renaming. The toolkit cannot
    /// infer which opaque fields are identifiers. Rename initial state and steps
    /// consistently in first-appearance order; the default preserves both.
    /// Renaming must not add, remove or reorder actions. Length changes are
    /// rejected; only the runner's separately gated delta debugger removes steps.
    /// The runner discards transformations that change the trace verdict.
    fn canonicalize(
        &self,
        initial: &serde_json::Value,
        steps: &[Self::Step],
    ) -> Result<(serde_json::Value, Vec<Self::Step>), ScenarioError> {
        Ok((initial.clone(), steps.to_vec()))
    }
    /// Optional normalization hint; nothing commutes unless explicitly declared.
    fn commutes(&self, _a: &Self::Step, _b: &Self::Step) -> bool {
        false
    }
}
