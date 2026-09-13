//! End-to-end lifecycle model checks for both supported concurrency engines.

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use verify_core::{Backend, CheckPlan, InvariantId, Verdict};
use verify_loom::{
    ConcurrencyBackend, EngineLimits, LoomLimits, ShuttleLimits, raise_counterexample, run_loom,
    run_shuttle,
};
use verify_scenario::{Expectation, Origin, OriginKind, Scenario, Status, ValidatedScenario};

const INVARIANT: &str = "TEST-REQ-001";

fn plan(handle: &str, engine: OriginKind) -> CheckPlan {
    let (bounds, seed) = match engine {
        OriginKind::Loom => (
            BTreeMap::from([
                ("max_branches".into(), serde_json::json!(128)),
                ("max_permutations".into(), serde_json::json!(1_024)),
            ]),
            None,
        ),
        OriginKind::Shuttle => (
            BTreeMap::from([
                ("max_iterations".into(), serde_json::json!(256)),
                ("max_steps".into(), serde_json::json!(128)),
            ]),
            Some(42),
        ),
        _ => unreachable!("test selects only concurrency engines"),
    };
    CheckPlan {
        invariant: InvariantId::try_from(INVARIANT.to_owned()).unwrap(),
        model: "request".into(),
        handle: handle.into(),
        bounds,
        seed,
        timeout_ms: NonZeroU64::new(5_000).unwrap(),
    }
}

fn counterexample(
    engine: OriginKind,
    seed: Option<u64>,
    actions: Vec<&'static str>,
) -> ValidatedScenario {
    let version = match engine {
        OriginKind::Loom => "0.7.2",
        OriginKind::Shuttle => "0.9.0",
        _ => unreachable!("test selects only concurrency engines"),
    };
    let steps = actions
        .into_iter()
        .map(|action| serde_json::json!({"action": action}))
        .collect::<Vec<_>>();
    // Build typed evidence directly: JSON encode/decode on Loom's small
    // coroutine stack can overflow before the negative control is reported.
    Scenario {
        schema: 1,
        project: "verify-loom-fixture".into(),
        model: "request".into(),
        invariant: InvariantId::try_from(INVARIANT.to_owned()).unwrap(),
        origin: Origin {
            kind: engine,
            tool_version: Some(version.into()),
            discovered_at: None,
            seed,
            reference: None,
        },
        bounds: BTreeMap::new(),
        initial: serde_json::Map::from_iter([
            ("state".into(), "active".into()),
            ("terminal_writes".into(), 0.into()),
        ]),
        steps,
        expect: Expectation::InvariantViolated,
        status: Status::Active,
        fingerprint: None,
    }
    .validate()
    .unwrap()
}

fn loom_positive() {
    use loom::sync::Arc;
    use loom::sync::atomic::{AtomicUsize, Ordering};
    use loom::thread;

    let state = Arc::new(AtomicUsize::new(0));
    let terminal_writes = Arc::new(AtomicUsize::new(0));
    let complete_state = Arc::clone(&state);
    let complete_writes = Arc::clone(&terminal_writes);
    let complete = thread::spawn(move || {
        if complete_state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            complete_writes.fetch_add(1, Ordering::SeqCst);
        }
    });
    let cancel_state = Arc::clone(&state);
    let cancel_writes = Arc::clone(&terminal_writes);
    let cancel = thread::spawn(move || {
        if cancel_state
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            cancel_writes.fetch_add(1, Ordering::SeqCst);
        }
    });
    complete.join().unwrap();
    cancel.join().unwrap();
    assert_ne!(state.load(Ordering::SeqCst), 0);
    assert_eq!(terminal_writes.load(Ordering::SeqCst), 1);
}

fn loom_negative() {
    use loom::sync::atomic::{AtomicUsize, Ordering};
    use loom::sync::{Arc, Mutex};
    use loom::thread;

    let state = Arc::new(AtomicUsize::new(0));
    let terminal_writes = Arc::new(AtomicUsize::new(0));
    let actions = Arc::new(Mutex::new(Vec::new()));
    let complete_state = Arc::clone(&state);
    let complete_writes = Arc::clone(&terminal_writes);
    let complete_actions = Arc::clone(&actions);
    let complete = thread::spawn(move || {
        if complete_state.load(Ordering::SeqCst) == 0 {
            complete_actions
                .lock()
                .unwrap()
                .push("complete_read_active");
            thread::yield_now();
            complete_state.store(1, Ordering::SeqCst);
            complete_writes.fetch_add(1, Ordering::SeqCst);
            complete_actions.lock().unwrap().push("complete_write");
        }
    });
    let cancel_state = Arc::clone(&state);
    let cancel_writes = Arc::clone(&terminal_writes);
    let cancel_actions = Arc::clone(&actions);
    let cancel = thread::spawn(move || {
        if cancel_state.load(Ordering::SeqCst) == 0 {
            cancel_actions.lock().unwrap().push("cancel_read_active");
            thread::yield_now();
            cancel_state.store(2, Ordering::SeqCst);
            cancel_writes.fetch_add(1, Ordering::SeqCst);
            cancel_actions.lock().unwrap().push("cancel_write");
        }
    });
    complete.join().unwrap();
    cancel.join().unwrap();
    if terminal_writes.load(Ordering::SeqCst) > 1 {
        let recorded = actions.lock().unwrap().clone();
        raise_counterexample(counterexample(OriginKind::Loom, None, recorded));
    }
}

