//! Labby-specific T0 model replay host.

mod checking;
mod coverage;
/// Project-owned, redacted incident reduction.
pub mod incidents;
mod reporting;

/// Bounded Labby lifecycle harnesses for the isolated Stateright backend.
pub mod stateright;

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{Read, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use labby_model::{
    BrowserRequestModel, CAPABILITY_VISIBILITY_MODEL, CATALOG_TOML, CapabilityVisibilityModel,
    MODEL, MODELS,
};
use serde::{Deserialize, Serialize};
use verify_core::{BackendRegistry, Catalog, InvariantStatus};
use verify_runner::{NormalizationOptions, ReplayLimits, ReplayReport, TargetRegistry};
use verify_scenario::{Expectation, MAX_SCENARIO_BYTES, Scenario, Status, ValidatedScenario};

const MAX_CATALOG_BYTES: usize = 1_048_576;
const MAX_SCENARIO_FILES: usize = 256;
const TOTAL_DEADLINE: Duration = Duration::from_secs(55);
const SCENARIO_DEADLINE: Duration = Duration::from_secs(5);
const NORMALIZATION_REPLAYS: usize = 16;
const TOTAL_REPLAY_ATTEMPTS: u32 = 18;

/// Run the project verification CLI. Arguments omit argv[0].
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some(command) if command == OsStr::new("incident") => {
            incidents::run(args, false, output, errors)
        }
        Some(command) if command == OsStr::new("incident-explore") => {
            incidents::run(args, true, output, errors)
        }
        Some(command) if command == OsStr::new("t1") => checking::run(args, output, errors),
        Some(command) if command == OsStr::new("report-t0") => {
            reporting::run(args, false, output, errors)
        }
        Some(command) if command == OsStr::new("report-t1") => {
            reporting::run(args, true, output, errors)
        }
        Some(command) if command == OsStr::new("replay") => replay(
            std::iter::once(OsString::from("replay")).chain(args),
            output,
            errors,
        ),
        Some(command) if command == OsStr::new("t0") => {
            let Some(root) = args.next() else {
                let _ = writeln!(errors, "usage: labby-verify t0 <formal-directory>");
                return 2;
            };
            if args.next().is_some() {
                let _ = writeln!(errors, "usage: labby-verify t0 <formal-directory>");
                return 2;
            }
            t0(Path::new(&root), output, errors)
        }
        _ => {
            let _ = writeln!(
                errors,
                "usage: labby-verify <t0|t1> <formal-directory>\n       labby-verify replay <scenario.json>...\n       labby-verify <incident|incident-explore> <lifecycle-json> <invariant>\n       labby-verify <report-t0|report-t1> <input.json> <revision-file> <dirty-file> <sha256-file> <json|text|markdown|html> <source-name>"
            );
            2
        }
    }
}

fn embedded_registry() -> Result<TargetRegistry<'static>, String> {
    let backend = stateright::backend()?;
    let kani = kani_backend()?;
    let mut backends = BackendRegistry::default();
    backends
        .register(&backend)
        .map_err(|error| error.to_string())?;
    backends
        .register(&kani)
        .map_err(|error| error.to_string())?;
    let catalog = Catalog::from_toml(CATALOG_TOML, &backends)
        .map_err(|error| format!("embedded catalog is invalid: {error}"))?;
    let mut registry = TargetRegistry::default();
    registry
        .register(&catalog, MODEL, BrowserRequestModel)
        .map_err(|error| format!("cannot register browser request model: {error}"))?;
    registry
        .register(
            &catalog,
            CAPABILITY_VISIBILITY_MODEL,
            CapabilityVisibilityModel,
        )
        .map_err(|error| format!("cannot register capability visibility model: {error}"))?;
    Ok(registry)
}

fn kani_backend() -> Result<verify_kani::KaniBackend, String> {
    let mut backend = verify_kani::KaniBackend::new(
        std::env::var_os("LABBY_KANI_DRIVER").unwrap_or_else(|| "kani-driver".into()),
    );
    backend.register(
        MODEL,
        "request_single_terminal",
        verify_kani::KaniHarness::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../formal/kani/browser_request.rs"),
            "request_single_terminal",
        )?,
    )?;
    Ok(backend)
}

