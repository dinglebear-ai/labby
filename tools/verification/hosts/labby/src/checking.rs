//! Explicit bounded model checking, separate from T0 replay.

use std::{
    ffi::OsString,
    io::Write,
    num::NonZeroU64,
    path::Path,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use verify_core::{Backend, BackendRegistry, Catalog, CheckPlan, InvariantStatus, Verdict};

use crate::{CATALOG_TOML, MAX_CATALOG_BYTES, MODEL, read_bounded_regular, require_directory};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct T1Report {
    pub schema: u32,
    pub lane: String,
    pub catalog_fingerprint: String,
    pub model: String,
    pub reports: Vec<verify_core::BackendReport>,
    pub universal_proof: bool,
}

pub(crate) fn run(
    args: impl Iterator<Item = OsString>,
    output: &mut impl Write,
    errors: &mut impl Write,
) -> i32 {
    let args: Vec<_> = args.collect();
    if args.len() != 1 {
        let _ = writeln!(errors, "usage: labby-verify t1 <formal-directory>");
        return 2;
    }
    match check(Path::new(&args[0])) {
        Ok(report) => {
            let failed = report.reports.is_empty()
                || report
                    .reports
                    .iter()
                    .any(|item| !matches!(item.verdict, Verdict::Bounded { .. }));
            if serde_json::to_writer(&mut *output, &report).is_err() || writeln!(output).is_err() {
                return 2;
            }
            i32::from(failed)
        }
        Err(error) => {
            let _ = writeln!(errors, "T1 failed closed: {error}");
            2
        }
    }
}

fn check(root: &Path) -> Result<T1Report, String> {
    let deadline = Instant::now() + Duration::from_secs(280);
    require_directory(root)?;
    let input = read_bounded_regular(&root.join("invariants.toml"), MAX_CATALOG_BYTES)?;
    let backend = crate::stateright::backend()?;
    let kani = crate::kani_backend()?;
    let mut registry = BackendRegistry::default();
    registry
        .register(&backend)
        .map_err(|error| error.to_string())?;
    registry
        .register(&kani)
        .map_err(|error| error.to_string())?;
    let catalog = Catalog::from_toml(&input, &registry).map_err(|error| error.to_string())?;
    let embedded =
        Catalog::from_toml(CATALOG_TOML, &registry).map_err(|error| error.to_string())?;
    if serde_json::to_value(catalog.catalog()).map_err(|error| error.to_string())?
        != serde_json::to_value(embedded.catalog()).map_err(|error| error.to_string())?
    {
        return Err("runtime catalog differs from compiled catalog".into());
    }
    let mut reports = Vec::new();
    for invariant in &catalog.catalog().invariant {
        if invariant.status != InvariantStatus::Active {
            continue;
        }
        let handles = invariant
            .checks
            .get(&backend.id())
            .ok_or("active invariant lacks Stateright binding")?;
        for handle in &handles.0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("total T1 deadline exceeded".into());
            }
            let timeout_ms = remaining.min(Duration::from_secs(50)).as_millis() as u64;
            let plan = CheckPlan {
                invariant: invariant.id.clone(),
                model: invariant.model.clone(),
                handle: handle.clone(),
                bounds: [
                    ("max_depth".into(), serde_json::json!(12)),
                    ("max_states".into(), serde_json::json!(20_000)),
                    ("max_actions".into(), serde_json::json!(32)),
                ]
                .into(),
                seed: None,
                timeout_ms: NonZeroU64::new(timeout_ms).ok_or("T1 deadline exhausted")?,
            };
            let mut report = backend.run(&plan);
            qualify_projection(&mut report, &crate::embedded_registry()?, deadline);
            reports.push(report);
        }
    }
    if Instant::now() >= deadline {
        return Err("total T1 deadline exceeded".into());
    }
    Ok(T1Report {
        schema: 1,
        lane: "model_checking".into(),
        catalog_fingerprint: format!("b3:{}", blake3::hash(input.as_bytes()).to_hex()),
        model: MODEL.into(),
        reports,
        universal_proof: false,
    })
}

