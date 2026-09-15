//! Finite replay, normalization, registry, CLI and corpus regression contracts.

#[path = "fixtures/counter.rs"]
mod counter;

use counter::{Counter, catalog, registry, scenario};
use serde_json::json;
use std::{cell::Cell, ffi::OsString, num::NonZeroUsize, process::Command, time::Duration};
use verify_core::{ScenarioError, ScenarioTarget, StepOutcome};
use verify_runner::{
    CorpusError, InsertResult, NormalizationOptions, ReplayLimits, TargetRegistry, TraceVerdict,
    insert_scenario, run_cli,
};
use verify_scenario::Status;

#[test]
fn normalization_retains_typed_target_resolution_errors() {
    use verify_runner::{NormalizationError, TargetResolutionError};
    let runner = registry(Counter::default(), "safety");
    let mut unknown = scenario(&[], "invariant_holds", "active")
        .scenario()
        .clone();
    unknown.model = "missing".into();
    unknown.fingerprint = None;
    match runner
        .normalize(
            &unknown.validate().unwrap(),
            &NormalizationOptions::default(),
        )
        .unwrap_err()
    {
        NormalizationError::Target(TargetResolutionError::UnknownTarget {
            project,
            model,
            available,
        }) => {
            assert_eq!(project, "example");
            assert_eq!(model, "missing");
            assert_eq!(available, vec![("example".into(), "counter".into())]);
        }
        error => panic!("unexpected typed error: {error}"),
    }
    let mut unknown = scenario(&[], "invariant_holds", "active")
        .scenario()
        .clone();
    unknown.invariant = "EX-COUNT-999".to_owned().try_into().unwrap();
    unknown.fingerprint = None;
    match runner
        .normalize(
            &unknown.validate().unwrap(),
            &NormalizationOptions::default(),
        )
        .unwrap_err()
    {
        NormalizationError::Target(TargetResolutionError::UnknownInvariant {
            project,
            model,
            invariant,
        }) => {
            assert_eq!(project, "example");
            assert_eq!(model, "counter");
            assert_eq!(invariant.as_str(), "EX-COUNT-999");
        }
        error => panic!("unexpected typed error: {error}"),
    }
}

#[test]
fn cli_rejects_duplicate_keys_as_invalid_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("duplicate.json");
    let raw = r#"{"schema":1,"project":"example","model":"counter","invariant":"EX-COUNT-001","origin":{"kind":"manual"},"steps":[{"actor":"a","value":3,"value":0}],"expect":"invariant_holds"}"#;
    std::fs::write(&path, raw).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_verify"))
        .arg("replay")
        .arg(path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("duplicate JSON object key")
    );
}

#[test]
fn normalization_cannot_disable_an_active_regression_gate() {
    let runner = registry(Counter::default(), "safety");
    for (values, expect) in [
        (&[2][..], "invariant_holds"),
        (&[0][..], "invariant_violated"),
    ] {
        let original = scenario(values, expect, "active");
        assert!(
            runner
                .replay(&original, &ReplayLimits::default())
                .gate_failure
        );
        let normalized = runner
            .normalize(&original, &NormalizationOptions::default())
            .unwrap();
        assert_eq!(normalized.scenario.scenario().status, Status::Active);
        assert!(
            runner
                .replay(&normalized.scenario, &ReplayLimits::default())
                .gate_failure
        );
    }
}

#[test]
fn identifier_renaming_cannot_add_or_remove_golden_steps() {
    let original = scenario(&[0, 1, 0], "invariant_holds", "active");
    for rename_length_change in [-1, 1] {
        let runner = registry(
            Counter {
                rename_length_change,
                ..Default::default()
            },
            "safety",
        );
        let normalized = runner
            .normalize(&original, &NormalizationOptions::default())
            .unwrap();
        assert!(normalized.discarded_transformation);
        assert_eq!(
            normalized.scenario.scenario().steps,
            original.scenario().steps
        );
    }
}

#[cfg(unix)]
#[test]
fn cli_rejects_fifo_before_blocking_open() {
    let temporary = tempfile::tempdir().unwrap();
    let fifo = temporary.path().join("scenario.json");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_verify"))
        .arg("replay")
        .arg(&fifo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(2));
            break;
        }
        // Includes cold binary startup on macOS, not just the file operation.
        if started.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("scenario loader blocked opening a FIFO");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn transient_and_initial_violations_are_sticky_and_empty_golden_is_valid() {
    let runner = registry(Counter::default(), "safety");
    let trace = scenario(&[0, 2, 0], "invariant_violated", "active");
    let report = runner.replay(&trace, &ReplayLimits::default());
    assert_eq!(report.verdict, TraceVerdict::InvariantViolated);
    assert_eq!(report.first_violation, Some(2));
    assert_eq!(report.observations.len(), 4);
    assert!(!report.gate_failure);
    let mut initial = trace.scenario().clone();
    initial.initial.insert("value".into(), json!(3));
    initial.fingerprint = None;
    assert_eq!(
        runner
            .replay(&initial.validate().unwrap(), &ReplayLimits::default())
            .first_violation,
        Some(0)
    );
    let empty = runner.replay(
        &scenario(&[], "invariant_holds", "active"),
        &ReplayLimits::default(),
    );
    assert_eq!(empty.verdict, TraceVerdict::InvariantHolds);
    assert_eq!(empty.observations.len(), 1);
}

