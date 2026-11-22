#[test]
fn the_running_process_reads_as_alive() {
    // The regression this pins is data destruction: when this returned
    // false for a live owner, reconciliation deleted sandboxes belonging to
    // running processes — this one included.
    assert!(owner_process_is_alive(std::process::id()));
}

#[test]
fn a_pid_outside_the_platform_range_is_proof_of_death() {
    // The one arm that returns `false` without an `ESRCH` from the kernel.
    // A pid that cannot be represented cannot name a live process, so
    // treating it as dead is proof rather than a guess.
    assert!(!owner_process_is_alive(
        u32::try_from(i32::MAX).expect("i32::MAX fits in u32") + 1
    ));
    assert!(!owner_process_is_alive(u32::MAX));
}

#[test]
fn an_unprovable_pid_fails_closed_as_alive() {
    // pid 1 always exists and, unprivileged, `kill(1, 0)` answers EPERM
    // rather than ESRCH. An owner we cannot prove dead must keep its
    // sandbox, so EPERM must read as alive.
    assert!(owner_process_is_alive(1));
}

use std::os::unix::fs::PermissionsExt as _;

use super::reconciliation::{owner_process_is_alive, sandbox_owner_pid};
use super::*;

/// Raise the helper bound for timing-sensitive tests.
///
/// The real 5 s default is a wall-clock bound: when a loaded host delays
/// the fake helper past it, the test fails for scheduling reasons rather
/// than behavior. Tests that assert helper *outcomes* use a generous bound
/// so only the behavior under test can fail them.
fn relax_helper_timeout() {
    let _ = HELPER_TIMEOUT.set(Some(Duration::from_mins(2)));
}

#[test]
fn helper_timeout_defaults_and_is_operator_overridable() {
    // The default is what a fresh process uses; the override is read once.
    assert_eq!(DEFAULT_HELPER_TIMEOUT, Duration::from_secs(5));
    assert_eq!(
        HELPER_TIMEOUT_ENV,
        "LABBY_CODE_MODE_MICROSANDBOX_HELPER_TIMEOUT_MS"
    );
    // `helper_timeout(DEFAULT_HELPER_TIMEOUT)` latches the first value, which `relax_helper_timeout`
    // uses to keep timing-sensitive tests from racing the host.
    relax_helper_timeout();
    assert_eq!(
        helper_timeout(DEFAULT_HELPER_TIMEOUT),
        Duration::from_mins(2)
    );
}

async fn stateful_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[test]
fn sandbox_names_carry_parseable_process_ownership() {
    assert_eq!(sandbox_owner_pid("labby-codemode-123-9"), Some(123));
    assert_eq!(sandbox_owner_pid("foreign-123-9"), None);
    assert_eq!(sandbox_owner_pid("labby-codemode-bad-9"), None);
}

#[tokio::test]
async fn reconciliation_removes_dead_owner_and_preserves_live_owner() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let calls = dir.path().join("calls");
    let live = format!("labby-codemode-{}-7", std::process::id());
    let dead = "labby-codemode-4294967294-8";
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = list ]; then printf '%s\\n' '{}' '{}'; fi\nexit 0\n",
        calls.display(),
        live,
        dead
    );
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");

    let config = config(executable);
    let (_command, guard) = runner_command(&spawn(), Some(&config), None)
        .await
        .expect("reconciliation and create succeed");
    let calls = std::fs::read_to_string(&calls).expect("recorded calls");
    assert!(
        calls
            .lines()
            .any(|line| line == format!("remove --quiet --force {dead}")),
        "dead-owner sandbox was not removed: {calls}"
    );
    assert!(
        !calls
            .lines()
            .any(|line| line.contains(&format!("force {live}"))),
        "live-owner sandbox was removed: {calls}"
    );
    let mut guard = guard.expect("created guard");
    guard.remove().await;
}

fn fake_msb(create_status: i32) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let calls = dir.path().join("calls");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = create ]; then exit {create_status}; fi\nexit 0\n",
        calls.display()
    );
    std::fs::write(&executable, script).expect("write fake msb");
    let mut permissions = std::fs::metadata(&executable)
        .expect("fake msb metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).expect("make fake msb executable");
    (dir, executable, calls)
}

fn spawn() -> RunnerSpawn {
    RunnerSpawn {
        program: "/bin/true".into(),
        args: vec!["internal".into(), "code-mode-runner".into()],
    }
}

