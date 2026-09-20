//! Labby-owned Stateright bindings, finite bounds, and replay qualification.

use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    num::NonZeroU64,
};

use labby_model::{BrowserRequestModel, BrowserRequestState, MODEL, Step};
use serde_json::json;
use stateright::Property;
use verify_core::{
    Backend, BackendRegistry, Bounds, CheckPlan, InvariantId, InvariantResult, ScenarioError,
    ScenarioTarget, StepOutcome, Verdict,
};
use verify_runner::{ReplayLimits, TargetRegistry, TraceVerdict};
use verify_scenario::Scenario;
use verify_stateright::{BfsHarness, ScenarioMetadata, StaterightBackend};

const HANDLES: [(&str, &str); 5] = [
    ("LABBY-REQ-001", "request_single_terminal"),
    ("LABBY-REQ-002", "request_terminal_cleanup"),
    ("LABBY-REQ-003", "request_terminal_immutable"),
    ("LABBY-REQ-004", "request_generation_owner"),
    ("LABBY-REQ-005", "request_cancel_before_dispatch"),
];

fn kani_metadata() -> verify_kani::KaniBackend {
    let mut kani = verify_kani::KaniBackend::new("missing-test-kani-driver");
    kani.register(
        MODEL,
        "request_single_terminal",
        verify_kani::KaniHarness::new("missing-test-harness.rs", "request_single_terminal")
            .unwrap(),
    )
    .unwrap();
    kani
}

fn bounds() -> Bounds {
    BTreeMap::from([
        ("max_actions".into(), json!(32)),
        ("max_depth".into(), json!(12)),
        ("max_states".into(), json!(20_000)),
    ])
}

fn plan(invariant: &str, handle: &str) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from(invariant.to_owned()).unwrap(),
        model: MODEL.into(),
        handle: handle.into(),
        bounds: bounds(),
        seed: None,
        timeout_ms: NonZeroU64::new(2_000).unwrap(),
    }
}

#[test]
fn catalog_registers_all_five_stateright_handles_without_running_search() {
    let backend = labby_verify::stateright::backend().unwrap();
    let mut backends = BackendRegistry::default();
    backends.register(&backend).unwrap();
    let kani = kani_metadata();
    backends.register(&kani).unwrap();
    let catalog = labby_model::catalog(&backends).unwrap();
    assert_eq!(catalog.catalog().invariant.len(), HANDLES.len());
    for (_, handle) in HANDLES {
        assert!(backend.has_handle(MODEL, handle));
    }
}

#[test]
fn production_model_is_clean_within_the_declared_t1_bounds() {
    let backend = labby_verify::stateright::backend().unwrap();
    for (invariant, handle) in HANDLES {
        let report = backend.run(&plan(invariant, handle));
        assert!(
            matches!(report.verdict, Verdict::Bounded { .. }),
            "{invariant}/{handle}: {:?}",
            report.verdict
        );
        assert!(report.scenarios.is_empty());
    }
}

#[test]
fn seeded_bfs_is_rejected_instead_of_silently_ignored() {
    let backend = labby_verify::stateright::backend().unwrap();
    let mut seeded = plan(HANDLES[0].0, HANDLES[0].1);
    seeded.seed = Some(7);
    let report = backend.run(&seeded);
    assert!(matches!(report.verdict, Verdict::Error { .. }));
}

#[derive(Clone, Debug)]
struct BrokenState {
    model: BrowserRequestState,
    key: String,
}

impl BrokenState {
    fn new(model: BrowserRequestState) -> Self {
        let key = serde_json::to_string(&model).unwrap();
        Self { model, key }
    }
}

