//! Contract tests for bounded Loom and Shuttle execution.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use verify_core::{Backend, CheckPlan, InvariantId, Kind, Verdict};
use verify_loom::{
    ConcurrencyBackend, EngineError, EngineLimits, EngineOutcome, LoomLimits, ShuttleLimits,
    StopReason, run_loom, run_shuttle,
};
use verify_scenario::{Expectation, OriginKind, Scenario, Status};

fn loom_plan(timeout_ms: u64) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from("TEST-REQ-001".to_owned()).unwrap(),
        model: "request".into(),
        handle: "terminal".into(),
        bounds: BTreeMap::from([
            ("max_branches".into(), serde_json::json!(32)),
            ("max_permutations".into(), serde_json::json!(8)),
        ]),
        seed: None,
        timeout_ms: NonZeroU64::new(timeout_ms).unwrap(),
    }
}

fn shuttle_plan(timeout_ms: u64) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from("TEST-REQ-001".to_owned()).unwrap(),
        model: "request".into(),
        handle: "terminal".into(),
        bounds: BTreeMap::from([
            ("max_iterations".into(), serde_json::json!(8)),
            ("max_steps".into(), serde_json::json!(32)),
        ]),
        seed: Some(42),
        timeout_ms: NonZeroU64::new(timeout_ms).unwrap(),
    }
}

fn valid_counterexample(kind: OriginKind, seed: Option<u64>) -> verify_scenario::ValidatedScenario {
    let version = if kind == OriginKind::Loom {
        "0.7.2"
    } else {
        "0.9.0"
    };
    serde_json::from_value::<Scenario>(serde_json::json!({
        "schema": 1,
        "project": "fixture",
        "model": "request",
        "invariant": "TEST-REQ-001",
        "origin": {"kind": kind, "tool_version": version, "seed": seed},
        "bounds": {},
        "initial": {},
        "steps": [{"action": "complete"}, {"action": "cancel"}],
        "expect": "invariant_violated",
        "status": "active"
    }))
    .unwrap()
    .validate()
    .unwrap()
}

#[test]
fn registration_is_metadata_only_and_duplicate_safe() {
    let mut backend = ConcurrencyBackend::loom();
    backend
        .register("request", "terminal", |_, _| EngineOutcome::Exhausted {
            schedules: 3,
        })
        .unwrap();
    assert!(backend.has_handle("request", "terminal"));
    assert!(backend.capabilities().supports(Kind::Safety));
    assert!(!backend.capabilities().supports(Kind::Liveness));
    assert!(backend.capabilities().concurrency);
    assert!(backend.capabilities().bounded);
    assert!(
        backend
            .register("request", "terminal", |_, _| EngineOutcome::Exhausted {
                schedules: 1,
            })
            .is_err()
    );
    let report = backend.run(&loom_plan(1_000));
    let Verdict::Bounded { bounds } = report.verdict else {
        panic!("original registration was not preserved")
    };
    assert_eq!(bounds["schedules"], 3);
}

#[test]
fn engine_specific_bounds_and_seed_contracts_are_enforced() {
    let mut loom = ConcurrencyBackend::loom();
    loom.register("request", "terminal", |_, limits| {
        assert!(matches!(limits, EngineLimits::Loom(_)));
        EngineOutcome::Exhausted { schedules: 3 }
    })
    .unwrap();
    let report = loom.run(&loom_plan(1_000));
    let Verdict::Bounded { bounds } = report.verdict else {
        panic!("expected bounded Loom report")
    };
    assert_eq!(
        bounds.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "max_branches",
            "max_permutations",
            "schedules",
            "timeout_ms"
        ]
    );
    let mut wrong = loom_plan(1_000);
    wrong.seed = Some(1);
    assert!(matches!(loom.run(&wrong).verdict, Verdict::Error { .. }));
    wrong = loom_plan(1_000);
    wrong
        .bounds
        .insert("max_steps".into(), serde_json::json!(1));
    assert!(matches!(loom.run(&wrong).verdict, Verdict::Error { .. }));

    let mut shuttle = ConcurrencyBackend::shuttle();
    shuttle
        .register("request", "terminal", |_, limits| {
            assert!(matches!(limits, EngineLimits::Shuttle(_)));
            EngineOutcome::Exhausted { schedules: 8 }
        })
        .unwrap();
    let report = shuttle.run(&shuttle_plan(1_000));
    let Verdict::Bounded { bounds } = report.verdict else {
        panic!("expected bounded Shuttle report")
    };
    assert_eq!(
        bounds.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "max_iterations",
            "max_steps",
            "schedules",
            "seed",
            "timeout_ms"
        ]
    );
    let mut missing_seed = shuttle_plan(1_000);
    missing_seed.seed = None;
    assert!(matches!(
        shuttle.run(&missing_seed).verdict,
        Verdict::Error { .. }
    ));
}