fn config(executable: std::path::PathBuf) -> super::super::MicrosandboxSpawn {
    super::super::MicrosandboxSpawn {
        executable,
        image: "debian".into(),
    }
}

#[tokio::test]
async fn reconciliation_caches_failure_and_retries_after_cooldown() {
    let (_dir, executable, calls) = fake_msb(0);
    std::fs::write(
        &executable,
        format!("#!/bin/sh\necho list >> '{}'\nexit 1\n", calls.display()),
    )
    .unwrap();
    let cfg = config(executable.clone());
    for _ in 0..2 {
        assert_eq!(
            reconcile_stale_sandboxes(&cfg).await.unwrap_err().kind(),
            "cleanup_failed"
        );
    }
    assert_eq!(std::fs::read_to_string(&calls).unwrap(), "list\n");
    let mut cache = RECONCILED_EXECUTABLES.get().unwrap().lock().await;
    let (failed_at, _) = cache.get_mut(&executable).unwrap().as_mut().unwrap();
    *failed_at -= Duration::from_secs(10);
    drop(cache);
    assert!(reconcile_stale_sandboxes(&cfg).await.is_err());
    assert_eq!(std::fs::read_to_string(calls).unwrap(), "list\nlist\n");
}

#[tokio::test]
async fn failed_create_attempts_bounded_cleanup() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let (_dir, executable, calls) = fake_msb(23);
    let config = config(executable);
    let error = runner_command(&spawn(), Some(&config), None)
        .await
        .err()
        .expect("create must fail");
    assert_eq!(error.kind(), "internal_error");
    let calls = std::fs::read_to_string(calls).expect("recorded calls");
    assert!(calls.lines().any(|line| line.starts_with("create ")));
    assert!(
        calls
            .lines()
            .any(|line| line.starts_with("remove --quiet --force labby-codemode-")),
        "failed creation must remove its named sandbox: {calls}"
    );
}

#[tokio::test]
async fn cleanup_circuit_rejection_does_not_cleanup_never_created_sandbox() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let calls = dir.path().join("calls");
    let unresolved_name = "labby-codemode-existing-leak";
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = list ]; then printf '%s\\n' '{}'; exit 0; fi\nif [ \"$1\" = remove ]; then exit 9; fi\nif [ \"$1\" = create ]; then exit 0; fi\nexit 0\n",
        calls.display(),
        unresolved_name
    );
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");
    let config = config(executable.clone());
    let unresolved = CleanupIdentity {
        executable,
        name: unresolved_name.to_string(),
    };
    cleanup_failed(&unresolved, false);

    let error = runner_command(&spawn(), Some(&config), None)
        .await
        .err()
        .expect("creation must fail closed");
    assert_eq!(error.kind(), "cleanup_failed");

    let recorded = std::fs::read_to_string(&calls).expect("recorded calls");
    assert!(
        !recorded.lines().any(|line| line.starts_with("create ")),
        "circuit rejection must happen before a new create: {recorded}"
    );
    let removes = recorded
        .lines()
        .filter(|line| line.starts_with("remove "))
        .collect::<Vec<_>>();
    assert_eq!(
        removes.len(),
        1,
        "existing leak should get one bounded retry: {recorded}"
    );
    assert!(
        removes[0].ends_with(unresolved_name),
        "retry must target only the existing leak, not a never-created sandbox: {recorded}"
    );
    resolve_failed_cleanup(&unresolved);
}

#[tokio::test]
async fn failed_create_with_absent_sandbox_does_not_poison_cleanup_circuit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let calls = dir.path().join("calls");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = create ]; then exit 23; fi\nif [ \"$1\" = remove ]; then exit 9; fi\nexit 0\n",
        calls.display()
    );
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");
    let config = config(executable);

    for _ in 0..2 {
        let error = runner_command(&spawn(), Some(&config), None)
            .await
            .err()
            .expect("create must fail");
        assert_eq!(
            error.kind(),
            "internal_error",
            "confirmed absence must not leave cleanup circuit debt"
        );
    }

    let recorded = std::fs::read_to_string(calls).expect("recorded calls");
    assert_eq!(
        recorded
            .lines()
            .filter(|line| line.starts_with("create "))
            .count(),
        2,
        "second create should not be blocked by phantom cleanup debt: {recorded}"
    );
}

