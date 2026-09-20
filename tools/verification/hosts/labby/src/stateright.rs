//! Finite Stateright domain harness for the browser-request lifecycle model.

use std::hash::{Hash, Hasher};

use labby_model::{
    BrowserRequestModel, BrowserRequestState, CAPABILITY_VISIBILITY_MODEL, CapabilityCode,
    CapabilityPhase, CapabilityVisibilityModel, CapabilityVisibilityState,
    CapabilityVisibilityStep, MODEL, RequestPhase, RuntimeCapability, Step,
};
use serde_json::json;
use stateright::Property;
use verify_core::{InvariantId, InvariantResult, ScenarioTarget, StepOutcome};
use verify_stateright::{BfsHarness, ScenarioMetadata, StaterightBackend};

const REQUESTS: [&str; 2] = ["request_0", "request_1"];
const GENERATIONS: [&str; 2] = ["generation_0", "generation_1"];

const REQ_001_HANDLE: &str = "request_single_terminal";
const REQ_002_HANDLE: &str = "request_terminal_cleanup";
const REQ_003_HANDLE: &str = "request_terminal_immutable";
const REQ_004_HANDLE: &str = "request_generation_owner";
const REQ_005_HANDLE: &str = "request_cancel_before_dispatch";
const CAP_001_HANDLE: &str = "degradation_visible_in_doctor";
const CAP_002_HANDLE: &str = "degradation_visible_in_readiness";
const CAP_003_HANDLE: &str = "degradation_has_operator_detail";
const CAP_004_HANDLE: &str = "fatal_guard_blocks_startup";

/// Construct the Labby-owned Stateright adapter with all catalog handles.
pub fn backend() -> Result<StaterightBackend, String> {
    backend_from_prefix(&[])
}

/// Explore after a validated incident prefix, retaining that prefix in every
/// projected counterexample rather than losing its reproduction context.
pub fn backend_from_prefix(prefix: &[Step]) -> Result<StaterightBackend, String> {
    if prefix.len() > 256 {
        return Err("incident prefix exceeds 256 steps".into());
    }
    let mut state = BrowserRequestModel
        .init(&json!({}))
        .map_err(|_| "invalid initial model")?;
    for step in prefix {
        BrowserRequestModel
            .apply(&mut state, step)
            .map_err(|_| "invalid incident prefix")?;
    }
    let mut backend = StaterightBackend::new();
    for handle in [
        REQ_001_HANDLE,
        REQ_002_HANDLE,
        REQ_003_HANDLE,
        REQ_004_HANDLE,
        REQ_005_HANDLE,
    ] {
        backend
            .register(
                MODEL,
                handle,
                BfsHarness::new(
                    BrowserLifecycle {
                        prefix: prefix.to_vec(),
                    },
                    ScenarioMetadata {
                        project: "labby".into(),
                        initial: serde_json::Map::new(),
                    },
                    property_name,
                    project_action,
                ),
            )
            .map_err(|error| error.to_string())?;
    }
    for handle in [
        CAP_001_HANDLE,
        CAP_002_HANDLE,
        CAP_003_HANDLE,
        CAP_004_HANDLE,
    ] {
        backend
            .register(
                CAPABILITY_VISIBILITY_MODEL,
                handle,
                BfsHarness::new(
                    CapabilityLifecycle,
                    ScenarioMetadata {
                        project: "labby".into(),
                        initial: serde_json::Map::new(),
                    },
                    capability_property_name,
                    project_capability_action,
                ),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(backend)
}

fn project_action(action: &Step) -> Result<serde_json::Value, String> {
    serde_json::to_value(action).map_err(|error| error.to_string())
}

fn property_name(invariant: &InvariantId) -> Option<&'static str> {
    match invariant.as_str() {
        "LABBY-REQ-001" => Some(REQ_001_HANDLE),
        "LABBY-REQ-002" => Some(REQ_002_HANDLE),
        "LABBY-REQ-003" => Some(REQ_003_HANDLE),
        "LABBY-REQ-004" => Some(REQ_004_HANDLE),
        "LABBY-REQ-005" => Some(REQ_005_HANDLE),
        _ => None,
    }
}

/// Hashable state wrapper whose identity is the canonical serialized model state.
#[derive(Clone, Debug)]
struct State {
    model: BrowserRequestState,
    canonical: String,
    prefix_position: usize,
}

impl State {
    fn new(model: BrowserRequestState) -> Self {
        let canonical =
            serde_json::to_string(&model).expect("BrowserRequestState serialization is infallible");
        Self {
            model,
            canonical,
            prefix_position: 0,
        }
    }
}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        self.canonical == other.canonical && self.prefix_position == other.prefix_position
    }
}

impl Eq for State {}

impl Hash for State {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.canonical.hash(state);
        self.prefix_position.hash(state);
    }
}

/// Small deterministic domain around the production-independent transition model.
#[derive(Clone, Debug, Default)]
struct BrowserLifecycle {
    prefix: Vec<Step>,
}

impl stateright::Model for BrowserLifecycle {
    type State = State;
    type Action = Step;

