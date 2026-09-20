//! Model-to-implementation conformance contract self-tests.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use verify_core::{
    ConformanceFailure, ConformanceTarget, InvariantId, InvariantResult, OutcomeClass,
    RelationStatus, ScenarioError, ScenarioTarget, StepOutcome, compare_trace, compare_trace_with,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Step {
    Increment,
}

#[derive(Clone, Debug)]
struct ModelState {
    count: u8,
}

struct Model {
    init_fails: bool,
    apply_fails: bool,
    rejects: bool,
}

impl ScenarioTarget for Model {
    type State = ModelState;
    type Step = Step;
    fn init(&self, initial: &serde_json::Value) -> Result<Self::State, ScenarioError> {
        if self.init_fails {
            return Err(ScenarioError::InvalidInitial("model init failure".into()));
        }
        Ok(ModelState {
            count: initial["count"].as_u64().unwrap_or_default() as u8,
        })
    }
    fn apply(&self, state: &mut Self::State, _: &Step) -> Result<StepOutcome, ScenarioError> {
        if self.apply_fails {
            return Err(ScenarioError::Harness("model apply failure".into()));
        }
        if self.rejects {
            return Ok(StepOutcome::Rejected {
                reason: "model rejection".into(),
            });
        }
        state.count += 1;
        Ok(StepOutcome::Applied {})
    }
    fn check(&self, _: &InvariantId, _: &Self::State) -> Result<InvariantResult, ScenarioError> {
        Ok(InvariantResult::Holds {})
    }
}

#[derive(Debug)]
struct AdapterState {
    public_total: u8,
    cleanup_marker: bool,
}

#[derive(Debug)]
struct AdapterObservation {
    public_total: u8,
}

#[derive(Default)]
struct Adapter {
    apply_fails: bool,
    observe_fails: bool,
    rejects: bool,
    increment_by: u8,
    pending_after_mutation: bool,
}

impl ConformanceTarget<Model> for Adapter {
    type ImplementationState = AdapterState;
    type Observation = AdapterObservation;
    async fn apply(
        &self,
        state: &mut AdapterState,
        _: &Step,
    ) -> Result<StepOutcome, ScenarioError> {
        if self.apply_fails {
            return Err(ScenarioError::Harness("adapter apply failure".into()));
        }
        if self.rejects {
            return Ok(StepOutcome::Rejected {
                reason: "HTTP 409".into(),
            });
        }
        state.public_total += self.increment_by;
        if self.pending_after_mutation {
            std::future::pending().await
        } else {
            Ok(StepOutcome::Applied {})
        }
    }
    async fn observe(&self, state: &AdapterState) -> Result<AdapterObservation, ScenarioError> {
        if self.observe_fails {
            return Err(ScenarioError::Harness("adapter observe failure".into()));
        }
        Ok(AdapterObservation {
            public_total: state.public_total,
        })
    }
    fn observations_conform(
        &self,
        model: &ModelState,
        implementation: &AdapterObservation,
    ) -> Result<(), String> {
        if model.count == implementation.public_total {
            Ok(())
        } else {
            Err(format!(
                "model count {} != public total {}",
                model.count, implementation.public_total
            ))
        }
    }
}

fn model() -> Model {
    Model {
        init_fails: false,
        apply_fails: false,
        rejects: false,
    }
}
fn state(count: u8) -> AdapterState {
    AdapterState {
        public_total: count,
        cleanup_marker: false,
    }
}

#[test]
fn compares_initial_and_every_controlled_step() {
    let adapter = Adapter {
        increment_by: 1,
        ..Adapter::default()
    };
    let mut implementation = state(3);
    let steps = [Step::Increment, Step::Increment];
    let mut positions = Vec::new();
    let trace = block_on(compare_trace_with(
        &model(),
        &adapter,
        &mut implementation,
        &json!({"count":3}),
        &steps,
        |row| positions.push(row.position),
    ));
    assert!(trace.conforms());
    assert_eq!(positions, [0, 1, 2]);
    assert_eq!(trace.observations.len(), steps.len() + 1);
    assert_eq!(implementation.public_total, 5);
}

