//! Domain-neutral bounded Stateright adapter.
//!
//! Adopting projects register typed [`stateright::Model`] values and own the
//! mapping from catalog invariants and model actions to portable scenarios.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    hash::Hash,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use stateright::{Checker, Model, Property};
use verify_core::{
    Availability, Backend, BackendId, BackendReport, Bounds, Capabilities, CheckPlan, InvariantId,
    Kind, Verdict,
};
use verify_scenario::{Expectation, Origin, OriginKind, Scenario, Status};

const BACKEND_ID: &str = "stateright";
const TOOL_VERSION: &str = "0.31.0";
const PROPERTY_NAME: &str = "verify-stateright-selected-invariant";

/// Scenario fields supplied by the adopting project rather than inferred by the adapter.
#[derive(Debug, Clone)]
pub struct ScenarioMetadata {
    /// Stable adopting-project identity.
    pub project: String,
    /// Opaque initial state used by the project's replay target.
    pub initial: serde_json::Map<String, serde_json::Value>,
}

/// Duplicate or invalid model/handle registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    /// Model and handle keys must be nonempty.
    EmptyKey,
    /// Replacing a binding implicitly would change catalog meaning.
    Duplicate {
        /// Registered model key.
        model: String,
        /// Registered handle key.
        handle: String,
    },
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyKey => output.write_str("model and handle must be nonempty"),
            Self::Duplicate { model, handle } => {
                write!(
                    output,
                    "Stateright harness already registered: {model}/{handle}"
                )
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

trait Harness: Send + Sync {
    fn run(&self, plan: &CheckPlan, limits: SearchLimits) -> HarnessReport;
}

/// A typed BFS harness registered behind the domain-neutral backend interface.
pub struct BfsHarness<M, R, P> {
    model: M,
    metadata: ScenarioMetadata,
    property: R,
    project_action: P,
}

impl<M, R, P> BfsHarness<M, R, P> {
    /// Construct a harness. `property` resolves a catalog invariant to a
    /// Stateright property name already declared by `model`. `project_action`
    /// emits the corresponding replay-target step JSON.
    pub fn new(model: M, metadata: ScenarioMetadata, property: R, project_action: P) -> Self {
        Self {
            model,
            metadata,
            property,
            project_action,
        }
    }
}

/// Caller-populated Stateright backend registry.
#[derive(Default)]
pub struct StaterightBackend {
    harnesses: BTreeMap<(String, String), Box<dyn Harness>>,
}

impl StaterightBackend {
    /// Create an empty adapter. Registration performs no model checking.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one project-owned typed model under a catalog model/handle pair.
    pub fn register<M, R, P>(
        &mut self,
        model: impl Into<String>,
        handle: impl Into<String>,
        harness: BfsHarness<M, R, P>,
    ) -> Result<(), RegistrationError>
    where
        M: Model + Clone + Send + Sync + 'static,
        M::State: Clone + Hash + Send + Sync + Eq + 'static,
        M::Action: Clone + Send + Sync + PartialEq + 'static,
        R: Fn(&InvariantId) -> Option<&'static str> + Send + Sync + 'static,
        P: Fn(&M::Action) -> Result<serde_json::Value, String> + Send + Sync + 'static,
    {
        let key = (model.into(), handle.into());
        if key.0.trim().is_empty() || key.1.trim().is_empty() {
            return Err(RegistrationError::EmptyKey);
        }
        if self.harnesses.contains_key(&key) {
            return Err(RegistrationError::Duplicate {
                model: key.0,
                handle: key.1,
            });
        }
        self.harnesses.insert(key, Box::new(harness));
        Ok(())
    }
}

impl Backend for StaterightBackend {
    fn id(&self) -> BackendId {
        BackendId::try_from(BACKEND_ID.to_owned()).expect("constant backend id is valid")
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kinds: BTreeSet::from([Kind::Safety, Kind::Security]),
            fairness: false,
            concurrency: false,
            bounded: true,
        }
    }

    fn has_handle(&self, model: &str, handle: &str) -> bool {
        self.harnesses
            .contains_key(&(model.to_owned(), handle.to_owned()))
    }

    fn availability(&self) -> Availability {
        Availability::Ready {}
    }

    fn run(&self, plan: &CheckPlan) -> BackendReport {
        let mut report = BackendReport {
            backend: self.id(),
            invariant: plan.invariant.clone(),
            verdict: Verdict::Error {
                reason: "invalid Stateright invocation".into(),
            },
            tool_version: Some(TOOL_VERSION.into()),
            scenarios: Vec::new(),
        };
        if plan.seed.is_some() {
            report.verdict = Verdict::Error {
                reason: "Stateright BFS does not accept a seed".into(),
            };
            return report;
        }
        let limits = match SearchLimits::parse(&plan.bounds, plan.timeout_ms.get()) {
            Ok(limits) => limits,
            Err(reason) => {
                report.verdict = Verdict::Error { reason };
                return report;
            }
        };
        let Some(harness) = self
            .harnesses
            .get(&(plan.model.clone(), plan.handle.clone()))
        else {
            report.verdict = Verdict::Error {
                reason: "unregistered Stateright model/handle".into(),
            };
            return report;
        };
        let result = harness.run(plan, limits);
        report.verdict = result.verdict;
        report.scenarios = result.scenarios;
        report
    }
}