fn shuttle_positive() {
    use shuttle::sync::Arc;
    use shuttle::sync::atomic::{AtomicUsize, Ordering};
    use shuttle::thread;

    let state = Arc::new(AtomicUsize::new(0));
    let terminal_writes = Arc::new(AtomicUsize::new(0));
    let complete_state = Arc::clone(&state);
    let complete_writes = Arc::clone(&terminal_writes);
    let complete = thread::spawn(move || {
        if complete_state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            complete_writes.fetch_add(1, Ordering::SeqCst);
        }
    });
    let cancel_state = Arc::clone(&state);
    let cancel_writes = Arc::clone(&terminal_writes);
    let cancel = thread::spawn(move || {
        if cancel_state
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            cancel_writes.fetch_add(1, Ordering::SeqCst);
        }
    });
    complete.join().unwrap();
    cancel.join().unwrap();
    assert_ne!(state.load(Ordering::SeqCst), 0);
    assert_eq!(terminal_writes.load(Ordering::SeqCst), 1);
}

fn shuttle_negative(seed: u64) {
    use shuttle::sync::atomic::{AtomicUsize, Ordering};
    use shuttle::sync::{Arc, Mutex};
    use shuttle::thread;

    let state = Arc::new(AtomicUsize::new(0));
    let terminal_writes = Arc::new(AtomicUsize::new(0));
    let actions = Arc::new(Mutex::new(Vec::new()));
    let complete_state = Arc::clone(&state);
    let complete_writes = Arc::clone(&terminal_writes);
    let complete_actions = Arc::clone(&actions);
    let complete = thread::spawn(move || {
        if complete_state.load(Ordering::SeqCst) == 0 {
            complete_actions
                .lock()
                .unwrap()
                .push("complete_read_active");
            thread::yield_now();
            complete_state.store(1, Ordering::SeqCst);
            complete_writes.fetch_add(1, Ordering::SeqCst);
            complete_actions.lock().unwrap().push("complete_write");
        }
    });
    let cancel_state = Arc::clone(&state);
    let cancel_writes = Arc::clone(&terminal_writes);
    let cancel_actions = Arc::clone(&actions);
    let cancel = thread::spawn(move || {
        if cancel_state.load(Ordering::SeqCst) == 0 {
            cancel_actions.lock().unwrap().push("cancel_read_active");
            thread::yield_now();
            cancel_state.store(2, Ordering::SeqCst);
            cancel_writes.fetch_add(1, Ordering::SeqCst);
            cancel_actions.lock().unwrap().push("cancel_write");
        }
    });
    complete.join().unwrap();
    cancel.join().unwrap();
    if terminal_writes.load(Ordering::SeqCst) > 1 {
        let recorded = actions.lock().unwrap().clone();
        raise_counterexample(counterexample(OriginKind::Shuttle, Some(seed), recorded));
    }
}

fn replay_terminal_overwrite(scenario: &ValidatedScenario) -> bool {
    let mut complete_saw_active = false;
    let mut cancel_saw_active = false;
    let mut terminal_writes = 0;
    for step in &scenario.scenario().steps {
        match step["action"].as_str().unwrap() {
            "complete_read_active" => complete_saw_active = true,
            "cancel_read_active" => cancel_saw_active = true,
            "complete_write" if complete_saw_active => terminal_writes += 1,
            "cancel_write" if cancel_saw_active => terminal_writes += 1,
            action => panic!("unexpected replay action: {action}"),
        }
    }
    terminal_writes > 1
}

#[test]
fn loom_finds_and_replays_the_broken_lifecycle_but_accepts_the_atomic_model() {
    let mut backend = ConcurrencyBackend::loom();
    backend
        .register("request", "atomic", |_, limits| {
            let EngineLimits::Loom(limits) = limits else {
                unreachable!()
            };
            run_loom(limits, loom_positive)
        })
        .unwrap();
    backend
        .register("request", "late-overwrite", move |_, limits| {
            let EngineLimits::Loom(limits) = limits else {
                unreachable!()
            };
            run_loom(limits, loom_negative)
        })
        .unwrap();

    assert!(matches!(
        backend.run(&plan("atomic", OriginKind::Loom)).verdict,
        Verdict::Bounded { .. }
    ));
    let report = backend.run(&plan("late-overwrite", OriginKind::Loom));
    assert!(matches!(report.verdict, Verdict::Falsified { .. }));
    let projected = Scenario::from_json(&report.scenarios[0].to_string()).unwrap();
    assert!(replay_terminal_overwrite(&projected));
}

#[test]
fn shuttle_finds_and_replays_the_broken_lifecycle_but_accepts_the_atomic_model() {
    let mut backend = ConcurrencyBackend::shuttle();
    backend
        .register("request", "atomic", |plan, limits| {
            let EngineLimits::Shuttle(limits) = limits else {
                unreachable!()
            };
            run_shuttle(limits, plan.seed.unwrap(), shuttle_positive)
        })
        .unwrap();
    backend
        .register("request", "late-overwrite", move |plan, limits| {
            let EngineLimits::Shuttle(limits) = limits else {
                unreachable!()
            };
            let seed = plan.seed.unwrap();
            run_shuttle(limits, seed, move || shuttle_negative(seed))
        })
        .unwrap();

    assert!(matches!(
        backend.run(&plan("atomic", OriginKind::Shuttle)).verdict,
        Verdict::Bounded { .. }
    ));
    let report = backend.run(&plan("late-overwrite", OriginKind::Shuttle));
    assert!(matches!(report.verdict, Verdict::Falsified { .. }));
    let projected = Scenario::from_json(&report.scenarios[0].to_string()).unwrap();
    assert!(replay_terminal_overwrite(&projected));
}

#[test]
fn lifecycle_limit_types_are_constructible_from_the_test_plan() {
    assert!(LoomLimits::new(128, 1_024, NonZeroU64::new(5_000).unwrap()).is_ok());
    assert!(ShuttleLimits::new(256, 128, NonZeroU64::new(5_000).unwrap()).is_ok());
}