impl PartialEq for BrokenState {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for BrokenState {}

impl Hash for BrokenState {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

#[derive(Clone, Copy)]
struct BrokenLifecycle;

fn broken_actions(state: &BrowserRequestState) -> Vec<Step> {
    if state.current_generation.is_none() {
        vec![Step::Connect {
            generation: "generation_0".into(),
        }]
    } else if !state.active.contains_key("request_0") && !state.terminal.contains_key("request_0") {
        vec![Step::Admit {
            request: "request_0".into(),
        }]
    } else if state
        .active
        .get("request_0")
        .is_some_and(|active| active.phase == labby_model::RequestPhase::Admitted)
    {
        vec![Step::Dispatch {
            request: "request_0".into(),
        }]
    } else if state.active.contains_key("request_0") {
        vec![Step::CompleteSuccess {
            request: "request_0".into(),
            generation: "generation_0".into(),
        }]
    } else {
        Vec::new()
    }
}

fn apply_broken(
    state: &mut BrowserRequestState,
    step: &Step,
) -> Result<StepOutcome, ScenarioError> {
    let outcome = BrowserRequestModel.apply(state, step)?;
    if matches!(outcome, StepOutcome::Applied {}) && matches!(step, Step::CompleteSuccess { .. }) {
        state.terminal_writes.insert("request_0".into(), 2);
    }
    Ok(outcome)
}

fn project_step(step: &Step) -> Result<serde_json::Value, String> {
    serde_json::to_value(step).map_err(|error| error.to_string())
}

fn broken_property(invariant: &InvariantId) -> Option<&'static str> {
    (invariant.as_str() == "LABBY-REQ-001").then_some("broken_single_terminal")
}

fn broken_holds(_: &BrokenLifecycle, state: &BrokenState) -> bool {
    let invariant = InvariantId::try_from("LABBY-REQ-001".to_owned()).unwrap();
    matches!(
        BrowserRequestModel.check(&invariant, &state.model),
        Ok(InvariantResult::Holds {})
    )
}

impl stateright::Model for BrokenLifecycle {
    type State = BrokenState;
    type Action = Step;

    fn init_states(&self) -> Vec<Self::State> {
        vec![BrokenState::new(
            BrowserRequestModel.init(&json!({})).unwrap(),
        )]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        actions.extend(broken_actions(&state.model));
    }

    fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
        let mut next = state.model.clone();
        matches!(
            apply_broken(&mut next, &action),
            Ok(StepOutcome::Applied {})
        )
        .then(|| BrokenState::new(next))
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![Property::always("broken_single_terminal", broken_holds)]
    }
}

#[derive(Clone, Copy)]
struct BrokenReplayTarget;

impl ScenarioTarget for BrokenReplayTarget {
    type State = BrowserRequestState;
    type Step = Step;

    fn init(&self, initial: &serde_json::Value) -> Result<Self::State, ScenarioError> {
        BrowserRequestModel.init(initial)
    }

    fn apply(
        &self,
        state: &mut Self::State,
        step: &Self::Step,
    ) -> Result<StepOutcome, ScenarioError> {
        apply_broken(state, step)
    }

    fn check(
        &self,
        id: &InvariantId,
        state: &Self::State,
    ) -> Result<InvariantResult, ScenarioError> {
        BrowserRequestModel.check(id, state)
    }

    fn canonicalize(
        &self,
        initial: &serde_json::Value,
        steps: &[Self::Step],
    ) -> Result<(serde_json::Value, Vec<Self::Step>), ScenarioError> {
        BrowserRequestModel.canonicalize(initial, steps)
    }
}

#[test]
fn test_only_broken_lifecycle_emits_a_replayable_violation() {
    let mut backend = StaterightBackend::new();
    backend
        .register(
            MODEL,
            "broken_single_terminal",
            BfsHarness::new(
                BrokenLifecycle,
                ScenarioMetadata {
                    project: "labby".into(),
                    initial: serde_json::Map::new(),
                },
                broken_property,
                project_step,
            ),
        )
        .unwrap();
    let report = backend.run(&plan("LABBY-REQ-001", "broken_single_terminal"));
    assert!(matches!(report.verdict, Verdict::Falsified { .. }));
    assert_eq!(report.scenarios.len(), 1);

    let scenario: Scenario = serde_json::from_value(report.scenarios[0].clone()).unwrap();
    let scenario = scenario.validate().unwrap();
    let production_backend = labby_verify::stateright::backend().unwrap();
    let mut backends = BackendRegistry::default();
    backends.register(&production_backend).unwrap();
    let kani = kani_metadata();
    backends.register(&kani).unwrap();
    let catalog = labby_model::catalog(&backends).unwrap();
    let mut targets = TargetRegistry::default();
    targets
        .register(&catalog, MODEL, BrokenReplayTarget)
        .unwrap();
    let replay = targets.replay(
        &scenario,
        &ReplayLimits {
            max_steps: 16,
            timeout: std::time::Duration::from_secs(1),
        },
    );
    assert_eq!(replay.verdict, TraceVerdict::InvariantViolated);
}
