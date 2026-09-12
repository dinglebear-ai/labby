//! The interface a project implements so the toolkit can replay its scenarios.
//!
//! This is not everything a fully instrumented project implements. A search
//! backend needs to enumerate *available* steps from a state, which replay
//! never does; Stateright's `Model`, for example, additionally requires
//! `actions(&self, state, &mut Vec<Action>)`. [`ScenarioTarget`] deliberately
//! has no analogue and should not grow one — a project adopting a search
//! backend implements both traits over shared `State`/`Step` types, keeping the
//! backend's requirements in the backend's layer.
//!
//! Bounds here are deliberately minimal. Stateright additionally wants
//! `Hash + Eq` on both associated types; a project using it adds those itself
//! rather than every project paying for a backend it may never run.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::invariant::InvariantId;
use crate::verdict::InvariantResult;

/// What happened when a step was applied.
///
/// Rejection is a modeled outcome, not a harness failure: a model that refuses
/// an illegal step is doing its job, and the distinction matters when a
/// minimizer is deciding whether a step mattered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    /// The step applied and may have changed the state.
    Applied,
    /// The step was legally refused by the model.
    Rejected { reason: String },
}

impl StepOutcome {
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
        }
    }

    pub const fn is_applied(&self) -> bool {
        matches!(self, Self::Applied)
    }
}

/// Something that went wrong in the harness, as distinct from the model
/// legitimately rejecting a step.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ScenarioError {
    #[error("scenario initial state is not valid for this target: {reason}")]
    Initial { reason: String },
    #[error("step could not be interpreted by this target: {reason}")]
    Step { reason: String },
    #[error("invariant {invariant} is not known to this target")]
    UnknownInvariant { invariant: InvariantId },
}

impl ScenarioError {
    pub fn initial(reason: impl Into<String>) -> Self {
        Self::Initial {
            reason: reason.into(),
        }
    }

    pub fn step(reason: impl Into<String>) -> Self {
        Self::Step {
            reason: reason.into(),
        }
    }
}

/// A project's model, as far as scenario replay is concerned.
pub trait ScenarioTarget {
    /// The modeled state.
    type State: Clone + fmt::Debug;
    /// One modeled transition. Serializable because it is what a committed
    /// scenario file stores, and what a backend's counterexample projects into.
    type Step: Serialize + DeserializeOwned + Clone + fmt::Debug;

    /// Build the starting state from the scenario's opaque `initial` value.
    ///
    /// An absent `initial` is normalized to `{}` before this is called, so
    /// implementations never have to distinguish absent from null.
    fn init(&self, initial: &serde_json::Value) -> Result<Self::State, ScenarioError>;

    /// Apply one step in place.
    fn apply(
        &self,
        state: &mut Self::State,
        step: &Self::Step,
    ) -> Result<StepOutcome, ScenarioError>;

    /// Evaluate one catalogued invariant against the current state.
    fn check(
        &self,
        invariant: &InvariantId,
        state: &Self::State,
    ) -> Result<InvariantResult, ScenarioError>;

    /// Declare that two adjacent steps commute, letting normalization put them
    /// in a canonical order and collapse traces that differ only by irrelevant
    /// interleaving.
    ///
    /// The default is "nothing commutes", which is always sound and merely
    /// weaker at deduplication. Only override this where the claim is true:
    /// wrongly declaring commutativity silently merges genuinely distinct
    /// counterexamples.
    fn commutes(&self, _a: &Self::Step, _b: &Self::Step) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_is_not_application() {
        assert!(StepOutcome::Applied.is_applied());
        assert!(!StepOutcome::rejected("closed").is_applied());
    }

    #[test]
    fn scenario_errors_name_the_offending_stage() {
        assert_eq!(
            ScenarioError::initial("missing `session`").to_string(),
            "scenario initial state is not valid for this target: missing `session`"
        );
        assert_eq!(
            ScenarioError::step("unknown variant `frobnicate`").to_string(),
            "step could not be interpreted by this target: unknown variant `frobnicate`"
        );
    }

    #[test]
    fn unknown_invariant_names_the_id() {
        let invariant = InvariantId::parse("LABBY-REQ-001").expect("valid");
        let message = ScenarioError::UnknownInvariant {
            invariant: invariant.clone(),
        }
        .to_string();
        assert!(message.contains(invariant.as_str()));
    }
}