#[test]
fn projected_scenarios_must_match_engine_plan_and_violation_contract() {
    let mut backend = ConcurrencyBackend::loom();
    backend
        .register("request", "terminal", |_, _| EngineOutcome::Falsified {
            schedules: 1,
            counterexamples: vec![valid_counterexample(OriginKind::Shuttle, Some(42))],
        })
        .unwrap();
    assert!(matches!(
        backend.run(&loom_plan(1_000)).verdict,
        Verdict::Error { .. }
    ));

    let mut backend = ConcurrencyBackend::loom();
    backend
        .register("request", "terminal", |_, _| EngineOutcome::Falsified {
            schedules: 1,
            counterexamples: vec![valid_counterexample(OriginKind::Loom, None)],
        })
        .unwrap();
    let report = backend.run(&loom_plan(1_000));
    assert!(matches!(report.verdict, Verdict::Falsified { .. }));
    let scenario = Scenario::from_json(&report.scenarios[0].to_string()).unwrap();
    assert_eq!(scenario.scenario().origin.kind, OriginKind::Loom);
    assert_eq!(scenario.scenario().expect, Expectation::InvariantViolated);
    assert_eq!(scenario.scenario().status, Status::Active);
}

#[test]
fn impossible_schedule_accounting_is_an_error() {
    for schedules in [0, 9] {
        let mut backend = ConcurrencyBackend::loom();
        backend
            .register("request", "terminal", move |_, _| {
                EngineOutcome::Exhausted { schedules }
            })
            .unwrap();
        assert!(matches!(
            backend.run(&loom_plan(1_000)).verdict,
            Verdict::Error { .. }
        ));
    }
}

#[test]
fn impossible_engine_outcomes_are_rejected() {
    let mut shuttle = ConcurrencyBackend::shuttle();
    shuttle
        .register("request", "terminal", |_, _| EngineOutcome::Exhausted {
            schedules: 3,
        })
        .unwrap();
    assert!(matches!(
        shuttle.run(&shuttle_plan(1_000)).verdict,
        Verdict::Error { .. }
    ));

    let mut loom = ConcurrencyBackend::loom();
    loom.register("request", "terminal", |_, _| EngineOutcome::Incomplete {
        schedules: 1,
        reason: StopReason::StepLimit,
    })
    .unwrap();
    assert!(matches!(
        loom.run(&loom_plan(1_000)).verdict,
        Verdict::Error { .. }
    ));
}

#[test]
fn backend_contains_untyped_project_panics_without_leaking_payload() {
    let mut backend = ConcurrencyBackend::loom();
    backend
        .register("request", "terminal", |_, _| panic!("private panic detail"))
        .unwrap();
    let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        backend.run(&loom_plan(1_000))
    }))
    .expect("backend must contain project harness panics");
    match report.verdict {
        Verdict::Error { reason } => assert!(!reason.contains("private panic detail")),
        verdict => panic!("unexpected verdict: {verdict:?}"),
    }
}

#[test]
fn elapsed_deadline_is_incomplete_even_if_harness_claims_exhaustion() {
    let mut backend = ConcurrencyBackend::loom();
    backend
        .register("request", "terminal", |_, _| {
            std::thread::sleep(Duration::from_millis(20));
            EngineOutcome::Exhausted { schedules: 1 }
        })
        .unwrap();
    assert!(matches!(
        backend.run(&loom_plan(1)).verdict,
        Verdict::Incomplete { .. }
    ));
}