#[derive(Clone, Copy)]
struct SearchLimits {
    max_depth: usize,
    max_states: usize,
    max_actions: usize,
    timeout: Duration,
}

impl SearchLimits {
    fn parse(bounds: &Bounds, timeout_ms: u64) -> Result<Self, String> {
        const EXPECTED: [&str; 3] = ["max_actions", "max_depth", "max_states"];
        if bounds.len() != EXPECTED.len() || EXPECTED.iter().any(|key| !bounds.contains_key(*key)) {
            return Err(
                "bounds must contain exactly max_depth, max_states, and max_actions".into(),
            );
        }
        let integer = |name: &str| {
            bounds[name]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("bound {name} must be a nonnegative platform-sized integer"))
        };
        let max_depth = integer("max_depth")?;
        let max_states = integer("max_states")?;
        let max_actions = integer("max_actions")?;
        if max_states == 0 || max_actions == 0 {
            return Err("max_states and max_actions must be positive".into());
        }
        Ok(Self {
            max_depth,
            max_states,
            max_actions,
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    fn reported(self) -> Bounds {
        BTreeMap::from([
            ("max_actions".into(), serde_json::json!(self.max_actions)),
            ("max_depth".into(), serde_json::json!(self.max_depth)),
            ("max_states".into(), serde_json::json!(self.max_states)),
            (
                "timeout_ms".into(),
                serde_json::json!(self.timeout.as_millis()),
            ),
        ])
    }
}

struct HarnessReport {
    verdict: Verdict,
    scenarios: Vec<serde_json::Value>,
}

struct RunControl<S> {
    started: Instant,
    timeout: Duration,
    max_states: usize,
    states: Mutex<HashSet<(S, usize)>>,
    exhausted_actions: Mutex<bool>,
    exhausted_states: Mutex<bool>,
    timed_out: Mutex<bool>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct DepthState<S> {
    inner: S,
    depth: usize,
}

struct BoundedModel<M: Model> {
    inner: M,
    property: fn(&M, &M::State) -> bool,
    max_depth: usize,
    max_actions: usize,
    control: Arc<RunControl<M::State>>,
}

impl<M: Model + Clone> Clone for BoundedModel<M> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            property: self.property,
            max_depth: self.max_depth,
            max_actions: self.max_actions,
            control: Arc::clone(&self.control),
        }
    }
}

impl<M> BoundedModel<M>
where
    M: Model,
    M::State: Clone + Hash + Eq,
{
    fn stopped(&self) -> bool {
        if self.control.started.elapsed() >= self.control.timeout {
            *self.control.timed_out.lock().expect("run control poisoned") = true;
            true
        } else {
            false
        }
    }

    fn selected_property(model: &Self, state: &DepthState<M::State>) -> bool {
        if model.stopped() {
            return true;
        }
        let holds = (model.property)(&model.inner, &state.inner);
        if model.stopped() { true } else { holds }
    }
}

