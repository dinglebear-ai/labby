use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use verify_core::{
    Availability, Backend, BackendId, BackendReport, Bounds, Capabilities, CheckPlan, Kind, Verdict,
};
use verify_scenario::{Expectation, OriginKind, Status, ValidatedScenario};

const LOOM_VERSION: &str = "0.7.2";
const SHUTTLE_VERSION: &str = "0.9.0";
const MAX_BRANCHES: usize = 10_000;
const MAX_SCHEDULES: usize = 1_000_000;
const MAX_STEPS: usize = 1_000_000;

/// Selected schedule engine and stable backend identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Exhaustive exploration of a finite Loom harness.
    Loom,
    /// Seeded sampling of a finite Shuttle harness.
    Shuttle,
}

impl Engine {
    fn id(self) -> &'static str {
        match self {
            Self::Loom => "loom",
            Self::Shuttle => "shuttle",
        }
    }

    fn version(self) -> &'static str {
        match self {
            Self::Loom => LOOM_VERSION,
            Self::Shuttle => SHUTTLE_VERSION,
        }
    }

    fn origin(self) -> OriginKind {
        match self {
            Self::Loom => OriginKind::Loom,
            Self::Shuttle => OriginKind::Shuttle,
        }
    }
}

/// Validated limits for one Loom exploration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoomLimits {
    max_branches: usize,
    max_permutations: usize,
    timeout_ms: NonZeroU64,
}

impl LoomLimits {
    /// Validate explicit branch, permutation, and deadline limits.
    pub fn new(
        max_branches: usize,
        max_permutations: usize,
        timeout_ms: NonZeroU64,
    ) -> Result<Self, String> {
        require_limit("max_branches", max_branches, MAX_BRANCHES)?;
        require_limit("max_permutations", max_permutations, MAX_SCHEDULES)?;
        Ok(Self {
            max_branches,
            max_permutations,
            timeout_ms,
        })
    }

    /// Maximum branches allowed within one modeled execution.
    pub fn max_branches(self) -> usize {
        self.max_branches
    }

    /// Maximum schedules the adapter may execute.
    pub fn max_permutations(self) -> usize {
        self.max_permutations
    }

    /// Wall-clock deadline for the exploration.
    pub fn timeout_ms(self) -> NonZeroU64 {
        self.timeout_ms
    }
}

/// Validated limits for one Shuttle exploration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShuttleLimits {
    max_iterations: usize,
    max_steps: usize,
    timeout_ms: NonZeroU64,
}

impl ShuttleLimits {
    /// Validate explicit iteration, step, and deadline limits.
    pub fn new(
        max_iterations: usize,
        max_steps: usize,
        timeout_ms: NonZeroU64,
    ) -> Result<Self, String> {
        require_limit("max_iterations", max_iterations, MAX_SCHEDULES)?;
        require_limit("max_steps", max_steps, MAX_STEPS)?;
        Ok(Self {
            max_iterations,
            max_steps,
            timeout_ms,
        })
    }

    /// Number of seeded schedules to sample.
    pub fn max_iterations(self) -> usize {
        self.max_iterations
    }

    /// Maximum scheduling steps allowed within one iteration.
    pub fn max_steps(self) -> usize {
        self.max_steps
    }

    /// Wall-clock deadline for the exploration.
    pub fn timeout_ms(self) -> NonZeroU64 {
        self.timeout_ms
    }
}

fn require_limit(name: &str, value: usize, maximum: usize) -> Result<(), String> {
    if value == 0 || value > maximum {
        Err(format!("{name} must be between 1 and {maximum}"))
    } else {
        Ok(())
    }
}

/// Engine-specific, already validated execution limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineLimits {
    /// Loom exploration limits.
    Loom(LoomLimits),
    /// Shuttle exploration limits.
    Shuttle(ShuttleLimits),
}

impl EngineLimits {
    fn schedule_cap(self) -> usize {
        match self {
            Self::Loom(limits) => limits.max_permutations,
            Self::Shuttle(limits) => limits.max_iterations,
        }
    }