#[test]
fn all_expectation_status_combinations_follow_the_gate_table() {
    let runner = registry(Counter::default(), "security");
    for status in ["active", "quarantined", "unreproduced"] {
        for expect in ["invariant_holds", "invariant_violated"] {
            for values in [&[0][..], &[2][..]] {
                let report =
                    runner.replay(&scenario(values, expect, status), &ReplayLimits::default());
                let matches = (values == [0] && expect == "invariant_holds")
                    || (values == [2] && expect == "invariant_violated");
                assert_eq!(report.matches_expectation, matches);
                assert_eq!(report.gate_failure, status == "active" && !matches);
                assert_eq!(
                    report.suggested_promotion,
                    status == "unreproduced" && matches
                );
            }
        }
    }
}

#[test]
fn failures_never_satisfy_counterexample_expectations() {
    let runner = registry(Counter::default(), "safety");
    for values in [&[-99][..], &[2, -99][..]] {
        let report = runner.replay(
            &scenario(values, "invariant_violated", "active"),
            &ReplayLimits::default(),
        );
        assert_eq!(report.verdict, TraceVerdict::Error);
        assert!(report.gate_failure);
    }
    let mut raw = scenario(&[], "invariant_violated", "active")
        .scenario()
        .clone();
    raw.invariant = "EX-COUNT-002".to_owned().try_into().unwrap();
    raw.fingerprint = None;
    assert_eq!(
        runner
            .replay(&raw.validate().unwrap(), &ReplayLimits::default())
            .verdict,
        TraceVerdict::Error
    );
    let mut raw = scenario(&[], "invariant_violated", "active")
        .scenario()
        .clone();
    raw.steps = vec![json!({"bad":true})];
    raw.fingerprint = None;
    assert_eq!(
        runner
            .replay(&raw.validate().unwrap(), &ReplayLimits::default())
            .verdict,
        TraceVerdict::Error
    );
    let mut raw = scenario(&[], "invariant_violated", "active")
        .scenario()
        .clone();
    raw.initial.insert("value".into(), json!("invalid"));
    raw.fingerprint = None;
    assert_eq!(
        runner
            .replay(&raw.validate().unwrap(), &ReplayLimits::default())
            .verdict,
        TraceVerdict::Error
    );
}

#[test]
fn rejections_and_incomplete_observations_remain_distinct() {
    let runner = registry(Counter::default(), "safety");
    let report = runner.replay(
        &scenario(&[-1], "invariant_holds", "active"),
        &ReplayLimits::default(),
    );
    assert_eq!(
        report.observations[1].outcome,
        Some(StepOutcome::Rejected {
            reason: "fixture rejection".into()
        })
    );
    let report = runner.replay(
        &scenario(&[-2, 0], "invariant_holds", "active"),
        &ReplayLimits::default(),
    );
    assert_eq!(report.verdict, TraceVerdict::Incomplete);
    assert!(report.gate_failure);
}

#[test]
fn registry_rejects_unknown_identity_and_replacement() {
    let mut runner = registry(Counter::default(), "safety");
    assert!(
        runner
            .register(&catalog("safety"), "counter", Counter::default())
            .is_err()
    );
    assert!(
        runner
            .register(&catalog("safety"), "unknown", Counter::default())
            .is_err()
    );
    assert_eq!(runner.targets(), vec![("example", "counter")]);
    let mut raw = scenario(&[], "invariant_holds", "active")
        .scenario()
        .clone();
    raw.project = "other".into();
    raw.fingerprint = None;
    assert_eq!(
        runner
            .replay(&raw.validate().unwrap(), &ReplayLimits::default())
            .verdict,
        TraceVerdict::Error
    );
    let mut raw = scenario(&[], "invariant_holds", "active")
        .scenario()
        .clone();
    raw.invariant = "EX-COUNT-999".to_owned().try_into().unwrap();
    raw.fingerprint = None;
    assert_eq!(
        runner
            .replay(&raw.validate().unwrap(), &ReplayLimits::default())
            .verdict,
        TraceVerdict::Error
    );
}