impl<M> Model for BoundedModel<M>
where
    M: Model,
    M::State: Clone + Hash + Eq,
{
    type State = DepthState<M::State>;
    type Action = M::Action;

    fn init_states(&self) -> Vec<Self::State> {
        self.inner
            .init_states()
            .into_iter()
            .map(|inner| DepthState { inner, depth: 0 })
            .collect()
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        if state.depth >= self.max_depth || self.stopped() {
            return;
        }
        self.inner.actions(&state.inner, actions);
        if self.stopped() {
            actions.clear();
            return;
        }
        if actions.len() > self.max_actions {
            *self
                .control
                .exhausted_actions
                .lock()
                .expect("run control poisoned") = true;
        }
        actions.truncate(self.max_actions);
    }

    fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
        let next = self.inner.next_state(&state.inner, action);
        if self.stopped() {
            return None;
        }
        next.map(|inner| DepthState {
            inner,
            depth: state.depth + 1,
        })
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![Property::always(PROPERTY_NAME, Self::selected_property)]
    }

    fn within_boundary(&self, state: &Self::State) -> bool {
        if state.depth > self.max_depth || self.stopped() {
            return false;
        }
        let inner_boundary = self.inner.within_boundary(&state.inner);
        if self.stopped() || !inner_boundary {
            return false;
        }
        let mut states = self.control.states.lock().expect("run control poisoned");
        if states.contains(&(state.inner.clone(), state.depth)) {
            return true;
        }
        if states.len() >= self.control.max_states {
            *self
                .control
                .exhausted_states
                .lock()
                .expect("run control poisoned") = true;
            return false;
        }
        states.insert((state.inner.clone(), state.depth));
        true
    }
}