    fn init_states(&self) -> Vec<Self::State> {
        let state = BrowserRequestModel
            .init(&json!({}))
            .expect("empty initial browser-request state is valid");
        vec![State::new(state)]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        if let Some(step) = self.prefix.get(state.prefix_position) {
            actions.push(step.clone());
            return;
        }
        let model = &state.model;
        for generation in GENERATIONS {
            if !model.seen_generations.contains(generation) {
                if model.current_generation.is_none() {
                    actions.push(Step::Connect {
                        generation: generation.into(),
                    });
                } else {
                    actions.push(Step::ReplaceConnection {
                        generation: generation.into(),
                    });
                }
            }
        }

        if let Some(current) = &model.current_generation {
            actions.push(Step::Disconnect {
                generation: current.clone(),
            });
            for request in REQUESTS {
                if !model.active.contains_key(request) && !model.terminal.contains_key(request) {
                    actions.push(Step::Admit {
                        request: request.into(),
                    });
                }
            }
        }

        for (request, active) in &model.active {
            match active.phase {
                RequestPhase::Admitted => {
                    actions.push(Step::Dispatch {
                        request: request.clone(),
                    });
                    actions.push(Step::FailBeforeDispatch {
                        request: request.clone(),
                    });
                }
                RequestPhase::Dispatched => {
                    for generation in GENERATIONS {
                        actions.push(Step::CompleteSuccess {
                            request: request.clone(),
                            generation: generation.into(),
                        });
                        actions.push(Step::CompleteError {
                            request: request.clone(),
                            generation: generation.into(),
                        });
                    }
                    actions.push(Step::Timeout {
                        request: request.clone(),
                    });
                }
            }
            actions.push(Step::Cancel {
                request: request.clone(),
            });
            actions.push(Step::InvalidateDocument {
                request: request.clone(),
            });
        }
    }

    fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        let mut next = last_state.model.clone();
        match BrowserRequestModel.apply(&mut next, &action) {
            Ok(outcome)
                if last_state.prefix_position < self.prefix.len()
                    || matches!(outcome, StepOutcome::Applied {}) =>
            {
                let mut state = State::new(next);
                state.prefix_position = (last_state.prefix_position + 1).min(self.prefix.len());
                Some(state)
            }
            Ok(_) | Err(_) => None,
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always(REQ_001_HANDLE, |_, state| holds("LABBY-REQ-001", state)),
            Property::always(REQ_002_HANDLE, |_, state| holds("LABBY-REQ-002", state)),
            Property::always(REQ_003_HANDLE, |_, state| holds("LABBY-REQ-003", state)),
            Property::always(REQ_004_HANDLE, |_, state| holds("LABBY-REQ-004", state)),
            Property::always(REQ_005_HANDLE, |_, state| holds("LABBY-REQ-005", state)),
        ]
    }
}

fn holds(id: &str, state: &State) -> bool {
    let invariant = InvariantId::try_from(id.to_owned()).expect("static invariant ID is valid");
    matches!(
        BrowserRequestModel.check(&invariant, &state.model),
        Ok(InvariantResult::Holds {})
    )
}

fn project_capability_action(
    action: &CapabilityVisibilityStep,
) -> Result<serde_json::Value, String> {
    serde_json::to_value(action).map_err(|error| error.to_string())
}