    fn reported(self, schedules: u64, seed: Option<u64>) -> Bounds {
        match self {
            Self::Loom(limits) => BTreeMap::from([
                (
                    "max_branches".into(),
                    serde_json::json!(limits.max_branches),
                ),
                (
                    "max_permutations".into(),
                    serde_json::json!(limits.max_permutations),
                ),
                ("schedules".into(), serde_json::json!(schedules)),
                (
                    "timeout_ms".into(),
                    serde_json::json!(limits.timeout_ms.get()),
                ),
            ]),
            Self::Shuttle(limits) => BTreeMap::from([
                (
                    "max_iterations".into(),
                    serde_json::json!(limits.max_iterations),
                ),
                ("max_steps".into(), serde_json::json!(limits.max_steps)),
                ("schedules".into(), serde_json::json!(schedules)),
                ("seed".into(), serde_json::json!(seed)),
                (
                    "timeout_ms".into(),
                    serde_json::json!(limits.timeout_ms.get()),
                ),
            ]),
        }
    }
}

/// Why a bounded exploration stopped before exhausting its declared domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The wall-clock deadline was reached.
    Deadline,
    /// Loom reached the per-execution branch limit.
    BranchLimit,
    /// Loom reached the requested permutation limit.
    PermutationLimit,
    /// Shuttle reached the per-iteration step limit.
    StepLimit,
}

impl StopReason {
    fn message(self) -> &'static str {
        match self {
            Self::Deadline => "concurrency exploration reached its deadline",
            Self::BranchLimit => "Loom reached its branch limit",
            Self::PermutationLimit => "Loom reached its permutation limit",
            Self::StepLimit => "Shuttle reached its scheduling-step limit",
        }
    }

    fn applies_to(self, engine: Engine) -> bool {
        matches!(
            (engine, self),
            (
                Engine::Loom,
                Self::Deadline | Self::BranchLimit | Self::PermutationLimit
            ) | (Engine::Shuttle, Self::Deadline | Self::StepLimit)
        )
    }
}

/// Infrastructure failures distinct from modeled property violations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// A project harness panicked without a typed counterexample.
    HarnessPanicked,
    /// The instrumented execution deadlocked.
    Deadlock,
    /// Ambient engine configuration could override the explicit plan.
    EnvironmentConflict,
}

impl EngineError {
    fn message(self) -> &'static str {
        match self {
            Self::HarnessPanicked => "instrumented concurrency harness panicked",
            Self::Deadlock => "instrumented concurrency execution deadlocked",
            Self::EnvironmentConflict => {
                "ambient concurrency engine configuration conflicts with the explicit plan"
            }
        }
    }
}

/// Typed result of running one bounded instrumented harness.
#[derive(Debug, PartialEq, Eq)]
pub enum EngineOutcome {
    /// The declared finite schedule domain was exhausted.
    Exhausted {
        /// Number of schedules actually completed.
        schedules: u64,
    },
    /// Exploration stopped at a declared limit.
    Incomplete {
        /// Number of schedules actually started.
        schedules: u64,
        /// Limit responsible for stopping exploration.
        reason: StopReason,
    },
    /// A concrete, validated invariant violation was found.
    Falsified {
        /// Number of schedules started through the failing execution.
        schedules: u64,
        /// Complete projectable counterexample envelopes.
        counterexamples: Vec<ValidatedScenario>,
    },
    /// The engine or harness failed independently of the property.
    Error {
        /// Stable, redacted error classification.
        reason: EngineError,
    },
}

impl EngineOutcome {
    fn schedules(&self) -> Option<u64> {
        match self {
            Self::Exhausted { schedules }
            | Self::Incomplete { schedules, .. }
            | Self::Falsified { schedules, .. } => Some(*schedules),
            Self::Error { .. } => None,
        }
    }
}

#[derive(Debug)]
struct DetectedCounterexample(ValidatedScenario);

/// Stop an instrumented schedule with a typed, already validated counterexample.
///
/// The enclosing [`run_loom`] or [`run_shuttle`] call catches this marker and
/// returns [`EngineOutcome::Falsified`].
pub fn raise_counterexample(scenario: ValidatedScenario) -> ! {
    std::panic::panic_any(DetectedCounterexample(scenario));
}

type Harness = dyn Fn(&CheckPlan, EngineLimits) -> EngineOutcome + Send + Sync;