impl<M, R, P> Harness for BfsHarness<M, R, P>
where
    M: Model + Clone + Send + Sync + 'static,
    M::State: Clone + Hash + Send + Sync + Eq + 'static,
    M::Action: Clone + Send + Sync + PartialEq + 'static,
    R: Fn(&InvariantId) -> Option<&'static str> + Send + Sync,
    P: Fn(&M::Action) -> Result<serde_json::Value, String> + Send + Sync,
{
    fn run(&self, plan: &CheckPlan, limits: SearchLimits) -> HarnessReport {
        let control = Arc::new(RunControl {
            started: Instant::now(),
            timeout: limits.timeout,
            max_states: limits.max_states,
            states: Mutex::new(HashSet::new()),
            exhausted_actions: Mutex::new(false),
            exhausted_states: Mutex::new(false),
            timed_out: Mutex::new(false),
        });
        let Some(property) = (self.property)(&plan.invariant) else {
            return HarnessReport {
                verdict: Verdict::Error {
                    reason: "harness does not define the requested invariant".into(),
                },
                scenarios: Vec::new(),
            };
        };
        if control.started.elapsed() >= control.timeout {
            return incomplete(limits, "Stateright deadline exhausted resolving property");
        }
        if property != plan.handle {
            return HarnessReport {
                verdict: Verdict::Error {
                    reason: "requested handle does not match the invariant property".into(),
                },
                scenarios: Vec::new(),
            };
        }
        let properties = self.model.properties();
        if control.started.elapsed() >= control.timeout {
            return incomplete(limits, "Stateright deadline exhausted loading properties");
        }
        let Some(property) = properties.into_iter().find(|candidate| {
            candidate.name == property && candidate.expectation == stateright::Expectation::Always
        }) else {
            return HarnessReport {
                verdict: Verdict::Error {
                    reason: "harness must resolve a declared Stateright always property".into(),
                },
                scenarios: Vec::new(),
            };
        };
        let initial_states = self.model.init_states();
        if control.started.elapsed() >= control.timeout {
            return incomplete(
                limits,
                "Stateright deadline exhausted loading initial state",
            );
        }
        let initial_in_boundary =
            initial_states.len() == 1 && self.model.within_boundary(&initial_states[0]);
        if control.started.elapsed() >= control.timeout {
            return incomplete(
                limits,
                "Stateright deadline exhausted checking initial state",
            );
        }
        if !initial_in_boundary {
            return HarnessReport {
                verdict: Verdict::Error {
                    reason: "scenario projection requires exactly one in-bound initial state"
                        .into(),
                },
                scenarios: Vec::new(),
            };
        }
        let model = BoundedModel {
            inner: self.model.clone(),
            property: property.condition,
            max_depth: limits.max_depth,
            max_actions: limits.max_actions,
            control: Arc::clone(&control),
        };
        // Stateright's own depth/state targets skip boundary observations and may
        // overshoot in 1500-state blocks. The wrapper supplies the actual bounds;
        // the pinned checker supplies the real single-threaded BFS traversal.
        let checker = model.checker().threads(1).spawn_bfs().join();
        if control.started.elapsed() >= control.timeout {
            return incomplete_with_counts(
                limits,
                "Stateright deadline exhausted",
                checker.unique_state_count(),
                checker.max_depth(),
            );
        }
        if let Some(path) = checker.discovery(PROPERTY_NAME) {
            let actions = path.into_actions();
            let mut steps = Vec::with_capacity(actions.len());
            for action in &actions {
                match (self.project_action)(action) {
                    Ok(step) => steps.push(step),
                    Err(reason) => {
                        if control.started.elapsed() >= control.timeout {
                            return incomplete_with_counts(
                                limits,
                                "Stateright deadline exhausted projecting counterexample",
                                checker.unique_state_count(),
                                checker.max_depth(),
                            );
                        }
                        return HarnessReport {
                            verdict: Verdict::Error {
                                reason: format!("counterexample projection failed: {reason}"),
                            },
                            scenarios: Vec::new(),
                        };
                    }
                }
                if control.started.elapsed() >= control.timeout {
                    return incomplete_with_counts(
                        limits,
                        "Stateright deadline exhausted projecting counterexample",
                        checker.unique_state_count(),
                        checker.max_depth(),
                    );
                }
            }
            let scenario = Scenario {
                schema: 1,
                project: self.metadata.project.clone(),
                model: plan.model.clone(),
                invariant: plan.invariant.clone(),
                origin: Origin {
                    kind: OriginKind::Stateright,
                    tool_version: Some(TOOL_VERSION.into()),
                    discovered_at: None,
                    seed: None,
                    reference: None,
                },
                bounds: limits
                    .reported()
                    .into_iter()
                    .filter_map(|(key, value)| {
                        value
                            .as_u64()
                            .map(|value| (key, verify_scenario::Bound::Unsigned(value)))
                    })
                    .collect(),
                initial: self.metadata.initial.clone(),
                steps,
                expect: Expectation::InvariantViolated,
                status: Status::Active,
                fingerprint: None,
            };
            let scenario = match scenario.validate().and_then(|scenario| {
                serde_json::to_value(scenario.scenario())
                    .map_err(verify_scenario::EnvelopeError::from)
            }) {
                Ok(scenario) => scenario,
                Err(error) => {
                    return HarnessReport {
                        verdict: Verdict::Error {
                            reason: format!("counterexample scenario is invalid: {error}"),
                        },
                        scenarios: Vec::new(),
                    };
                }
            };
            return HarnessReport {
                verdict: Verdict::Falsified {
                    reason: "Stateright found an invariant counterexample".into(),
                },
                scenarios: vec![scenario],
            };
        }
        let timed_out = *control.timed_out.lock().expect("run control poisoned");
        let exhausted_actions = *control
            .exhausted_actions
            .lock()
            .expect("run control poisoned");
        let exhausted_states = *control
            .exhausted_states
            .lock()
            .expect("run control poisoned");
        if timed_out || exhausted_actions || exhausted_states {
            let mut explored = limits.reported();
            explored.insert(
                "states".into(),
                serde_json::json!(checker.unique_state_count()),
            );
            explored.insert("depth".into(), serde_json::json!(checker.max_depth()));
            HarnessReport {
                verdict: Verdict::Incomplete {
                    reason: match (timed_out, exhausted_actions) {
                        (true, _) => "Stateright deadline exhausted",
                        (false, true) => "Stateright action budget exhausted",
                        (false, false) => "Stateright state budget exhausted",
                    }
                    .into(),
                    explored,
                },
                scenarios: Vec::new(),
            }
        } else {
            HarnessReport {
                verdict: Verdict::Bounded {
                    bounds: limits.reported(),
                },
                scenarios: Vec::new(),
            }
        }
    }
}

fn incomplete(limits: SearchLimits, reason: &str) -> HarnessReport {
    HarnessReport {
        verdict: Verdict::Incomplete {
            reason: reason.into(),
            explored: limits.reported(),
        },
        scenarios: Vec::new(),
    }
}

