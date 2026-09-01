#![allow(clippy::panic)]

#[path = "support/lib.rs"]
mod support;

use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

use support::LiveLabbyBuilder;

#[test]
fn orchestration_cleanup_kills_term_resistant_process_group_within_deadline() {
    let run_root = tempfile::tempdir().expect("temporary parent");
    let owned_root = run_root.path().join("cleanup-selftest");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/ci/labby-live-e2e.sh");
    let started = Instant::now();
    let status = Command::new("bash")
        .arg(script)
        .arg("pr")
        .arg("1")
        .env("LABBY_E2E_RUN_ROOT", &owned_root)
        .env("LABBY_E2E_CLEANUP_SELFTEST", "1")
        .status()
        .expect("cleanup self-test starts");

    assert!(status.success(), "cleanup self-test failed: {status}");
    assert!(started.elapsed() < Duration::from_secs(8));
    let report = std::fs::read_to_string(owned_root.join("artifacts/cleanup-selftest.json"))
        .expect("cleanup report");
    assert!(
        report.contains("\"owned_children_absent\":true"),
        "{report}"
    );
}

#[test]
fn outer_supervisor_kills_wedged_cleanup_before_post_deadline_mutation() {
    let run_root = tempfile::tempdir().expect("temporary parent");
    let owned_root = run_root.path().join("wedged-cleanup-selftest");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/ci/labby-live-e2e.sh");
    let started = Instant::now();
    let status = Command::new("bash")
        .arg(script)
        .args(["pr", "1"])
        .env("LABBY_E2E_RUN_ROOT", &owned_root)
        .env("LABBY_E2E_WEDGED_SHARD_SELFTEST", "1")
        .env("LABBY_E2E_SHARD_TIMEOUT_SECONDS", "1")
        .env("LABBY_E2E_RUN_TIMEOUT_SECONDS", "5")
        .env("LABBY_E2E_PREBUILT", "1")
        .env("LABBY_E2E_BINARY", "/usr/bin/true")
        .status()
        .expect("wedged-cleanup self-test starts");

    assert!(
        !status.success(),
        "a watchdog-killed shard must fail qualification: {status}"
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    let marker = owned_root.join("post-deadline-mutation");
    assert!(!marker.exists());
    thread::sleep(Duration::from_secs(2));
    assert!(
        !marker.exists(),
        "terminated shard mutated after supervisor returned"
    );
}

#[test]
fn orchestration_cleanup_reaps_retained_group_after_leader_exits() {
    let run_root = tempfile::tempdir().expect("temporary parent");
    let owned_root = run_root.path().join("retained-group-selftest");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/ci/labby-live-e2e.sh");
    let status = Command::new("bash")
        .arg(script)
        .args(["pr", "1"])
        .env("LABBY_E2E_RUN_ROOT", &owned_root)
        .env("LABBY_E2E_RETAINED_GROUP_SELFTEST", "1")
        .status()
        .expect("retained group self-test starts");

    assert!(
        status.success(),
        "retained group self-test failed: {status}"
    );
    let report = std::fs::read_to_string(owned_root.join("artifacts/retained-group-selftest.json"))
        .expect("retained group report");
    assert!(
        report.contains("\"retained_group_absent\":true"),
        "{report}"
    );
}

#[test]
fn orchestration_secret_scan_detects_nested_retained_canary() {
    let run_root = tempfile::tempdir().expect("temporary parent");
    let owned_root = run_root.path().join("secret-scan-selftest");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/ci/labby-live-e2e.sh");
    let status = Command::new("bash")
        .arg(script)
        .arg("pr")
        .arg("1")
        .env("LABBY_E2E_RUN_ROOT", &owned_root)
        .env("LABBY_E2E_SECRET_SCAN_SELFTEST", "1")
        .status()
        .expect("secret scan self-test starts");

    assert!(status.success(), "secret scan self-test failed: {status}");
    let report = std::fs::read_to_string(owned_root.join("artifacts/secret-scan-selftest.json"))
        .expect("secret scan report");
    assert!(
        report.contains("\"retained_secret_detected\":true"),
        "{report}"
    );
}

#[test]
fn orchestration_cancellation_exits_with_signal_status_and_reaps_children() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let owned_root = parent.path().join("signal-selftest");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/ci/labby-live-e2e.sh");
    let mut child = Command::new("bash")
        .arg(script)
        .arg("pr")
        .arg("1")
        .env("LABBY_E2E_RUN_ROOT", &owned_root)
        .env("LABBY_E2E_SIGNAL_SELFTEST", "1")
        .spawn()
        .expect("signal self-test starts");
    let ready = owned_root.join("signal-selftest.ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let owned_pid = std::fs::read_to_string(&ready).expect("signal self-test readiness");
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let status = child.wait().expect("signal self-test exits");
    assert_eq!(
        status.code(),
        Some(143),
        "unexpected cancellation status: {status}"
    );
    assert!(
        !Command::new("kill")
            .args(["-0", owned_pid.trim()])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn orchestration_exit_cleanup_failure_overrides_success() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let owned_root = parent.path().join("exit-failure-selftest");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/ci/labby-live-e2e.sh");
    let status = Command::new("bash")
        .arg(script)
        .args(["pr", "1"])
        .env("LABBY_E2E_RUN_ROOT", &owned_root)
        .env("LABBY_E2E_EXIT_FAILURE_SELFTEST", "1")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
    let report = std::fs::read_to_string(owned_root.join("artifacts/status.json")).unwrap();
    assert!(report.contains("\"cleanup\":1"), "{report}");
}

#[test]
fn orchestration_listener_audit_is_independent_and_detects_a_listener() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let owned_root = parent.path().join("listener-selftest");
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/ci/labby-live-e2e.sh");
    let status = Command::new("bash")
        .arg(script)
        .args(["pr", "1"])
        .env("LABBY_E2E_RUN_ROOT", &owned_root)
        .env("LABBY_E2E_LISTENER_SELFTEST", "1")
        .status()
        .unwrap();
    assert!(status.success(), "listener self-test failed: {status}");
    let report =
        std::fs::read_to_string(owned_root.join("artifacts/listener-selftest.json")).unwrap();
    assert!(
        report.contains("\"owned_listener_detected\":true"),
        "{report}"
    );
}

#[tokio::test]
async fn boots_real_binary_reports_both_readiness_contracts_and_cleans_up() {
    let guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("live labby starts");
    let root = guard.root().to_path_buf();
    assert!(guard.connection().base_url.starts_with("http://127.0.0.1:"));
    assert!(!guard.identity().binary_sha256.is_empty());
    let result = guard.finish().await;
    assert!(result.is_clean(), "cleanup failures: {:?}", result.failures);
    assert!(!root.exists(), "owned root leaked: {}", root.display());
}

#[tokio::test]
async fn cleanup_is_idempotent() {
    let mut guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("live labby starts");
    let first = guard.finish_with_deadline(Duration::from_secs(10)).await;
    let second = guard.finish_with_deadline(Duration::from_secs(10)).await;
    assert!(first.is_clean(), "{:?}", first.failures);
    assert!(second.is_clean(), "{:?}", second.failures);
}

#[tokio::test]
async fn restart_preserves_connection_and_advances_process_generation() {
    let mut guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("live labby starts");
    let connection = guard.connection().base_url.clone();
    guard.restart().await.expect("live labby restarts");
    assert_eq!(guard.connection().base_url, connection);
    let cleanup = guard.finish().await;
    assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
}

#[tokio::test]
async fn parallel_instances_have_distinct_identity_roots_and_listeners() {
    let (first, second) = tokio::join!(
        LiveLabbyBuilder::new().start(),
        LiveLabbyBuilder::new().start()
    );
    let first = first.expect("first instance");
    let second = second.expect("second instance");
    assert_ne!(first.root(), second.root());
    assert_ne!(first.connection().base_url, second.connection().base_url);
    assert_ne!(first.identity().nonce, second.identity().nonce);
    let (first_cleanup, second_cleanup) = tokio::join!(first.finish(), second.finish());
    assert!(first_cleanup.is_clean(), "{:?}", first_cleanup.failures);
    assert!(second_cleanup.is_clean(), "{:?}", second_cleanup.failures);
}

#[tokio::test]
async fn invalid_argument_reports_early_exit_with_bounded_diagnostics() {
    let failure = match LiveLabbyBuilder::new()
        .arg("--definitely-invalid-e2e-argument")
        .readiness_deadline(Duration::from_secs(3))
        .start()
        .await
    {
        Ok(guard) => {
            drop(guard.finish().await);
            panic!("invalid argument unexpectedly started")
        }
        Err(failure) => failure,
    };
    assert!(failure.contains("run="));
    assert!(failure.contains("binary_sha256="));
    assert!(failure.contains("stderr_tail="));
    assert!(failure.contains("health_ready_history="));
    assert!(failure.contains("process_inventory="));
}

#[tokio::test]
async fn occupied_port_is_refused_without_touching_the_foreign_listener() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("foreign listener");
    let address = listener.local_addr().unwrap();
    let failure = match LiveLabbyBuilder::new()
        .port(address.port())
        .readiness_deadline(Duration::from_secs(3))
        .start()
        .await
    {
        Ok(guard) => {
            drop(guard.finish().await);
            panic!("occupied port unexpectedly started")
        }
        Err(failure) => failure,
    };
    assert!(failure.contains("exited before readiness") || failure.contains("deadline"));
    assert_eq!(listener.local_addr().unwrap(), address);
}