pub(crate) fn qualify_projection(
    report: &mut verify_core::BackendReport,
    registry: &verify_runner::TargetRegistry<'_>,
    deadline: Instant,
) {
    let outcome = (|| -> Result<(), String> {
        if matches!(report.verdict, Verdict::Falsified { .. }) && report.scenarios.is_empty() {
            return Err("Stateright falsification has no projected counterexample".into());
        }
        if !report.scenarios.is_empty() && !matches!(report.verdict, Verdict::Falsified { .. }) {
            return Err("Non-falsified Stateright result contains counterexamples".into());
        }
        for value in &report.scenarios {
            let scenario = verify_scenario::Scenario::from_json(&value.to_string())
                .map_err(|error| error.to_string())?;
            let data = scenario.scenario();
            if data.invariant != report.invariant
                || data.expect != verify_scenario::Expectation::InvariantViolated
                || data.status != verify_scenario::Status::Active
            {
                return Err("Projected counterexample identity or expectation mismatch".into());
            }
            let timeout = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(5));
            if timeout.is_zero() {
                return Err("Counterexample qualification deadline exceeded".into());
            }
            let replay = registry.replay(
                &scenario,
                &verify_runner::ReplayLimits {
                    max_steps: verify_scenario::MAX_STEPS,
                    timeout,
                },
            );
            if replay.gate_failure
                || replay.verdict != verify_runner::TraceVerdict::InvariantViolated
                || Instant::now() >= deadline
            {
                return Err(
                    "Projected counterexample did not reproduce against the registered model"
                        .into(),
                );
            }
        }
        Ok(())
    })();
    if let Err(reason) = outcome {
        report.verdict = Verdict::Error { reason };
        // Retain the raw projection for diagnosis, but never as qualified evidence.
        for value in &mut report.scenarios {
            if let Some(object) = value.as_object_mut() {
                object.insert("status".into(), serde_json::json!("unreproduced"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use verify_core::{InvariantId, InvariantResult, ScenarioError, ScenarioTarget, StepOutcome};
    struct Fixture(bool);
    impl ScenarioTarget for Fixture {
        type State = bool;
        type Step = bool;
        fn init(&self, _: &Value) -> Result<bool, ScenarioError> {
            Ok(false)
        }
        fn apply(&self, state: &mut bool, step: &bool) -> Result<StepOutcome, ScenarioError> {
            *state = *step && self.0;
            Ok(StepOutcome::Applied {})
        }
        fn check(&self, _: &InvariantId, state: &bool) -> Result<InvariantResult, ScenarioError> {
            Ok(if *state {
                InvariantResult::Violated {
                    reason: "test-only broken target".into(),
                }
            } else {
                InvariantResult::Holds {}
            })
        }
    }
    #[test]
    fn projection_requires_actual_reproduction_not_just_envelope_validity() {
        let catalog = Catalog::from_json(&json!({"schema":1,"project":"fixture","namespace":"FIX","invariant":[{"id":"FIX-REQ-001","title":"fixture","kind":"safety","severity":"high","model":"fixture"}]}).to_string(), &BackendRegistry::default()).unwrap();
        for broken in [false, true] {
            let mut registry = verify_runner::TargetRegistry::default();
            registry
                .register(&catalog, "fixture", Fixture(broken))
                .unwrap();
            let mut report = verify_core::BackendReport {
                backend: verify_core::BackendId::try_from("stateright".to_owned()).unwrap(),
                invariant: InvariantId::try_from("FIX-REQ-001".to_owned()).unwrap(),
                verdict: Verdict::Falsified {
                    reason: "candidate".into(),
                },
                tool_version: Some("0.31.0".into()),
                scenarios: vec![
                    json!({"schema":1,"project":"fixture","model":"fixture","invariant":"FIX-REQ-001","origin":{"kind":"stateright"},"steps":[true],"expect":"invariant_violated","status":"active"}),
                ],
            };
            qualify_projection(
                &mut report,
                &registry,
                Instant::now() + Duration::from_secs(1),
            );
            assert_eq!(matches!(report.verdict, Verdict::Falsified { .. }), broken);
            if !broken {
                assert_eq!(report.scenarios[0]["status"], "unreproduced");
            }
        }
    }
}