fn incomplete_with_counts(
    limits: SearchLimits,
    reason: &str,
    states: usize,
    depth: usize,
) -> HarnessReport {
    let mut report = incomplete(limits, reason);
    if let Verdict::Incomplete { explored, .. } = &mut report.verdict {
        explored.insert("states".into(), serde_json::json!(states));
        explored.insert("depth".into(), serde_json::json!(depth));
    }
    report
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, thread};

    use super::*;

    #[derive(Clone)]
    struct CounterModel {
        broken_at: Option<u8>,
        delay: Option<Delay>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Delay {
        Property,
        Actions,
        NextState,
    }

    impl Model for CounterModel {
        type State = u8;
        type Action = ();

        fn init_states(&self) -> Vec<Self::State> {
            vec![0]
        }

        fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
            if self.delay == Some(Delay::Actions) && *state == 4 {
                thread::sleep(Duration::from_millis(5));
            }
            if *state < 4 {
                actions.push(());
            }
        }

        fn next_state(&self, state: &Self::State, (): Self::Action) -> Option<Self::State> {
            if self.delay == Some(Delay::NextState) && *state == 0 {
                thread::sleep(Duration::from_millis(5));
            }
            Some(state + 1)
        }

        fn properties(&self) -> Vec<Property<Self>> {
            vec![Property::always("counter_safe", |model, state| {
                if model.delay == Some(Delay::Property) {
                    thread::sleep(Duration::from_millis(5));
                }
                model.broken_at != Some(*state)
            })]
        }
    }

    fn invariant() -> InvariantId {
        InvariantId::try_from("TEST-SAFE-001".to_owned()).unwrap()
    }

    fn plan(bounds: Bounds) -> CheckPlan {
        CheckPlan {
            invariant: invariant(),
            model: "counter".into(),
            handle: "counter_safe".into(),
            bounds,
            seed: None,
            timeout_ms: NonZeroU64::new(1_000).unwrap(),
        }
    }

    fn bounds(depth: usize, states: usize) -> Bounds {
        BTreeMap::from([
            ("max_actions".into(), serde_json::json!(1)),
            ("max_depth".into(), serde_json::json!(depth)),
            ("max_states".into(), serde_json::json!(states)),
        ])
    }

    fn backend(broken_at: Option<u8>) -> StaterightBackend {
        backend_with_model(CounterModel {
            broken_at,
            delay: None,
        })
    }

    fn backend_with_model(model: CounterModel) -> StaterightBackend {
        let mut backend = StaterightBackend::new();
        backend
            .register(
                "counter",
                "counter_safe",
                BfsHarness::new(
                    model,
                    ScenarioMetadata {
                        project: "adapter-test".into(),
                        initial: serde_json::Map::new(),
                    },
                    |id: &InvariantId| (id.as_str() == "TEST-SAFE-001").then_some("counter_safe"),
                    |_: &()| Ok(serde_json::json!({ "action": "increment" })),
                ),
            )
            .unwrap();
        backend
    }

    #[test]
    fn declares_only_bounded_safety_and_security_without_fairness() {
        let capabilities = backend(None).capabilities();
        assert_eq!(
            capabilities.kinds,
            BTreeSet::from([Kind::Safety, Kind::Security])
        );
        assert!(capabilities.bounded);
        assert!(!capabilities.fairness);
        assert!(!capabilities.concurrency);
    }

    #[test]
    fn broken_model_projects_shortest_valid_counterexample() {
        let report = backend(Some(2)).run(&plan(bounds(2, 10)));
        assert!(matches!(report.verdict, Verdict::Falsified { .. }));
        assert_eq!(report.scenarios.len(), 1);
        let scenario: Scenario = serde_json::from_value(report.scenarios[0].clone()).unwrap();
        assert_eq!(scenario.expect, Expectation::InvariantViolated);
        assert_eq!(scenario.steps.len(), 2);
        assert!(scenario.validate().is_ok());
    }

    #[test]
    fn requested_depth_is_inclusive_for_property_observation() {
        let report = backend(Some(2)).run(&plan(bounds(1, 10)));
        assert!(matches!(report.verdict, Verdict::Bounded { .. }));
        let report = backend(Some(2)).run(&plan(bounds(2, 10)));
        assert!(matches!(report.verdict, Verdict::Falsified { .. }));
    }

    #[test]
    fn fixed_finite_search_is_bounded_not_universal() {
        let report = backend(None).run(&plan(bounds(4, 10)));
        assert!(matches!(report.verdict, Verdict::Bounded { .. }));
        assert!(report.scenarios.is_empty());
    }

    #[test]
    fn state_exhaustion_is_incomplete() {
        let report = backend(None).run(&plan(bounds(4, 2)));
        assert!(matches!(report.verdict, Verdict::Incomplete { .. }));
    }

    #[test]
    fn malformed_or_unknown_bounds_fail_closed() {
        let report = backend(None).run(&plan(BTreeMap::from([
            ("max_actions".into(), serde_json::json!(1)),
            ("max_depth".into(), serde_json::json!(2)),
        ])));
        assert!(matches!(report.verdict, Verdict::Error { .. }));

        let mut unknown = bounds(2, 10);
        unknown.insert("threads".into(), serde_json::json!(1));
        let report = backend(None).run(&plan(unknown));
        assert!(matches!(report.verdict, Verdict::Error { .. }));
    }

    #[test]
    fn unresolved_invariant_is_an_error() {
        let mut plan = plan(bounds(2, 10));
        plan.invariant = InvariantId::try_from("TEST-UNKNOWN-001".to_owned()).unwrap();
        let report = backend(None).run(&plan);
        assert!(matches!(report.verdict, Verdict::Error { .. }));
    }

    #[test]
    fn unsupported_seed_fails_closed() {
        let mut plan = plan(bounds(2, 10));
        plan.seed = Some(7);
        let report = backend(None).run(&plan);
        assert!(matches!(report.verdict, Verdict::Error { .. }));
    }

    #[test]
    fn cross_bound_handle_and_invariant_fail_closed() {
        let mut backend = StaterightBackend::new();
        backend
            .register(
                "counter",
                "unrelated_property",
                BfsHarness::new(
                    CounterModel {
                        broken_at: None,
                        delay: None,
                    },
                    ScenarioMetadata {
                        project: "adapter-test".into(),
                        initial: serde_json::Map::new(),
                    },
                    |id: &InvariantId| (id.as_str() == "TEST-SAFE-001").then_some("counter_safe"),
                    |_: &()| Ok(serde_json::json!({ "action": "increment" })),
                ),
            )
            .unwrap();
        let mut plan = plan(bounds(2, 10));
        plan.handle = "unrelated_property".into();
        assert!(matches!(backend.run(&plan).verdict, Verdict::Error { .. }));
    }

    #[test]
    fn slow_falsifying_property_is_incomplete_not_falsified() {
        let backend = backend_with_model(CounterModel {
            broken_at: Some(0),
            delay: Some(Delay::Property),
        });
        let mut plan = plan(bounds(2, 10));
        plan.timeout_ms = NonZeroU64::new(1).unwrap();
        assert!(matches!(
            backend.run(&plan).verdict,
            Verdict::Incomplete { .. }
        ));
    }

    #[test]
    fn slow_next_state_callback_is_incomplete() {
        let backend = backend_with_model(CounterModel {
            broken_at: None,
            delay: Some(Delay::NextState),
        });
        let mut plan = plan(bounds(2, 10));
        plan.timeout_ms = NonZeroU64::new(1).unwrap();
        assert!(matches!(
            backend.run(&plan).verdict,
            Verdict::Incomplete { .. }
        ));
    }

    #[test]
    fn slow_final_actions_callback_is_incomplete() {
        let backend = backend_with_model(CounterModel {
            broken_at: None,
            delay: Some(Delay::Actions),
        });
        let mut plan = plan(bounds(5, 10));
        plan.timeout_ms = NonZeroU64::new(1).unwrap();
        assert!(matches!(
            backend.run(&plan).verdict,
            Verdict::Incomplete { .. }
        ));
    }

    #[test]
    fn slow_counterexample_projection_is_incomplete() {
        let mut backend = StaterightBackend::new();
        backend
            .register(
                "counter",
                "counter_safe",
                BfsHarness::new(
                    CounterModel {
                        broken_at: Some(1),
                        delay: None,
                    },
                    ScenarioMetadata {
                        project: "adapter-test".into(),
                        initial: serde_json::Map::new(),
                    },
                    |id: &InvariantId| (id.as_str() == "TEST-SAFE-001").then_some("counter_safe"),
                    |_: &()| {
                        thread::sleep(Duration::from_millis(5));
                        Ok(serde_json::json!({ "action": "increment" }))
                    },
                ),
            )
            .unwrap();
        let mut plan = plan(bounds(2, 10));
        plan.timeout_ms = NonZeroU64::new(1).unwrap();
        assert!(matches!(
            backend.run(&plan).verdict,
            Verdict::Incomplete { .. }
        ));
    }
}