#[tokio::test]
async fn readiness_timeout_is_bounded_and_cleans_the_spawned_process() {
    let started = Instant::now();
    let failure = match LiveLabbyBuilder::new()
        .ready_path("/does-not-exist")
        .readiness_deadline(Duration::from_millis(500))
        .start()
        .await
    {
        Ok(guard) => {
            drop(guard.finish().await);
            panic!("invalid readiness path unexpectedly passed")
        }
        Err(failure) => failure,
    };
    assert!(failure.contains("readiness deadline exceeded"));
    assert!(started.elapsed() < Duration::from_secs(8));
}

#[tokio::test]
async fn partial_invalid_config_exits_with_sanitized_diagnostics() {
    let canary = "must-not-escape-config-canary";
    let failure = match LiveLabbyBuilder::new()
        .config(format!("[gateway\nsecret = \"{canary}\""))
        .readiness_deadline(Duration::from_secs(3))
        .start()
        .await
    {
        Ok(guard) => {
            drop(guard.finish().await);
            panic!("partial config unexpectedly started")
        }
        Err(failure) => failure,
    };
    assert!(failure.contains("stderr_tail="));
    assert!(!failure.contains(canary));
}

// Migrated from startup_config: the lifecycle guard now owns and evidences the
// exact compiled `labby serve` process whose invalid startup is under test.
#[tokio::test]
async fn adopted_invalid_gateway_config_exits_without_panicking() {
    let failure = match LiveLabbyBuilder::new()
        .config(
            r#"
[[upstream]]
name = "invalid-fixture"
command = "/definitely/not/allowed"
"#,
        )
        .readiness_deadline(Duration::from_secs(3))
        .start()
        .await
    {
        Ok(guard) => {
            drop(guard.finish().await);
            panic!("invalid config must fail startup");
        }
        Err(failure) => failure,
    };
    assert!(failure.contains("loaded gateway config failed validation"));
    assert!(!failure.contains("panicked at"));
}

