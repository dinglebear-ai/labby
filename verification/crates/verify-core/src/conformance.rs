//! Generic model-to-implementation conformance comparison.

use std::fmt;

use serde_json::Value;

use crate::{ScenarioError, ScenarioTarget, StepOutcome};

/// Public-boundary adapter for comparing an implementation with a scenario model.
///
/// The implementation observation is produced independently. The project-owned
/// relation is the only place that interprets it against the model state. The
/// caller must initialize `ImplementationState` before comparison and retain it
/// in a cancellation-safe RAII owner so dropping the comparison future cannot
/// bypass cleanup.
pub trait ConformanceTarget<M: ScenarioTarget> {
    /// State retained by the real implementation adapter between controlled steps.
    type ImplementationState: fmt::Debug;
    /// Independently collected, project-defined implementation observation.
    type Observation: fmt::Debug;

    /// Apply one model-vocabulary step through the implementation boundary.
    fn apply(
        &self,
        state: &mut Self::ImplementationState,
        step: &M::Step,
    ) -> impl Future<Output = Result<StepOutcome, ScenarioError>>;

    /// Observe the implementation without deriving values from the model state.
    fn observe(
        &self,
        state: &Self::ImplementationState,
    ) -> impl Future<Output = Result<Self::Observation, ScenarioError>>;

    /// Decide the project's explicit observation relation.
    fn observations_conform(
        &self,
        model: &M::State,
        implementation: &Self::Observation,
    ) -> Result<(), String>;
}

/// Semantic transition class shared across independently worded domains.
///
/// Rejection reasons are intentionally excluded: a model and a public adapter
/// can describe the same legitimate rejection with different text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeClass {
    /// The controlled transition was applied.
    Applied,
    /// The controlled transition was legitimately rejected.
    Rejected,
}

impl From<&StepOutcome> for OutcomeClass {
    fn from(value: &StepOutcome) -> Self {
        match value {
            StepOutcome::Applied {} => Self::Applied,
            StepOutcome::Rejected { .. } => Self::Rejected,
        }
    }
}

/// One collected initial or post-step conformance observation.
#[derive(Debug)]
pub struct ConformanceObservation<O> {
    /// Zero for the initial observation; otherwise the one-based step position.
    pub position: usize,
    /// Model outcome for a step, absent for the initial observation.
    pub model_outcome: Option<OutcomeClass>,
    /// Implementation outcome for a step, absent for the initial observation.
    pub implementation_outcome: Option<OutcomeClass>,
    /// Independently collected implementation observation.
    pub implementation: O,
    /// Result of applying the project-owned observation relation.
    pub relation: RelationStatus,
}

/// Result of comparing one independently collected implementation observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationStatus {
    /// The implementation observation relates to the model state.
    Conforms,
    /// The observation relation found a concrete divergence.
    Diverges {
        /// Project-supplied, redacted relation diagnostic.
        reason: String,
    },
}

/// First failed comparison operation, with its trace position retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConformanceFailure {
    /// The model rejected its initial input as invalid.
    ModelInit {
        /// Redacted model initialization diagnostic.
        reason: String,
    },
    /// The model could not execute a controlled step.
    ModelApply {
        /// One-based step position.
        position: usize,
        /// Redacted model harness diagnostic.
        reason: String,
    },
    /// The implementation adapter could not execute a controlled step.
    ImplementationApply {
        /// One-based step position.
        position: usize,
        /// Redacted adapter harness diagnostic.
        reason: String,
    },
    /// The implementation adapter could not collect an observation.
    ImplementationObserve {
        /// Zero for initial state; otherwise the one-based step position.
        position: usize,
        /// Redacted adapter observation diagnostic.
        reason: String,
    },
    /// Model and implementation disagreed about applying versus rejecting a step.
    OutcomeDivergence {
        /// One-based step position.
        position: usize,
        /// Model transition class.
        model: OutcomeClass,
        /// Implementation transition class.
        implementation: OutcomeClass,
    },
    /// The project observation relation rejected independently collected state.
    ObservationDivergence {
        /// Zero for initial state; otherwise the one-based step position.
        position: usize,
        /// Project-supplied, redacted relation diagnostic.
        reason: String,
    },
}

/// Comparison result retaining all collected observations through divergence.
#[derive(Debug)]
pub struct ConformanceTrace<O> {
    /// Every collected initial or post-step observation, including divergence.
    pub observations: Vec<ConformanceObservation<O>>,
    /// First failure, or `None` when the complete trace conforms.
    pub failure: Option<ConformanceFailure>,
}

impl<O> ConformanceTrace<O> {
    /// Whether the initial state and every controlled step conformed.
    pub fn conforms(&self) -> bool {
        self.failure.is_none()
    }
}