#[test]
fn unsupported_semantics_and_exhausted_budgets_are_incomplete() {
    let trace = scenario(&[0], "invariant_holds", "active");
    for kind in ["liveness", "refinement"] {
        assert_eq!(
            registry(Counter::default(), kind)
                .replay(&trace, &ReplayLimits::default())
                .verdict,
            TraceVerdict::Incomplete
        );
    }
    let runner = registry(Counter::default(), "safety");
    for limits in [
        ReplayLimits {
            max_steps: 0,
            ..Default::default()
        },
        ReplayLimits {
            timeout: Duration::ZERO,
            ..Default::default()
        },
    ] {
        assert_eq!(
            runner.replay(&trace, &limits).verdict,
            TraceVerdict::Incomplete
        );
    }
}

#[test]
fn golden_traces_never_shrink_but_counterexamples_do() {
    let runner = registry(Counter::default(), "safety");
    let golden = scenario(&[0, 1, 0], "invariant_holds", "active");
    let normalized = runner
        .normalize(&golden, &NormalizationOptions::default())
        .unwrap();
    assert_eq!(normalized.scenario.scenario().steps.len(), 3);
    assert_eq!(
        normalized.scenario.scenario().steps[0],
        json!({"actor":"actor_0","value":0})
    );
    let violation = scenario(&[0, 0, 2, 0, 0], "invariant_violated", "active");
    let normalized = runner
        .normalize(&violation, &NormalizationOptions::default())
        .unwrap();
    assert_eq!(
        normalized.scenario.scenario().steps,
        vec![json!({"actor":"actor_0","value":2})]
    );
    assert_eq!(
        runner
            .replay(&normalized.scenario, &ReplayLimits::default())
            .verdict,
        TraceVerdict::InvariantViolated
    );
    let second = runner
        .normalize(&normalized.scenario, &NormalizationOptions::default())
        .unwrap();
    assert_eq!(
        normalized.scenario.fingerprint(),
        second.scenario.fingerprint()
    );
}

#[test]
fn canonical_ids_collapse_cross_origin_traces_and_bad_renaming_is_discarded() {
    let runner = registry(Counter::default(), "safety");
    let original = scenario(&[2], "invariant_violated", "active");
    let mut renamed = original.scenario().clone();
    renamed.steps[0]["actor"] = json!("totally_different");
    renamed.fingerprint = None;
    assert_eq!(
        runner
            .normalize(&original, &NormalizationOptions::default())
            .unwrap()
            .scenario
            .fingerprint(),
        runner
            .normalize(
                &renamed.validate().unwrap(),
                &NormalizationOptions::default()
            )
            .unwrap()
            .scenario
            .fingerprint()
    );
    let broken = registry(
        Counter {
            broken_rename: true,
            ..Default::default()
        },
        "safety",
    );
    let result = broken
        .normalize(&original, &NormalizationOptions::default())
        .unwrap();
    assert!(result.discarded_transformation);
    assert_eq!(result.scenario.fingerprint(), original.fingerprint());
}

#[test]
fn determinism_failures_quarantine_and_budget_is_explicit() {
    let flaky = registry(
        Counter {
            flaky: Some(Cell::new(false)),
            ..Default::default()
        },
        "safety",
    );
    let trace = scenario(&[], "invariant_holds", "active");
    assert_eq!(
        flaky
            .normalize(&trace, &NormalizationOptions::default())
            .unwrap()
            .scenario
            .scenario()
            .status,
        Status::Quarantined
    );
    let runner = registry(Counter::default(), "safety");
    let options = NormalizationOptions {
        max_replays: NonZeroUsize::new(6).unwrap(),
        ..Default::default()
    };
    let result = runner
        .normalize(
            &scenario(&[0, 2, 0], "invariant_violated", "active"),
            &options,
        )
        .unwrap();
    assert!(result.budget_exhausted);
    assert_eq!(result.replays, 6);
    assert!(
        runner
            .normalize(
                &trace,
                &NormalizationOptions {
                    determinism_runs: NonZeroUsize::new(1).unwrap(),
                    ..Default::default()
                }
            )
            .is_err()
    );
}

#[test]
fn unmatched_evidence_is_not_shrunk_and_promotion_is_never_automatic() {
    let runner = registry(Counter::default(), "safety");
    let result = runner
        .normalize(
            &scenario(&[0, 0], "invariant_violated", "unreproduced"),
            &NormalizationOptions::default(),
        )
        .unwrap();
    assert_eq!(result.scenario.scenario().status, Status::Unreproduced);
    assert_eq!(result.scenario.scenario().steps.len(), 2);
    let result = runner
        .normalize(
            &scenario(&[0], "invariant_holds", "unreproduced"),
            &NormalizationOptions::default(),
        )
        .unwrap();
    assert_eq!(result.scenario.scenario().status, Status::Unreproduced);
    assert!(
        runner
            .replay(&result.scenario, &ReplayLimits::default())
            .suggested_promotion
    );
    assert!(
        runner
            .normalize(
                &scenario(&[-99], "invariant_violated", "active"),
                &NormalizationOptions::default()
            )
            .is_err()
    );
}

