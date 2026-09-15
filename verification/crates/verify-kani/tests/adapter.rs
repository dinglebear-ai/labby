//! Fake-process boundary tests for the bounded Kani adapter.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use verify_core::{Availability, Backend, Bounds, CheckPlan, InvariantId, Verdict};
use verify_kani::{KaniBackend, KaniHarness};

fn invariant() -> InvariantId {
    InvariantId::try_from("TEST-REQ-001".to_owned()).unwrap()
}

fn bounds(max_output_bytes: usize) -> Bounds {
    BTreeMap::from([
        (
            "max_output_bytes".into(),
            serde_json::json!(max_output_bytes),
        ),
        ("unwind".into(), serde_json::json!(8)),
    ])
}

fn plan(timeout_ms: u64, max_output_bytes: usize) -> CheckPlan {
    CheckPlan {
        invariant: invariant(),
        model: "request".into(),
        handle: "terminal".into(),
        bounds: bounds(max_output_bytes),
        seed: None,
        timeout_ms: NonZeroU64::new(timeout_ms).unwrap(),
    }
}

fn named_executable(directory: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn executable(directory: &TempDir, body: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    named_executable(directory, &format!("kani-driver-{id}"), body)
}

fn backend(path: &Path) -> KaniBackend {
    let mut backend = KaniBackend::new(path);
    backend
        .register(
            "request",
            "terminal",
            KaniHarness::new("missing-until-execution.rs", "proof").unwrap(),
        )
        .unwrap();
    backend
}

#[test]
fn handle_registration_and_lookup_do_not_execute_or_resolve_sources() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("executed");
    let script = executable(
        &directory,
        &format!("touch '{}'; printf 'kani 0.67.0\\n'", marker.display()),
    );
    let backend = backend(&script);
    assert!(backend.has_handle("request", "terminal"));
    assert!(!backend.has_handle("request", "other"));
    assert!(!marker.exists());
}

#[test]
fn duplicate_registration_does_not_replace_the_original_metadata() {
    let mut backend = KaniBackend::new("kani-driver");
    backend
        .register(
            "request",
            "terminal",
            KaniHarness::new("first.rs", "first").unwrap(),
        )
        .unwrap();
    assert!(
        backend
            .register(
                "request",
                "terminal",
                KaniHarness::new("replacement.rs", "replacement").unwrap(),
            )
            .is_err()
    );
    assert!(backend.has_handle("request", "terminal"));
}

#[test]
fn invalid_bounds_fail_before_the_tool_is_probed() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("executed");
    let script = executable(
        &directory,
        &format!("touch '{}'; printf 'kani 0.67.0\\n'", marker.display()),
    );
    let mut invalid = plan(500, 1024);
    invalid.bounds.insert("unwind".into(), serde_json::json!(0));
    assert!(matches!(
        backend(&script).run(&invalid).verdict,
        Verdict::Error { .. }
    ));
    assert!(!marker.exists());
}

#[test]
fn absent_or_wrong_version_is_skipped() {
    let missing = backend(Path::new("definitely-not-a-real-kani-driver"));
    assert!(matches!(
        missing.availability(),
        Availability::Missing { .. }
    ));
    assert!(matches!(
        missing.run(&plan(500, 1024)).verdict,
        Verdict::Skipped { .. }
    ));

    let directory = tempfile::tempdir().unwrap();
    let wrong = executable(&directory, "printf 'kani 0.66.0\\n'");
    assert!(matches!(
        backend(&wrong).run(&plan(500, 1024)).verdict,
        Verdict::Skipped { .. }
    ));
}

#[test]
fn explicit_bundle_driver_can_resolve_its_sibling_tools() {
    let directory = tempfile::tempdir().unwrap();
    named_executable(
        &directory,
        "goto-cc",
        "printf 'VERIFICATION:- SUCCESSFUL\\nComplete - 1 successfully verified harnesses, 0 failures, 1 total.\\n'",
    );
    let driver = named_executable(
        &directory,
        "kani",
        "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\ncommand -v goto-cc >/dev/null || exit 1\nprintf 'VERIFICATION:- SUCCESSFUL\\nComplete - 1 successfully verified harnesses, 0 failures, 1 total.\\n'; exit 0",
    );
    let report = backend(&driver).run(&plan(500, 4096));
    assert!(
        matches!(report.verdict, Verdict::Bounded { .. }),
        "sibling tool was not resolved: {:?}",
        report.verdict
    );
}