/// Compare an initial state and every controlled step against caller-owned state.
///
/// Dropping this future releases its mutable borrow without consuming the
/// implementation state, leaving caller-controlled cleanup possible.
pub async fn compare_trace<M, T>(
    model: &M,
    target: &T,
    implementation_state: &mut T::ImplementationState,
    initial: &Value,
    steps: &[M::Step],
) -> ConformanceTrace<T::Observation>
where
    M: ScenarioTarget,
    T: ConformanceTarget<M>,
{
    compare_trace_with(model, target, implementation_state, initial, steps, |_| {}).await
}

/// Compare a trace and report each collected observation as it occurs.
pub async fn compare_trace_with<M, T, F>(
    model: &M,
    target: &T,
    implementation_state: &mut T::ImplementationState,
    initial: &Value,
    steps: &[M::Step],
    mut on_observation: F,
) -> ConformanceTrace<T::Observation>
where
    M: ScenarioTarget,
    T: ConformanceTarget<M>,
    F: FnMut(&ConformanceObservation<T::Observation>),
{
    let mut observations = Vec::with_capacity(steps.len() + 1);
    let mut model_state = match model.init(initial) {
        Ok(state) => state,
        Err(error) => {
            return failed(
                observations,
                ConformanceFailure::ModelInit {
                    reason: error.to_string(),
                },
            );
        }
    };
    match observe(
        target,
        &model_state,
        implementation_state,
        ObservationContext {
            position: 0,
            model_outcome: None,
            implementation_outcome: None,
        },
        &mut observations,
        &mut on_observation,
    )
    .await
    {
        Ok(Some(failure)) | Err(failure) => return failed(observations, failure),
        Ok(None) => {}
    }

    for (index, step) in steps.iter().enumerate() {
        let position = index + 1;
        let model_outcome = match model.apply(&mut model_state, step) {
            Ok(outcome) => outcome,
            Err(error) => {
                return failed(
                    observations,
                    ConformanceFailure::ModelApply {
                        position,
                        reason: error.to_string(),
                    },
                );
            }
        };
        let implementation_outcome = match target.apply(implementation_state, step).await {
            Ok(outcome) => outcome,
            Err(error) => {
                return failed(
                    observations,
                    ConformanceFailure::ImplementationApply {
                        position,
                        reason: error.to_string(),
                    },
                );
            }
        };
        let model_class = OutcomeClass::from(&model_outcome);
        let implementation_class = OutcomeClass::from(&implementation_outcome);
        let relation_failure = match observe(
            target,
            &model_state,
            implementation_state,
            ObservationContext {
                position,
                model_outcome: Some(model_class),
                implementation_outcome: Some(implementation_class),
            },
            &mut observations,
            &mut on_observation,
        )
        .await
        {
            Ok(failure) => failure,
            Err(failure) => return failed(observations, failure),
        };
        if model_class != implementation_class {
            return failed(
                observations,
                ConformanceFailure::OutcomeDivergence {
                    position,
                    model: model_class,
                    implementation: implementation_class,
                },
            );
        }
        if let Some(failure) = relation_failure {
            return failed(observations, failure);
        }
    }

    ConformanceTrace {
        observations,
        failure: None,
    }
}

struct ObservationContext {
    position: usize,
    model_outcome: Option<OutcomeClass>,
    implementation_outcome: Option<OutcomeClass>,
}

async fn observe<M, T, F>(
    target: &T,
    model_state: &M::State,
    implementation_state: &T::ImplementationState,
    context: ObservationContext,
    observations: &mut Vec<ConformanceObservation<T::Observation>>,
    on_observation: &mut F,
) -> Result<Option<ConformanceFailure>, ConformanceFailure>
where
    M: ScenarioTarget,
    T: ConformanceTarget<M>,
    F: FnMut(&ConformanceObservation<T::Observation>),
{
    let implementation = match target.observe(implementation_state).await {
        Ok(observation) => observation,
        Err(error) => {
            return Err(ConformanceFailure::ImplementationObserve {
                position: context.position,
                reason: error.to_string(),
            });
        }
    };
    let relation = match target.observations_conform(model_state, &implementation) {
        Ok(()) => RelationStatus::Conforms,
        Err(reason) => RelationStatus::Diverges { reason },
    };
    let relation_failure = match &relation {
        RelationStatus::Conforms => None,
        RelationStatus::Diverges { reason } => Some(ConformanceFailure::ObservationDivergence {
            position: context.position,
            reason: reason.clone(),
        }),
    };
    let observation = ConformanceObservation {
        position: context.position,
        model_outcome: context.model_outcome,
        implementation_outcome: context.implementation_outcome,
        implementation,
        relation,
    };
    on_observation(&observation);
    observations.push(observation);
    Ok(relation_failure)
}

fn failed<O>(
    observations: Vec<ConformanceObservation<O>>,
    failure: ConformanceFailure,
) -> ConformanceTrace<O> {
    ConformanceTrace {
        observations,
        failure: Some(failure),
    }
}