/// Caller-populated adapter for one selected concurrency engine.
pub struct ConcurrencyBackend {
    engine: Engine,
    harnesses: BTreeMap<(String, String), Arc<Harness>>,
}

impl ConcurrencyBackend {
    /// Create an empty Loom adapter.
    pub fn loom() -> Self {
        Self::new(Engine::Loom)
    }

    /// Create an empty Shuttle adapter.
    pub fn shuttle() -> Self {
        Self::new(Engine::Shuttle)
    }

    fn new(engine: Engine) -> Self {
        Self {
            engine,
            harnesses: BTreeMap::new(),
        }
    }

    /// Register a project-owned harness without running it.
    pub fn register(
        &mut self,
        model: impl Into<String>,
        handle: impl Into<String>,
        harness: impl Fn(&CheckPlan, EngineLimits) -> EngineOutcome + Send + Sync + 'static,
    ) -> Result<(), String> {
        let key = (model.into(), handle.into());
        if key.0.trim().is_empty() || key.1.trim().is_empty() {
            return Err("model and handle must be nonempty".into());
        }
        match self.harnesses.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => Err(format!(
                "{} harness already registered: {}/{}",
                self.engine.id(),
                entry.key().0,
                entry.key().1
            )),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Arc::new(harness));
                Ok(())
            }
        }
    }
}

impl Backend for ConcurrencyBackend {
    fn id(&self) -> BackendId {
        BackendId::try_from(self.engine.id().to_owned()).expect("constant backend id")
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kinds: BTreeSet::from([Kind::Safety]),
            fairness: false,
            concurrency: true,
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
                reason: "invalid concurrency invocation".into(),
            },
            tool_version: Some(self.engine.version().into()),
            scenarios: Vec::new(),
        };
        let limits = match parse_limits(self.engine, plan) {
            Ok(value) => value,
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
                reason: "unregistered concurrency model/handle".into(),
            };
            return report;
        };

        let started = Instant::now();
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| harness(plan, limits)));
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(_) => EngineOutcome::Error {
                reason: EngineError::HarnessPanicked,
            },
        };
        let invalid_schedule_count = match &outcome {
            EngineOutcome::Exhausted { schedules } | EngineOutcome::Falsified { schedules, .. } => {
                *schedules == 0 || exceeds_schedule_cap(*schedules, limits)
            }
            EngineOutcome::Incomplete { schedules, .. } => exceeds_schedule_cap(*schedules, limits),
            EngineOutcome::Error { .. } => false,
        };
        let invalid_engine_outcome = match &outcome {
            EngineOutcome::Exhausted { schedules } => {
                self.engine == Engine::Shuttle
                    && usize::try_from(*schedules).ok() != Some(limits.schedule_cap())
            }
            EngineOutcome::Incomplete { reason, .. } => !reason.applies_to(self.engine),
            EngineOutcome::Falsified { .. } | EngineOutcome::Error { .. } => false,
        };
        if invalid_schedule_count || invalid_engine_outcome {
            report.verdict = Verdict::Error {
                reason: "concurrency harness reported an invalid engine outcome".into(),
            };
            return report;
        }
        if started.elapsed() >= Duration::from_millis(plan.timeout_ms.get())
            && matches!(
                &outcome,
                EngineOutcome::Exhausted { .. } | EngineOutcome::Incomplete { .. }
            )
        {
            let schedules = outcome.schedules().unwrap_or(0);
            report.verdict = Verdict::Incomplete {
                reason: StopReason::Deadline.message().into(),
                explored: limits.reported(schedules, plan.seed),
            };
            return report;
        }

        match outcome {
            EngineOutcome::Exhausted { schedules } => {
                report.verdict = Verdict::Bounded {
                    bounds: limits.reported(schedules, plan.seed),
                };
            }
            EngineOutcome::Incomplete { schedules, reason } => {
                report.verdict = Verdict::Incomplete {
                    reason: reason.message().into(),
                    explored: limits.reported(schedules, plan.seed),
                };
            }
            EngineOutcome::Falsified {
                schedules: _,
                counterexamples,
            } => match project_scenarios(self.engine, plan, counterexamples) {
                Ok(scenarios) if !scenarios.is_empty() => {
                    report.verdict = Verdict::Falsified {
                        reason: "instrumented lifecycle invariant violated".into(),
                    };
                    report.scenarios = scenarios;
                }
                Ok(_) | Err(_) => {
                    report.verdict = Verdict::Error {
                        reason: "concurrency harness projected an invalid counterexample".into(),
                    };
                }
            },
            EngineOutcome::Error { reason } => {
                report.verdict = Verdict::Error {
                    reason: reason.message().into(),
                };
            }
        }
        report
    }
}