#[tokio::test]
async fn failed_create_preserves_stdout_and_stderr_diagnostics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let script = "#!/bin/sh\nif [ \"$1\" = create ]; then printf 'backend-detail'; printf 'stderr-detail' >&2; exit 23; fi\nexit 0\n";
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");

    let error = runner_command(&spawn(), Some(&config(executable)), None)
        .await
        .err()
        .expect("create must fail");
    let message = error.to_string();
    assert!(message.contains("stderr: stderr-detail"), "{message}");
    assert!(message.contains("stdout: backend-detail"), "{message}");
}

#[tokio::test]
async fn oversized_helper_output_is_drained_but_diagnostic_is_bounded() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let script = "#!/bin/sh\nif [ \"$1\" = create ]; then head -c 1048576 /dev/zero | tr '\\0' x >&2; exit 23; fi\nexit 0\n";
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");

    let error = runner_command(&spawn(), Some(&config(executable)), None)
        .await
        .err()
        .expect("create must fail");
    let message = error.to_string();
    assert!(message.len() < 2_048, "diagnostic was not bounded");
    assert!(
        message.contains("failed to create Microsandbox"),
        "{message}"
    );
}

#[tokio::test]
async fn successful_create_returns_stream_command_and_async_guard() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let (_dir, executable, calls) = fake_msb(0);
    let config = config(executable);
    let (command, guard) = runner_command(&spawn(), Some(&config), None)
        .await
        .expect("create succeeds");
    let debug = format!("{command:?}");
    assert!(debug.contains("--stream"));
    assert!(debug.contains("/opt/labby/labby"));
    let mut guard = guard.expect("Microsandbox guard");
    guard.remove().await;
    let calls = std::fs::read_to_string(calls).expect("recorded calls");
    let create = calls
        .lines()
        .find(|line| line.starts_with("create "))
        .expect("create call");
    for required in [
        "--security restricted",
        "--no-net",
        "--pull never",
        "--cpus 1",
        "--memory 256M",
        "--max-duration 24h",
        ":/opt/labby/labby:ro,nodev,nosuid",
    ] {
        assert!(
            create.contains(required),
            "missing `{required}` in: {create}"
        );
    }
    assert!(calls.lines().any(|line| line.starts_with("remove ")));
}

mod cleanup;

#[tokio::test]
async fn terminal_error_returns_before_slow_remove_and_refills_before_checkout() {
    use crate::pool::{RunnerPool, config::PoolConfig};
    use crate::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput};
    use futures::StreamExt as _;

    let _state = stateful_test_guard().await;
    let (dir, executable, calls) = fake_msb(0);
    let script = format!(
        r#"#!/bin/sh
echo "$1" >> '{}'
case "$1" in
  list) echo '[]';;
  remove) sleep 1;;
  exec)
    read hello
    echo '{{"type":"protocol_version","version":1}}'
    read start
    echo '{{"type":"error","kind":"server_error","message":"fixture execution error"}}'
    cat >/dev/null;;
esac
"#,
        calls.display()
    );
    std::fs::write(&executable, script).unwrap();
    let pool = RunnerPool::with_config_and_spawn(
        PoolConfig {
            size: 1,
            max_overflow: 1,
            recycle_after: 10,
        },
        spawn(),
        Some(config(executable)),
    );
    let deadline = || tokio::time::Instant::now() + Duration::from_secs(5);
    let mut lease = pool.checkout(deadline()).await.unwrap();
    let runner = lease.runner_mut();
    crate::runner_io::write_runner_input(
        runner.stdin.as_mut().unwrap(),
        &CodeModeRunnerInput::Start {
            code: String::new(),
            proxy: String::new(),
        },
    )
    .await
    .unwrap();
    let frame = runner.lines.next().await.unwrap().unwrap();
    assert!(matches!(
        serde_json::from_str::<CodeModeRunnerOutput>(&frame).unwrap(),
        CodeModeRunnerOutput::Error { .. }
    ));
    tokio::time::timeout(
        Duration::from_millis(100),
        pool.finish_response_lease(lease),
    )
    .await
    .unwrap();
    let next = pool.checkout(deadline()).await.unwrap();
    assert!(
        next.is_pooled(),
        "checkout waited for the already scheduled refill"
    );
    let created = || {
        std::fs::read_to_string(&calls)
            .unwrap()
            .lines()
            .filter(|line| *line == "create")
            .count()
    };
    assert_eq!(created(), 2);

    next.evict().await;
    pool.shutdown().await;
    drop(dir);
}