#[test]
fn initial_and_post_step_relation_divergence_retain_actual_observation() {
    let adapter = Adapter {
        increment_by: 2,
        ..Adapter::default()
    };
    let mut initially_wrong = state(4);
    let initial = block_on(compare_trace(
        &model(),
        &adapter,
        &mut initially_wrong,
        &json!({"count":3}),
        &[],
    ));
    assert_eq!(initial.observations.len(), 1);
    assert!(matches!(
        initial.observations[0].relation,
        RelationStatus::Diverges { .. }
    ));
    assert!(matches!(
        initial.failure,
        Some(ConformanceFailure::ObservationDivergence { position: 0, .. })
    ));

    let mut implementation = state(3);
    let post = block_on(compare_trace(
        &model(),
        &adapter,
        &mut implementation,
        &json!({"count":3}),
        &[Step::Increment],
    ));
    assert_eq!(post.observations.len(), 2);
    assert_eq!(post.observations[1].implementation.public_total, 5);
    assert!(matches!(
        post.observations[1].relation,
        RelationStatus::Diverges { .. }
    ));
}

#[test]
fn both_outcome_mismatch_directions_observe_before_failing() {
    for (model_rejects, adapter_rejects, expected_model, expected_adapter) in [
        (false, true, OutcomeClass::Applied, OutcomeClass::Rejected),
        (true, false, OutcomeClass::Rejected, OutcomeClass::Applied),
    ] {
        let model = Model {
            rejects: model_rejects,
            ..model()
        };
        let adapter = Adapter {
            rejects: adapter_rejects,
            increment_by: u8::from(!adapter_rejects),
            ..Adapter::default()
        };
        let mut implementation = state(0);
        let trace = block_on(compare_trace(
            &model,
            &adapter,
            &mut implementation,
            &json!({"count":0}),
            &[Step::Increment],
        ));
        assert_eq!(trace.observations.len(), 2);
        assert_eq!(trace.observations[1].model_outcome, Some(expected_model));
        assert_eq!(
            trace.observations[1].implementation_outcome,
            Some(expected_adapter)
        );
        assert!(matches!(
            trace.failure,
            Some(ConformanceFailure::OutcomeDivergence { position: 1, .. })
        ));
    }
}

#[test]
fn init_apply_and_observe_errors_are_distinct_and_retain_prefix() {
    let cases = [
        (
            Model {
                init_fails: true,
                ..model()
            },
            Adapter::default(),
            0,
            "model_init",
        ),
        (
            Model {
                apply_fails: true,
                ..model()
            },
            Adapter::default(),
            1,
            "model_apply",
        ),
        (
            model(),
            Adapter {
                apply_fails: true,
                ..Adapter::default()
            },
            1,
            "implementation_apply",
        ),
    ];
    for (model, adapter, count, name) in cases {
        let mut implementation = state(0);
        let trace = block_on(compare_trace(
            &model,
            &adapter,
            &mut implementation,
            &json!({"count":0}),
            &[Step::Increment],
        ));
        assert_eq!(trace.observations.len(), count);
        assert_eq!(failure_name(trace.failure.as_ref().unwrap()), name);
    }
    let adapter = Adapter {
        observe_fails: true,
        ..Adapter::default()
    };
    let mut implementation = state(0);
    let trace = block_on(compare_trace(
        &model(),
        &adapter,
        &mut implementation,
        &json!({"count":0}),
        &[],
    ));
    assert!(trace.observations.is_empty());
    assert!(matches!(
        trace.failure,
        Some(ConformanceFailure::ImplementationObserve { position: 0, .. })
    ));
}

#[test]
fn dropping_pending_comparison_retains_caller_owned_state_for_cleanup() {
    let adapter = Adapter {
        increment_by: 1,
        pending_after_mutation: true,
        ..Adapter::default()
    };
    let mut implementation = state(0);
    let model = model();
    let initial = json!({"count":0});
    let steps = [Step::Increment];
    {
        let mut future = Box::pin(compare_trace(
            &model,
            &adapter,
            &mut implementation,
            &initial,
            &steps,
        ));
        assert!(matches!(poll_once(future.as_mut()), Poll::Pending));
    }
    assert_eq!(implementation.public_total, 1);
    implementation.cleanup_marker = true;
    assert!(implementation.cleanup_marker);
}

fn failure_name(value: &ConformanceFailure) -> &'static str {
    match value {
        ConformanceFailure::ModelInit { .. } => "model_init",
        ConformanceFailure::ModelApply { .. } => "model_apply",
        ConformanceFailure::ImplementationApply { .. } => "implementation_apply",
        ConformanceFailure::ImplementationObserve { .. } => "implementation_observe",
        ConformanceFailure::OutcomeDivergence { .. } => "outcome_divergence",
        ConformanceFailure::ObservationDivergence { .. } => "observation_divergence",
    }
}

fn poll_once<F: Future>(future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    let waker = Waker::noop();
    future.poll(&mut Context::from_waker(waker))
}
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    loop {
        match poll_once(future.as_mut()) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