fn exceeds_schedule_cap(schedules: u64, limits: EngineLimits) -> bool {
    usize::try_from(schedules)
        .map(|count| count > limits.schedule_cap())
        .unwrap_or(true)
}

fn parse_limits(engine: Engine, plan: &CheckPlan) -> Result<EngineLimits, String> {
    match engine {
        Engine::Loom => {
            require_exact_keys(&plan.bounds, &["max_branches", "max_permutations"])?;
            if plan.seed.is_some() {
                return Err("Loom does not accept a seed".into());
            }
            LoomLimits::new(
                bound(&plan.bounds, "max_branches")?,
                bound(&plan.bounds, "max_permutations")?,
                plan.timeout_ms,
            )
            .map(EngineLimits::Loom)
        }
        Engine::Shuttle => {
            require_exact_keys(&plan.bounds, &["max_iterations", "max_steps"])?;
            if plan.seed.is_none() {
                return Err("Shuttle requires an explicit seed".into());
            }
            ShuttleLimits::new(
                bound(&plan.bounds, "max_iterations")?,
                bound(&plan.bounds, "max_steps")?,
                plan.timeout_ms,
            )
            .map(EngineLimits::Shuttle)
        }
    }
}

fn require_exact_keys(bounds: &Bounds, keys: &[&str]) -> Result<(), String> {
    if bounds.len() == keys.len() && keys.iter().all(|key| bounds.contains_key(*key)) {
        Ok(())
    } else {
        Err(format!(
            "bounds must contain exactly {}",
            keys.join(" and ")
        ))
    }
}

fn bound(bounds: &Bounds, key: &str) -> Result<usize, String> {
    bounds[key]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("bound {key} must be a positive platform-sized integer"))
}

fn project_scenarios(
    engine: Engine,
    plan: &CheckPlan,
    counterexamples: Vec<ValidatedScenario>,
) -> Result<Vec<serde_json::Value>, String> {
    counterexamples
        .into_iter()
        .map(|validated| {
            let scenario = validated.scenario();
            if scenario.model != plan.model
                || scenario.invariant != plan.invariant
                || scenario.origin.kind != engine.origin()
                || scenario.origin.tool_version.as_deref() != Some(engine.version())
                || scenario.origin.seed != plan.seed
                || scenario.expect != Expectation::InvariantViolated
                || scenario.status != Status::Active
            {
                return Err("counterexample identity or provenance mismatch".into());
            }
            serde_json::to_value(scenario)
                .map_err(|_| "failed to serialize validated concurrency scenario".into())
        })
        .collect()
}

/// Execute one finite Loom harness with explicit branch, permutation, and deadline caps.
pub fn run_loom(limits: LoomLimits, harness: impl Fn() + Sync + Send + 'static) -> EngineOutcome {
    if has_loom_environment_conflict() {
        return EngineOutcome::Error {
            reason: EngineError::EnvironmentConflict,
        };
    }

    let schedules = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&schedules);
    let timeout = Duration::from_millis(limits.timeout_ms.get());
    let mut builder = loom::model::Builder::new();
    builder.max_branches = limits.max_branches;
    builder.max_permutations = limits.max_permutations.checked_add(1);
    builder.checkpoint_interval = 1;
    builder.max_duration = Some(timeout);
    builder.preemption_bound = None;
    builder.checkpoint_file = None;
    builder.expect_explicit_explore = false;
    builder.location = false;
    builder.log = false;
    let started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        builder.check(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            harness();
        });
    }));
    classify_engine_result(
        Engine::Loom,
        result,
        schedules.load(Ordering::SeqCst),
        limits.max_permutations,
        started.elapsed() >= timeout,
    )
}