#[test]
fn corpus_is_content_addressed_non_overwriting_and_replay_does_not_mutate() {
    let directory = tempfile::tempdir().unwrap();
    let trace = scenario(&[2], "invariant_violated", "active");
    assert_eq!(
        insert_scenario(directory.path(), &trace).unwrap(),
        InsertResult::Inserted
    );
    let path = directory.path().join(trace.corpus_path());
    let before = std::fs::read(&path).unwrap();
    let mut alternate = trace.scenario().clone();
    alternate.status = Status::Unreproduced;
    assert_eq!(
        insert_scenario(directory.path(), &alternate.validate().unwrap()).unwrap(),
        InsertResult::Existing
    );
    let mut output = Vec::new();
    let mut errors = Vec::new();
    assert_eq!(
        run_cli(
            &registry(Counter::default(), "safety"),
            [OsString::from("replay"), path.clone().into_os_string()],
            &mut output,
            &mut errors
        ),
        0
    );
    let report: verify_runner::ReplayReport = serde_json::from_slice(&output).unwrap();
    assert_eq!(report.verdict, TraceVerdict::InvariantViolated);
    assert!(errors.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::write(&path, b"corrupt").unwrap();
    assert!(matches!(
        insert_scenario(directory.path(), &trace),
        Err(CorpusError::Envelope(_))
    ));
}

#[test]
fn cli_status_exit_codes_and_invalid_input_are_not_hidden() {
    let directory = tempfile::tempdir().unwrap();
    let runner = registry(Counter::default(), "safety");
    for (status, expected) in [("active", 1), ("quarantined", 0), ("unreproduced", 0)] {
        let trace = scenario(&[2], "invariant_holds", status);
        let path = directory.path().join(status);
        std::fs::write(&path, serde_json::to_vec(trace.scenario()).unwrap()).unwrap();
        assert_eq!(
            run_cli(
                &runner,
                [OsString::from("replay"), path.into_os_string()],
                &mut Vec::new(),
                &mut Vec::new()
            ),
            expected
        );
    }
    for args in [
        vec![],
        vec![OsString::from("replay")],
        vec![OsString::from("oops")],
        vec![
            OsString::from("replay"),
            directory.path().join("absent").into_os_string(),
        ],
    ] {
        assert_eq!(run_cli(&runner, args, &mut Vec::new(), &mut Vec::new()), 2);
    }
    let path = directory.path().join("active");
    let output = Command::new(env!("CARGO_BIN_EXE_verify"))
        .arg("replay")
        .arg(path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: verify_runner::ReplayReport = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.verdict, TraceVerdict::Error);
    assert!(report.diagnostic.unwrap().contains("unknown target"));
    assert!(
        Command::new(env!("CARGO_BIN_EXE_verify"))
            .arg("--help")
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[cfg(unix)]
#[test]
fn corpus_rejects_symlink_directories_and_files() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let trace = scenario(&[], "invariant_holds", "active");
    symlink(outside.path(), root.path().join("example")).unwrap();
    assert!(matches!(
        insert_scenario(root.path(), &trace),
        Err(CorpusError::UnsafePath)
    ));
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn cooperative_deadline_after_slow_call_is_incomplete() {
    struct Slow(Counter);
    impl ScenarioTarget for Slow {
        type State = i64;
        type Step = counter::Step;
        fn init(&self, initial: &serde_json::Value) -> Result<i64, ScenarioError> {
            self.0.init(initial)
        }
        fn apply(&self, state: &mut i64, step: &Self::Step) -> Result<StepOutcome, ScenarioError> {
            std::thread::sleep(Duration::from_millis(5));
            self.0.apply(state, step)
        }
        fn check(
            &self,
            id: &verify_core::InvariantId,
            state: &i64,
        ) -> Result<verify_core::InvariantResult, ScenarioError> {
            self.0.check(id, state)
        }
    }
    let mut runner = TargetRegistry::default();
    runner
        .register(&catalog("safety"), "counter", Slow(Counter::default()))
        .unwrap();
    let report = runner.replay(
        &scenario(&[0], "invariant_holds", "active"),
        &ReplayLimits {
            timeout: Duration::from_millis(1),
            ..Default::default()
        },
    );
    assert_eq!(report.verdict, TraceVerdict::Incomplete);
}