fn capability_property_name(invariant: &InvariantId) -> Option<&'static str> {
    match invariant.as_str() {
        "LABBY-CAP-001" => Some(CAP_001_HANDLE),
        "LABBY-CAP-002" => Some(CAP_002_HANDLE),
        "LABBY-CAP-003" => Some(CAP_003_HANDLE),
        "LABBY-CAP-004" => Some(CAP_004_HANDLE),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct CapabilityState {
    model: CapabilityVisibilityState,
    canonical: String,
}

impl CapabilityState {
    fn new(model: CapabilityVisibilityState) -> Self {
        let canonical = serde_json::to_string(&model)
            .expect("CapabilityVisibilityState serialization is infallible");
        Self { model, canonical }
    }
}

impl PartialEq for CapabilityState {
    fn eq(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }
}

impl Eq for CapabilityState {}

impl Hash for CapabilityState {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.canonical.hash(state);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct CapabilityLifecycle;

impl stateright::Model for CapabilityLifecycle {
    type State = CapabilityState;
    type Action = CapabilityVisibilityStep;

    fn init_states(&self) -> Vec<Self::State> {
        let state = CapabilityVisibilityModel
            .init(&json!({}))
            .expect("empty capability-visibility state is valid");
        vec![CapabilityState::new(state)]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        match state.model.phase {
            CapabilityPhase::Configuring => {
                if !state.model.remote_bind {
                    actions.push(CapabilityVisibilityStep::BindRemote);
                }
                if !state.model.exact_remote_host {
                    actions.push(CapabilityVisibilityStep::AllowExactRemoteHost);
                }
                if !state.model.wildcard_host {
                    actions.push(CapabilityVisibilityStep::AllowWildcardHost);
                }
                if !state.model.host_validation_disabled {
                    actions.push(CapabilityVisibilityStep::DisableHostValidation);
                }
                if !state.model.gateway_feature_disabled {
                    actions.push(CapabilityVisibilityStep::DisableGatewayFeature);
                }
                if !state.model.upstream_configured {
                    actions.push(CapabilityVisibilityStep::ConfigureUpstream);
                }
                if !state.model.protected_route_configured {
                    actions.push(CapabilityVisibilityStep::ConfigureProtectedRoute);
                }
                actions.push(CapabilityVisibilityStep::Start);
            }
            CapabilityPhase::Running => {
                if !state
                    .model
                    .degraded
                    .contains(&CapabilityCode::ArtifactsUnavailable)
                {
                    actions.push(CapabilityVisibilityStep::RecordRuntimeDegradation {
                        code: RuntimeCapability::Artifacts,
                    });
                }
                if !state
                    .model
                    .degraded
                    .contains(&CapabilityCode::UsageTelemetryUnavailable)
                {
                    actions.push(CapabilityVisibilityStep::RecordRuntimeDegradation {
                        code: RuntimeCapability::UsageTelemetry,
                    });
                }
            }
            CapabilityPhase::Blocked => {}
        }
    }

    fn next_state(&self, last_state: &Self::State, action: Self::Action) -> Option<Self::State> {
        let mut next = last_state.model.clone();
        match CapabilityVisibilityModel.apply(&mut next, &action) {
            Ok(StepOutcome::Applied {}) => Some(CapabilityState::new(next)),
            Ok(StepOutcome::Rejected { .. }) | Err(_) => None,
        }
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always(CAP_001_HANDLE, |_, state| {
                holds_capability("LABBY-CAP-001", state)
            }),
            Property::always(CAP_002_HANDLE, |_, state| {
                holds_capability("LABBY-CAP-002", state)
            }),
            Property::always(CAP_003_HANDLE, |_, state| {
                holds_capability("LABBY-CAP-003", state)
            }),
            Property::always(CAP_004_HANDLE, |_, state| {
                holds_capability("LABBY-CAP-004", state)
            }),
        ]
    }
}

fn holds_capability(id: &str, state: &CapabilityState) -> bool {
    let invariant = InvariantId::try_from(id.to_owned()).expect("static invariant ID is valid");
    matches!(
        CapabilityVisibilityModel.check(&invariant, &state.model),
        Ok(InvariantResult::Holds {})
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_domain_is_finite_and_deterministic() {
        let model = BrowserLifecycle::default();
        let state = &stateright::Model::init_states(&model)[0];
        let mut first = Vec::new();
        let mut second = Vec::new();
        stateright::Model::actions(&model, state, &mut first);
        stateright::Model::actions(&model, state, &mut second);
        assert_eq!(first, second);
        assert_eq!(first.len(), GENERATIONS.len());
    }

    #[test]
    fn every_catalog_invariant_resolves_to_a_declared_property() {
        let names: Vec<_> = stateright::Model::properties(&BrowserLifecycle::default())
            .into_iter()
            .map(|property| property.name)
            .collect();
        for id in 1..=5 {
            let invariant = InvariantId::try_from(format!("LABBY-REQ-{id:03}"))
                .expect("generated invariant ID is valid");
            assert!(names.contains(&property_name(&invariant).expect("mapping exists")));
        }
    }

    #[test]
    fn capability_action_domain_is_finite_and_deterministic() {
        let model = CapabilityLifecycle;
        let state = &stateright::Model::init_states(&model)[0];
        let mut first = Vec::new();
        let mut second = Vec::new();
        stateright::Model::actions(&model, state, &mut first);
        stateright::Model::actions(&model, state, &mut second);
        assert_eq!(first, second);
        assert!(first.contains(&CapabilityVisibilityStep::Start));
        assert_eq!(first.len(), 8);
    }

    #[test]
    fn every_capability_invariant_resolves_to_a_declared_property() {
        let names: Vec<_> = stateright::Model::properties(&CapabilityLifecycle)
            .into_iter()
            .map(|property| property.name)
            .collect();
        for id in 1..=4 {
            let invariant = InvariantId::try_from(format!("LABBY-CAP-{id:03}"))
                .expect("generated invariant ID is valid");
            assert!(names.contains(&capability_property_name(&invariant).expect("mapping exists")));
        }
    }

    #[test]
    fn incident_prefix_keeps_rejected_steps_and_then_opens_exploration() {
        let prefix = vec![
            Step::Connect {
                generation: "generation_0".into(),
            },
            Step::Cancel {
                request: "request_0".into(),
            },
        ];
        let model = BrowserLifecycle {
            prefix: prefix.clone(),
        };
        let mut state = stateright::Model::init_states(&model).remove(0);
        for (position, expected) in prefix.into_iter().enumerate() {
            let mut actions = Vec::new();
            stateright::Model::actions(&model, &state, &mut actions);
            assert_eq!(actions, vec![expected.clone()]);
            state = stateright::Model::next_state(&model, &state, expected).unwrap();
            assert_eq!(state.prefix_position, position + 1);
        }
        let mut actions = Vec::new();
        stateright::Model::actions(&model, &state, &mut actions);
        assert!(actions.contains(&Step::Admit {
            request: "request_0".into()
        }));
    }
}