#[test]
fn loom_helper_enforces_the_exact_permutation_cap() {
    let schedules = Arc::new(AtomicUsize::new(0));
    let observed = schedules.clone();
    let limits = LoomLimits::new(32, 2, NonZeroU64::new(1_000).unwrap()).unwrap();
    let outcome = run_loom(limits, move || {
        observed.fetch_add(1, Ordering::SeqCst);
        let first = loom::thread::spawn(loom::thread::yield_now);
        let second = loom::thread::spawn(loom::thread::yield_now);
        first.join().unwrap();
        second.join().unwrap();
    });
    assert_eq!(schedules.load(Ordering::SeqCst), 2);
    assert!(matches!(
        outcome,
        EngineOutcome::Incomplete {
            schedules: 2,
            reason: StopReason::PermutationLimit
        }
    ));
}

#[test]
fn shuttle_helper_reports_the_actual_seeded_iteration_count() {
    let limits = ShuttleLimits::new(4, 32, NonZeroU64::new(1_000).unwrap()).unwrap();
    let outcome = run_shuttle(limits, 7, || {
        let worker = shuttle::thread::spawn(shuttle::thread::yield_now);
        worker.join().unwrap();
    });
    assert!(matches!(outcome, EngineOutcome::Exhausted { schedules: 4 }));
}

#[test]
fn helper_limits_have_distinct_typed_stop_reasons() {
    let loom = run_loom(
        LoomLimits::new(1, 8, NonZeroU64::new(1_000).unwrap()).unwrap(),
        || {
            let first = loom::thread::spawn(loom::thread::yield_now);
            let second = loom::thread::spawn(loom::thread::yield_now);
            first.join().unwrap();
            second.join().unwrap();
        },
    );
    assert!(matches!(
        loom,
        EngineOutcome::Incomplete {
            reason: StopReason::BranchLimit,
            ..
        }
    ));

    let shuttle = run_shuttle(
        ShuttleLimits::new(4, 1, NonZeroU64::new(1_000).unwrap()).unwrap(),
        7,
        || {
            shuttle::thread::yield_now();
            shuttle::thread::yield_now();
        },
    );
    assert!(matches!(
        shuttle,
        EngineOutcome::Incomplete {
            reason: StopReason::StepLimit,
            ..
        }
    ));
}

#[test]
fn helper_deadlocks_are_infrastructure_errors() {
    let outcome = run_loom(
        LoomLimits::new(32, 8, NonZeroU64::new(1_000).unwrap()).unwrap(),
        || {
            let lock = loom::sync::Mutex::new(());
            let _first = lock.lock().unwrap();
            let _second = lock.lock().unwrap();
        },
    );
    assert!(matches!(
        outcome,
        EngineOutcome::Error {
            reason: EngineError::Deadlock
        }
    ));
}

#[test]
fn helper_deadline_reports_the_actual_completed_schedule_count() {
    let outcome = run_loom(
        LoomLimits::new(32, 8, NonZeroU64::new(1).unwrap()).unwrap(),
        || std::thread::sleep(Duration::from_millis(2)),
    );
    assert!(matches!(
        outcome,
        EngineOutcome::Incomplete {
            schedules: 1,
            reason: StopReason::Deadline
        }
    ));
}

#[test]
fn shuttle_rejects_ambient_seed_override_without_mutating_global_environment() {
    const CHILD: &str = "VERIFY_LOOM_SHUTTLE_ENV_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let limits = ShuttleLimits::new(1, 8, NonZeroU64::new(1_000).unwrap()).unwrap();
        assert!(matches!(
            run_shuttle(limits, 7, || {}),
            EngineOutcome::Error {
                reason: EngineError::EnvironmentConflict
            }
        ));
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "shuttle_rejects_ambient_seed_override_without_mutating_global_environment",
        ])
        .env(CHILD, "1")
        .env("SHUTTLE_RANDOM_SEED", "99")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn extreme_limits_are_rejected_before_engine_allocation() {
    assert!(LoomLimits::new(usize::MAX, 1, NonZeroU64::new(1).unwrap()).is_err());
    assert!(ShuttleLimits::new(1, usize::MAX, NonZeroU64::new(1).unwrap()).is_err());
}