#[test]
fn fake_process_results_preserve_kani_semantics() {
    let directory = tempfile::tempdir().unwrap();
    let success = executable(
        &directory,
        "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nprintf 'VERIFICATION:- SUCCESSFUL\\nComplete - 1 successfully verified harnesses, 0 failures, 1 total.\\n'",
    );
    assert!(matches!(
        backend(&success).run(&plan(500, 4096)).verdict,
        Verdict::Bounded { .. }
    ));

    let failure = executable(
        &directory,
        "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nprintf 'Status: FAILURE\\nVERIFICATION:- FAILED\\n1 failures, 1 total.\\n'; exit 1",
    );
    let report = backend(&failure).run(&plan(500, 4096));
    assert!(matches!(report.verdict, Verdict::Falsified { .. }));
    assert!(report.scenarios.is_empty());

    let unwind = executable(
        &directory,
        "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nprintf 'Status: FAILURE\\nDescription: unwinding assertion loop 1\\nVERIFICATION:- FAILED\\n'; exit 1",
    );
    assert!(matches!(
        backend(&unwind).run(&plan(500, 4096)).verdict,
        Verdict::Incomplete { .. }
    ));

    let compiler = executable(
        &directory,
        "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nprintf 'error[E0308]: mismatched types\\n' >&2; exit 1",
    );
    assert!(matches!(
        backend(&compiler).run(&plan(500, 4096)).verdict,
        Verdict::Error { .. }
    ));
}

#[test]
fn deadline_terminates_the_process_group() {
    let directory = tempfile::tempdir().unwrap();
    let child_pid = directory.path().join("child.pid");
    let script = executable(
        &directory,
        &format!(
            "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nsleep 30 &\nprintf '%s' $! > '{}'\nwait",
            child_pid.display()
        ),
    );
    let started = Instant::now();
    let report = backend(&script).run(&plan(50, 4096));
    assert!(matches!(report.verdict, Verdict::Incomplete { .. }));
    assert!(started.elapsed() < Duration::from_secs(3));
    let pid = fs::read_to_string(child_pid).unwrap();
    let status = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "descendant process {pid} survived timeout"
    );
}

#[test]
fn completed_parent_does_not_leave_a_pipe_holding_descendant() {
    let directory = tempfile::tempdir().unwrap();
    let child_pid = directory.path().join("detached-child.pid");
    let script = executable(
        &directory,
        &format!(
            "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nsleep 30 &\nprintf '%s' $! > '{}'\nprintf 'driver exited without a result\\n'",
            child_pid.display()
        ),
    );
    let started = Instant::now();
    let report = backend(&script).run(&plan(500, 4096));
    assert!(matches!(report.verdict, Verdict::Error { .. }));
    assert!(started.elapsed() < Duration::from_secs(3));
    let pid = fs::read_to_string(child_pid).unwrap();
    let status = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "descendant process {pid} survived parent exit"
    );
}

#[test]
fn combined_output_limit_is_bounded_and_incomplete() {
    let directory = tempfile::tempdir().unwrap();
    let script = executable(
        &directory,
        "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nwhile :; do printf '0123456789'; printf 'abcdefghij' >&2; done",
    );
    let report = backend(&script).run(&plan(500, 128));
    assert!(matches!(report.verdict, Verdict::Incomplete { .. }));
}

#[test]
fn detached_descendant_output_handles_do_not_bypass_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("detached.py");
    let pid_file = directory.path().join("detached.pid");
    fs::write(
        &fixture,
        r#"import os, sys, time
pid = os.fork()
if pid == 0:
    os.setsid()
    with open(sys.argv[1], "w") as marker:
        marker.write(str(os.getpid()))
    time.sleep(2)
    os._exit(0)
while not os.path.exists(sys.argv[1]):
    time.sleep(0.005)
print("VERIFICATION:- SUCCESSFUL", flush=True)
"#,
    )
    .unwrap();
    let script = executable(
        &directory,
        &format!(
            "if [ \"$1\" = --version ]; then printf 'kani 0.67.0\\n'; exit 0; fi\nexec python3 '{}' '{}'",
            fixture.display(),
            pid_file.display()
        ),
    );
    let started = Instant::now();
    let report = backend(&script).run(&plan(500, 4096));
    let elapsed = started.elapsed();
    // The adapter cannot reap an escaped descendant. The fixture owns cleanup.
    if let Ok(pid) = fs::read_to_string(&pid_file) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", pid.trim()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    assert!(
        elapsed < Duration::from_millis(1500),
        "deadline was bypassed: {elapsed:?}"
    );
    assert!(
        matches!(report.verdict, Verdict::Incomplete { .. }),
        "{report:?}"
    );
}