fn replay(
    args: impl IntoIterator<Item = OsString>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    match embedded_registry() {
        Ok(registry) => verify_runner::run_cli(&registry, args, output, errors),
        Err(error) => {
            let _ = writeln!(errors, "{error}");
            2
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogIdentity {
    project: String,
    models: Vec<String>,
    fingerprint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenCoverage {
    invariant: String,
    active_holds_traces: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioReport {
    path: String,
    invariant: String,
    expectation: Expectation,
    status: Status,
    source_fingerprint: String,
    canonical_fingerprint: String,
    normalization: NormalizationReport,
    replay: ReplayReport,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NormalizationReport {
    status: String,
    replays: usize,
    budget_exhausted: bool,
    discarded_transformation: bool,
    error: Option<String>,
}

struct NormalizedForT0 {
    scenario: ValidatedScenario,
    report: NormalizationReport,
    problem: bool,
}

fn normalize_for_t0(
    registry: &TargetRegistry<'_>,
    scenario: &ValidatedScenario,
    replay_limits: ReplayLimits,
) -> NormalizedForT0 {
    let raw = scenario.scenario();
    match registry.normalize(
        scenario,
        &NormalizationOptions {
            determinism_runs: NonZeroUsize::new(3).expect("positive constant"),
            max_replays: NonZeroUsize::new(NORMALIZATION_REPLAYS).expect("positive constant"),
            replay: replay_limits,
        },
    ) {
        Ok(result)
            if raw.expect != Expectation::InvariantHolds
                || result.scenario.scenario().steps.len() == raw.steps.len() =>
        {
            let problem = result.budget_exhausted
                || result.discarded_transformation
                || result.scenario.scenario().status != raw.status;
            NormalizedForT0 {
                scenario: result.scenario,
                report: NormalizationReport {
                    status: if problem { "guarded" } else { "normalized" }.into(),
                    replays: result.replays,
                    budget_exhausted: result.budget_exhausted,
                    discarded_transformation: result.discarded_transformation,
                    error: None,
                },
                problem,
            }
        }
        Ok(result) => NormalizedForT0 {
            scenario: scenario.clone(),
            report: NormalizationReport {
                status: "guarded".into(),
                replays: result.replays,
                budget_exhausted: result.budget_exhausted,
                discarded_transformation: true,
                error: Some("golden normalization changed the trace length".into()),
            },
            problem: true,
        },
        Err(error) => NormalizedForT0 {
            scenario: scenario.clone(),
            report: NormalizationReport {
                status: "error".into(),
                replays: 0,
                budget_exhausted: false,
                discarded_transformation: false,
                error: Some(error.to_string()),
            },
            problem: true,
        },
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct T0Report {
    schema: u32,
    lane: String,
    catalog: CatalogIdentity,
    reports: Vec<ScenarioReport>,
    uncovered_backend_ids: Vec<String>,
    golden_coverage: Vec<GoldenCoverage>,
    universal_proof: bool,
    gate_failure: bool,
}

fn t0(root: &Path, output: &mut impl Write, errors: &mut impl Write) -> i32 {
    let started = Instant::now();
    match build_t0_report(root, started) {
        Ok((report, gate_failure)) => {
            if serde_json::to_writer(&mut *output, &report).is_err() || writeln!(output).is_err() {
                return 2;
            }
            i32::from(gate_failure)
        }
        Err(error) => {
            let _ = writeln!(errors, "T0 failed closed: {error}");
            2
        }
    }
}

fn build_t0_report(root: &Path, started: Instant) -> Result<(T0Report, bool), String> {
    let deadline = started + TOTAL_DEADLINE;
    require_directory(root)?;
    let catalog_path = root.join("invariants.toml");
    let catalog_text = read_bounded_regular(&catalog_path, MAX_CATALOG_BYTES)?;
    let backend = stateright::backend()?;
    let kani = kani_backend()?;
    let mut backends = BackendRegistry::default();
    backends
        .register(&backend)
        .map_err(|error| error.to_string())?;
    backends
        .register(&kani)
        .map_err(|error| error.to_string())?;
    let catalog = Catalog::from_toml(&catalog_text, &backends)
        .map_err(|error| format!("invalid catalog {}: {error}", catalog_path.display()))?;
    let embedded = Catalog::from_toml(CATALOG_TOML, &backends)
        .map_err(|error| format!("embedded catalog is invalid: {error}"))?;
    if serde_json::to_value(catalog.catalog()).map_err(|error| error.to_string())?
        != serde_json::to_value(embedded.catalog()).map_err(|error| error.to_string())?
    {
        return Err("runtime catalog differs from the catalog embedded in this binary".into());
    }
    for model in MODELS {
        if catalog
            .catalog()
            .invariant
            .iter()
            .all(|item| item.model != *model)
        {
            return Err(format!("catalog does not contain model {model}"));
        }
    }
    let mut registry = TargetRegistry::default();
    registry
        .register(&catalog, MODEL, BrowserRequestModel)
        .map_err(|error| format!("cannot register browser request model: {error}"))?;
    registry
        .register(
            &catalog,
            CAPABILITY_VISIBILITY_MODEL,
            CapabilityVisibilityModel,
        )
        .map_err(|error| format!("cannot register capability visibility model: {error}"))?;

    let scenarios = root.join("scenarios");
    require_directory(&scenarios)?;
    validate_scenario_model_directories(&scenarios)?;
    let mut paths = Vec::new();
    for model in MODELS {
        paths.extend(scenario_paths(&scenarios.join(model))?);
    }
    paths.sort();
    if paths.is_empty() {
        return Err(format!("scenario corpus is empty: {}", scenarios.display()));
    }

    let mut coverage = BTreeMap::<String, usize>::new();
    let mut reports = Vec::with_capacity(paths.len());
    let mut gate_failure = false;
    let catalog_models: BTreeMap<_, _> = catalog
        .catalog()
        .invariant
        .iter()
        .map(|item| (item.id.as_str(), item.model.as_str()))
        .collect();
    for path in paths {
        if started.elapsed() >= TOTAL_DEADLINE {
            return Err("total T0 deadline exceeded".into());
        }
        let input = read_bounded_regular(&path, MAX_SCENARIO_BYTES)?;
        let scenario = Scenario::from_json(&input)
            .map_err(|error| format!("invalid scenario {}: {error}", path.display()))?;
        let raw = scenario.scenario();
        if raw.project != catalog.catalog().project
            || !MODELS.contains(&raw.model.as_str())
            || catalog_models.get(raw.invariant.as_str()).copied() != Some(raw.model.as_str())
        {
            return Err(format!(
                "scenario {} has an unknown or mismatched project, model, or invariant",
                path.display()
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("total T0 deadline exceeded".into());
        }
        // One source replay, bounded normalization, and one canonical replay
        // canonical replay share the remaining total budget.
        let per_attempt = (remaining / TOTAL_REPLAY_ATTEMPTS).max(Duration::from_nanos(1));
        let replay_limits = ReplayLimits {
            max_steps: verify_scenario::MAX_STEPS,
            timeout: per_attempt.min(SCENARIO_DEADLINE),
        };
        let source_replay = registry.replay(&scenario, &replay_limits);
        let normalized = normalize_for_t0(&registry, &scenario, replay_limits.clone());
        let canonical = normalized.scenario;
        let normalization_report = normalized.report;
        let normalization_problem = normalized.problem;
        if Instant::now() >= deadline {
            return Err("total T0 deadline exceeded during normalization".into());
        }
        let replay = registry.replay(&canonical, &replay_limits);
        if scenario.scenario().status == Status::Active
            && scenario.scenario().expect == Expectation::InvariantHolds
            && !normalization_problem
            && !replay.gate_failure
            && coverage::has_semantic_witness(&canonical, deadline)?
        {
            *coverage
                .entry(scenario.scenario().invariant.to_string())
                .or_default() += 1;
        }
        gate_failure |= source_replay.gate_failure
            || replay.gate_failure
            || (raw.status == Status::Active && normalization_problem);
        reports.push(ScenarioReport {
            path: relative_display(root, &path),
            invariant: scenario.scenario().invariant.to_string(),
            expectation: scenario.scenario().expect,
            status: scenario.scenario().status,
            source_fingerprint: scenario.fingerprint(),
            canonical_fingerprint: canonical.fingerprint(),
            normalization: normalization_report,
            replay,
        });
    }

    let active: Vec<_> = catalog
        .catalog()
        .invariant
        .iter()
        .filter(|item| item.status == InvariantStatus::Active)
        .collect();
    if active.is_empty() {
        return Err("catalog has no active invariants for T0".into());
    }
    let golden_coverage: Vec<_> = active
        .iter()
        .map(|item| GoldenCoverage {
            invariant: item.id.to_string(),
            active_holds_traces: coverage.get(item.id.as_str()).copied().unwrap_or_default(),
        })
        .collect();
    gate_failure |= golden_coverage
        .iter()
        .any(|item| item.active_holds_traces == 0);

    let uncovered_backend_ids = active
        .iter()
        .filter(|item| item.checks.is_empty())
        .map(|item| item.id.to_string())
        .collect();
    let catalog_fingerprint = format!("b3:{}", blake3::hash(catalog_text.as_bytes()).to_hex());
    Ok((
        T0Report {
            schema: 2,
            lane: "model_replay".into(),
            catalog: CatalogIdentity {
                project: catalog.catalog().project.clone(),
                models: MODELS.iter().map(|model| (*model).to_owned()).collect(),
                fingerprint: catalog_fingerprint,
            },
            reports,
            uncovered_backend_ids,
            golden_coverage,
            universal_proof: false,
            gate_failure,
        },
        gate_failure,
    ))
}

fn validate_scenario_model_directories(root: &Path) -> Result<(), String> {
    for entry in
        fs::read_dir(root).map_err(|error| format!("cannot list {}: {error}", root.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot read {} entry: {error}", root.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if !file_type.is_dir() {
            return Err(format!(
                "scenario root may contain only model directories: {}",
                entry.path().display()
            ));
        }
        let Some(model) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(format!(
                "scenario model directory is not valid UTF-8: {}",
                entry.path().display()
            ));
        };
        if !MODELS.contains(&model.as_str()) {
            return Err(format!(
                "unknown scenario model directory `{model}` under {}",
                root.display()
            ));
        }
    }
    Ok(())
}

fn scenario_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    require_directory(root)?;
    let mut paths = Vec::new();
    for entry in
        fs::read_dir(root).map_err(|error| format!("cannot list {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("cannot list {}: {error}", root.display()))?;
        let path = entry.path();
        let kind = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
            .file_type();
        if kind.is_symlink() || !kind.is_file() {
            return Err(format!(
                "scenario path is not a regular file: {}",
                path.display()
            ));
        }
        if path.extension() != Some(OsStr::new("json")) {
            return Err(format!("unexpected scenario file: {}", path.display()));
        }
        paths.push(path);
        if paths.len() > MAX_SCENARIO_FILES {
            return Err(format!(
                "scenario corpus exceeds {MAX_SCENARIO_FILES} files"
            ));
        }
    }
    paths.sort();
    Ok(paths)
}

fn require_directory(path: &Path) -> Result<(), String> {
    let kind = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
        .file_type();
    if kind.is_symlink() || !kind.is_dir() {
        return Err(format!(
            "path is not an ordinary directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn read_bounded_regular(path: &Path, limit: usize) -> Result<String, String> {
    let before = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err(format!("path is not a regular file: {}", path.display()));
    }
    if before.len() > limit as u64 {
        return Err(format!("input exceeds {limit} bytes: {}", path.display()));
    }
    let file =
        File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let after = file
        .metadata()
        .map_err(|error| format!("cannot inspect open {}: {error}", path.display()))?;
    if !after.is_file() || after.len() > limit as u64 {
        return Err(format!(
            "open path is not a bounded regular file: {}",
            path.display()
        ));
    }
    let mut input = String::new();
    file.take(limit as u64 + 1)
        .read_to_string(&mut input)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if input.len() > limit {
        return Err(format!("input exceeds {limit} bytes: {}", path.display()));
    }
    Ok(input)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use verify_core::{
        BackendRegistry, Catalog, InvariantId, InvariantResult, ScenarioError, ScenarioTarget,
        StepOutcome,
    };
    use verify_runner::{ReplayLimits, TargetRegistry};
    use verify_scenario::Scenario;

    use super::normalize_for_t0;

    #[derive(Clone, Debug)]
    struct BrokenCounter {
        truncate_golden: bool,
    }

    impl ScenarioTarget for BrokenCounter {
        type State = bool;
        type Step = u8;

        fn init(&self, _initial: &Value) -> Result<Self::State, ScenarioError> {
            Ok(false)
        }

        fn apply(
            &self,
            state: &mut Self::State,
            step: &Self::Step,
        ) -> Result<StepOutcome, ScenarioError> {
            *state |= *step == 1;
            Ok(StepOutcome::Applied {})
        }

        fn check(
            &self,
            _id: &InvariantId,
            state: &Self::State,
        ) -> Result<InvariantResult, ScenarioError> {
            Ok(if *state {
                InvariantResult::Violated {
                    reason: "deliberately broken test target".into(),
                }
            } else {
                InvariantResult::Holds {}
            })
        }

        fn canonicalize(
            &self,
            initial: &Value,
            steps: &[Self::Step],
        ) -> Result<(Value, Vec<Self::Step>), ScenarioError> {
            let mut steps = steps.to_vec();
            if self.truncate_golden {
                steps.pop();
            }
            Ok((initial.clone(), steps))
        }
    }

    fn registry(target: BrokenCounter) -> TargetRegistry<'static> {
        let catalog = Catalog::from_json(
            &json!({
                "schema": 1,
                "project": "fixture",
                "namespace": "FIX",
                "invariant": [{
                    "id": "FIX-REQ-001",
                    "title": "fixture property",
                    "kind": "safety",
                    "severity": "high",
                    "model": "broken_counter"
                }]
            })
            .to_string(),
            &BackendRegistry::default(),
        )
        .unwrap();
        let mut registry = TargetRegistry::default();
        registry
            .register(&catalog, "broken_counter", target)
            .unwrap();
        registry
    }

    fn scenario(steps: &[u8], expect: &str) -> verify_scenario::ValidatedScenario {
        Scenario::from_json(
            &json!({
                "schema": 1,
                "project": "fixture",
                "model": "broken_counter",
                "invariant": "FIX-REQ-001",
                "origin": {"kind": "manual"},
                "steps": steps,
                "expect": expect,
                "status": "active"
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn host_policy_accepts_a_completed_counterexample_shrink() {
        let registry = registry(BrokenCounter {
            truncate_golden: false,
        });
        let source = scenario(&[0, 1, 0], "invariant_violated");
        assert!(
            !registry
                .replay(&source, &ReplayLimits::default())
                .gate_failure
        );
        let normalized = normalize_for_t0(&registry, &source, ReplayLimits::default());
        assert!(!normalized.problem, "{:?}", normalized.report.error);
        assert!(
            normalized.scenario.scenario().steps.len() < source.scenario().steps.len(),
            "counterexample should be minimized"
        );
        assert!(
            !registry
                .replay(&normalized.scenario, &ReplayLimits::default())
                .gate_failure
        );
    }

    #[test]
    fn host_policy_still_guards_golden_length_changes() {
        let registry = registry(BrokenCounter {
            truncate_golden: true,
        });
        let source = scenario(&[0], "invariant_holds");
        let normalized = normalize_for_t0(&registry, &source, ReplayLimits::default());
        assert!(normalized.problem);
        assert_eq!(normalized.scenario.scenario().steps.len(), 1);
    }
}