/// Execute exactly the requested number of seeded Shuttle schedules unless a limit stops it.
pub fn run_shuttle(
    limits: ShuttleLimits,
    seed: u64,
    harness: impl Fn() + Sync + Send + 'static,
) -> EngineOutcome {
    if has_shuttle_environment_conflict() {
        return EngineOutcome::Error {
            reason: EngineError::EnvironmentConflict,
        };
    }

    let schedules = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&schedules);
    let timeout = Duration::from_millis(limits.timeout_ms.get());
    let mut config = shuttle::Config::new();
    config.max_steps = shuttle::MaxSteps::FailAfter(limits.max_steps);
    config.max_time = Some(timeout);
    config.failure_persistence = shuttle::FailurePersistence::None;
    let scheduler = shuttle::scheduler::RandomScheduler::new_from_seed(seed, limits.max_iterations);
    let started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        shuttle::Runner::new(scheduler, config).run(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            harness();
        })
    }));
    classify_engine_result(
        Engine::Shuttle,
        result.map(|_| ()),
        schedules.load(Ordering::SeqCst),
        limits.max_iterations,
        started.elapsed() >= timeout,
    )
}

fn classify_engine_result(
    engine: Engine,
    result: Result<(), Box<dyn Any + Send>>,
    schedules: usize,
    schedule_cap: usize,
    deadline_reached: bool,
) -> EngineOutcome {
    let schedules = u64::try_from(schedules).unwrap_or(u64::MAX);
    match result {
        Err(payload) => classify_panic(engine, payload, schedules),
        Ok(()) if deadline_reached => EngineOutcome::Incomplete {
            schedules,
            reason: StopReason::Deadline,
        },
        Ok(()) if engine == Engine::Loom && schedules as usize >= schedule_cap => {
            EngineOutcome::Incomplete {
                schedules,
                reason: StopReason::PermutationLimit,
            }
        }
        Ok(()) if engine == Engine::Shuttle && (schedules as usize) < schedule_cap => {
            EngineOutcome::Incomplete {
                schedules,
                reason: StopReason::Deadline,
            }
        }
        Ok(()) => EngineOutcome::Exhausted { schedules },
    }
}

fn classify_panic(engine: Engine, payload: Box<dyn Any + Send>, schedules: u64) -> EngineOutcome {
    if payload.is::<DetectedCounterexample>() {
        let detected = payload
            .downcast::<DetectedCounterexample>()
            .expect("type checked above");
        return EngineOutcome::Falsified {
            schedules,
            counterexamples: vec![detected.0],
        };
    }
    let text = panic_text(payload.as_ref());
    if text.is_some_and(|message| message.contains("deadlock")) {
        EngineOutcome::Error {
            reason: EngineError::Deadlock,
        }
    } else if engine == Engine::Loom
        && text.is_some_and(|message| message.contains("maximum number of branches"))
    {
        EngineOutcome::Incomplete {
            schedules,
            reason: StopReason::BranchLimit,
        }
    } else if engine == Engine::Shuttle
        && text.is_some_and(|message| message.contains("exceeded max_steps bound"))
    {
        EngineOutcome::Incomplete {
            schedules,
            reason: StopReason::StepLimit,
        }
    } else {
        EngineOutcome::Error {
            reason: EngineError::HarnessPanicked,
        }
    }
}

fn panic_text(payload: &(dyn Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

fn has_loom_environment_conflict() -> bool {
    [
        "LOOM_CHECKPOINT_INTERVAL",
        "LOOM_MAX_BRANCHES",
        "LOOM_LOCATION",
        "LOOM_LOG",
        "LOOM_MAX_DURATION",
        "LOOM_MAX_PERMUTATIONS",
        "LOOM_MAX_PREEMPTIONS",
        "LOOM_CHECKPOINT_FILE",
    ]
    .into_iter()
    .any(|name| std::env::var_os(name).is_some())
}

fn has_shuttle_environment_conflict() -> bool {
    ["SHUTTLE_RANDOM_SEED", "SHUTTLE_ALWAYS_PERSIST_SEED"]
        .into_iter()
        .any(|name| std::env::var_os(name).is_some())
}