#[tokio::test]
async fn evidence_disk_failure_does_not_skip_process_cleanup() {
    let guard = LiveLabbyBuilder::new()
        .fail_evidence_writes()
        .start()
        .await
        .expect("live labby starts");
    let root = guard.root().to_path_buf();
    let cleanup = guard.finish().await;
    assert!(
        cleanup
            .failures
            .iter()
            .any(|failure| failure.contains("evidence write failed"))
    );
    assert!(!root.exists(), "cleanup still removes the owned root");
}

#[cfg(unix)]
#[tokio::test]
async fn signal_supervisor_child() {
    let Some(marker) = std::env::var_os("LABBY_E2E_SIGNAL_MARKER") else {
        return;
    };
    let guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("live labby starts");
    let root = guard.root().to_path_buf();
    let marker_path = std::path::PathBuf::from(marker);
    let marker_root = root.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(&marker_path, marker_root.as_os_str().as_encoded_bytes())
            .expect("write signal marker");
    });
    let cleanup = guard.finish_on_supported_signal().await;
    assert!(cleanup.is_clean(), "{:?}", cleanup.failures);
    assert!(!root.exists());
}

#[cfg(unix)]
#[test]
fn sigterm_drives_supervised_cleanup_to_completion() {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("signal-ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "signal_supervisor_child", "--nocapture"])
        .env("LABBY_E2E_SIGNAL_MARKER", &marker)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "signal supervisor exited early"
        );
        assert!(
            Instant::now() < deadline,
            "signal supervisor did not become ready"
        );
        thread::sleep(Duration::from_millis(20));
    }
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap()),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "signal supervisor failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "signal supervisor cleanup hung");
        thread::sleep(Duration::from_millis(20));
    }
    let owned_root =
        std::path::PathBuf::from(String::from_utf8(std::fs::read(marker).unwrap()).unwrap());
    assert!(!owned_root.exists(), "signal cleanup retained owned root");
}
