use super::super::cleanup::take_failed_cleanup_counted;
use super::*;

#[tokio::test]
async fn failed_explicit_remove_gets_bounded_drop_fallback() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let before = ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed);
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let calls = dir.path().join("calls");
    let first_remove = dir.path().join("first-remove");
    let sandbox = dir.path().join("sandbox");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = create ]; then printf '%s\\n' \"$4\" > '{}'; exit 0; fi\nif [ \"$1\" = list ]; then if [ -s '{}' ]; then IFS= read -r name < '{}'; printf '%s\\n' \"$name\"; fi; exit 0; fi\nif [ \"$1\" = remove ] && [ ! -e '{}' ]; then touch '{}'; exit 9; fi\nif [ \"$1\" = remove ]; then rm -f '{}'; exit 0; fi\nexit 0\n",
        calls.display(),
        sandbox.display(),
        sandbox.display(),
        sandbox.display(),
        first_remove.display(),
        first_remove.display(),
        sandbox.display()
    );
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");
    let config = config(executable);
    let (_command, guard) = runner_command(&spawn(), Some(&config), None)
        .await
        .expect("create succeeds");
    let mut guard = guard.expect("guard");
    guard.remove().await;
    drop(guard);

    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let recorded = std::fs::read_to_string(&calls).expect("recorded calls");
        if recorded
            .lines()
            .filter(|line| line.starts_with("remove "))
            .count()
            >= 2
            && ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed) == before
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fallback did not retry: {recorded}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn absent_failed_cleanup_is_reconciled_and_active_count_recovers() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let before = ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed);
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let calls = dir.path().join("calls");
    let name_file = dir.path().join("name");
    let remove_count = dir.path().join("remove-count");
    let script = format!(
        r#"#!/bin/sh
printf '%s
' "$*" >> '{calls}'
if [ "$1" = create ]; then
  prev=
  for arg in "$@"; do
    if [ "$prev" = --name ]; then printf '%s
' "$arg" > '{name}'; break; fi
    prev="$arg"
  done
  exit 0
fi
if [ "$1" = list ]; then
  [ -f '{name}' ] && cat '{name}' || true
  exit 0
fi
if [ "$1" = remove ]; then
  count=0
  [ -f '{count}' ] && count=$(cat '{count}')
  count=$((count + 1))
  printf '%s
' "$count" > '{count}'
  if [ "$count" -le 2 ]; then rm -f '{name}'; exit 9; fi
  rm -f '{name}'
fi
exit 0
"#,
        calls = calls.display(),
        name = name_file.display(),
        count = remove_count.display(),
    );
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");
    let config = config(executable);
    let admission = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let permit = admission
        .clone()
        .acquire_owned()
        .await
        .expect("test permit");
    let (_command, guard) = runner_command(&spawn(), Some(&config), Some(permit))
        .await
        .expect("initial create succeeds");
    let mut guard = guard.expect("guard");
    guard.remove().await;
    drop(guard);

    // The fallback logs removal before its helper exits. Its admission
    // permit is released only after the worker records the failed cleanup,
    // so wait for that lifecycle boundary before creating a replacement.
    let _completed = tokio::time::timeout(Duration::from_secs(3), admission.acquire_owned())
        .await
        .expect("fallback cleanup worker must finish")
        .expect("test admission semaphore remains open");
    let removes = std::fs::read_to_string(&calls)
        .expect("recorded calls")
        .lines()
        .filter(|line| line.starts_with("remove "))
        .count();
    assert_eq!(removes, 2, "explicit and fallback removals must finish");
    assert_eq!(
        ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed),
        before + 1,
        "failed counted cleanup must remain accounted until absence is proven"
    );

    let (_command, guard) = runner_command(&spawn(), Some(&config), None)
        .await
        .expect("proven-absent failed cleanup should reopen creation");
    let mut guard = guard.expect("replacement guard");
    guard.remove().await;
    assert_eq!(
        ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed),
        before,
        "reconciliation plus replacement cleanup must restore active accounting"
    );
}

#[tokio::test]
async fn live_failed_cleanup_remains_fail_closed_until_removal_is_proven() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let before = ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed);
    let dir = tempfile::tempdir().expect("tempdir");
    let executable = dir.path().join("msb");
    let calls = dir.path().join("calls");
    let name_file = dir.path().join("name");
    let allow_remove = dir.path().join("allow-remove");
    let script = format!(
        r#"#!/bin/sh
printf '%s
' "$*" >> '{calls}'
if [ "$1" = create ]; then
  prev=
  for arg in "$@"; do
    if [ "$prev" = --name ]; then printf '%s
' "$arg" > '{name}'; break; fi
    prev="$arg"
  done
  exit 0
fi
if [ "$1" = list ]; then
  [ -f '{name}' ] && cat '{name}' || true
  exit 0
fi
if [ "$1" = remove ]; then
  if [ -f '{allow}' ]; then rm -f '{name}'; exit 0; fi
  exit 9
fi
exit 0
"#,
        calls = calls.display(),
        name = name_file.display(),
        allow = allow_remove.display(),
    );
    std::fs::write(&executable, script).expect("write fake msb");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");
    let config = config(executable);
    let (_command, guard) = runner_command(&spawn(), Some(&config), None)
        .await
        .expect("initial create succeeds");
    let mut guard = guard.expect("guard");
    guard.remove().await;
    drop(guard);

    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let removes = std::fs::read_to_string(&calls)
            .expect("recorded calls")
            .lines()
            .filter(|line| line.starts_with("remove "))
            .count();
        if removes >= 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            break;
        }
        assert!(Instant::now() < deadline, "fallback remove did not run");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let error = runner_command(&spawn(), Some(&config), None)
        .await
        .err()
        .expect("live unresolved guest must keep creation closed");
    assert_eq!(error.kind(), "cleanup_failed");
    assert!(error.to_string().contains("cleanup(s) remain unresolved"));

    std::fs::write(&allow_remove, "ok").expect("allow cleanup");
    reconcile_failed_cleanups(&config.executable)
        .await
        .expect("later proven removal should clear the circuit");
    assert_eq!(
        ACTIVE_SANDBOXES.load(std::sync::atomic::Ordering::Relaxed),
        before
    );
}

#[tokio::test]
async fn failed_cleanup_ledger_transfers_counted_ownership_once() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let dir = tempfile::tempdir().expect("tempdir");
    let identity = CleanupIdentity {
        executable: dir.path().join("msb"),
        name: "labby-codemode-ledger-once".to_string(),
    };
    cleanup_failed(&identity, true);

    assert!(take_failed_cleanup_counted(&identity));
    assert!(
        !take_failed_cleanup_counted(&identity),
        "a concurrent resolver must not acquire counted ownership twice"
    );
}

#[tokio::test]
async fn failed_cleanup_circuit_is_scoped_to_executable() {
    let _state = stateful_test_guard().await;
    relax_helper_timeout();
    let dir = tempfile::tempdir().expect("tempdir");
    let executable_a = dir.path().join("msb-a");
    let executable_b = dir.path().join("msb-b");
    let identity = CleanupIdentity {
        executable: executable_b.clone(),
        name: "labby-codemode-1-1".to_string(),
    };
    cleanup_failed(&identity, false);

    ensure_cleanup_circuit_closed(&executable_a)
        .expect("another executable must not inherit the failed cleanup");
    assert_eq!(
        ensure_cleanup_circuit_closed(&executable_b)
            .expect_err("matching executable must remain blocked")
            .kind(),
        "cleanup_failed"
    );
    resolve_failed_cleanup(&identity);
}
