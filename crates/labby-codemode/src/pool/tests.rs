use super::config::PoolConfig;
use super::*;

fn cfg(size: usize, recycle_after: u64, max_overflow: usize) -> PoolConfig {
    PoolConfig {
        size,
        recycle_after,
        max_overflow,
    }
}

#[tokio::test]
async fn cancelled_admission_restores_pooled_overflow_and_fresh_reservations() {
    // Hold the real spawn admission gate, so cancellation happens after
    // checkout reserves bookkeeping but before any subprocess is launched.
    let admission = microsandbox_admission();
    let _held = admission
        .acquire_many(config::microsandbox_max_runners() as u32)
        .await
        .unwrap();
    for (size, fresh) in [(1, false), (0, false), (1, true)] {
        let pool = RunnerPool::with_config_and_spawn(
            cfg(size, 100, 1),
            RunnerSpawn {
                program: "unused".into(),
                args: vec![],
            },
            Some(MicrosandboxSpawn {
                executable: "unused".into(),
                image: "unused".into(),
            }),
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut checkout = Box::pin(async {
            if fresh {
                pool.checkout_fresh(deadline).await
            } else {
                pool.checkout(deadline).await
            }
        });
        assert!(futures::poll!(&mut checkout).is_pending());
        assert_eq!(pool.active_leases.load(Ordering::Acquire), 1);
        drop(checkout);
        assert_eq!(pool.active_leases.load(Ordering::Acquire), 0);
        assert_eq!(pool.state.lock().unwrap().free_slots.len(), size);
        assert_eq!(pool.available_overflow_permits(), 1);
        tokio::time::timeout(std::time::Duration::from_millis(100), pool.shutdown())
            .await
            .expect("cancelled admission must not strand shutdown");
    }
}

#[tokio::test]
async fn failed_spawn_returns_each_admission_reservation_once() {
    let temp = tempfile::tempdir().unwrap();
    for (size, fresh) in [(1, false), (0, false), (1, true)] {
        let pool = RunnerPool::with_config_and_spawn(
            cfg(size, 100, 1),
            RunnerSpawn {
                program: temp.path().join("missing-runner"),
                args: vec![],
            },
            None,
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let result = if fresh {
            pool.checkout_fresh(deadline).await
        } else {
            pool.checkout(deadline).await
        };
        assert!(result.is_err());
        assert_eq!(pool.active_leases.load(Ordering::Acquire), 0);
        assert_eq!(pool.state.lock().unwrap().free_slots.len(), size);
        assert_eq!(pool.available_overflow_permits(), 1);
        tokio::time::timeout(std::time::Duration::from_millis(100), pool.shutdown())
            .await
            .unwrap();
    }
}

/// A pooled runner returned via `release` is parked in its slot and the SAME
/// process is handed out on the next checkout (PID reuse) — the core warm
/// behavior. Crucially, the parent-side bookkeeping reuses the process
/// rather than spawning a fresh one each time.
#[tokio::test]
async fn pool_reuses_the_same_process_across_checkouts() {
    let pool = RunnerPool::with_config(cfg(1, 100, 4));
    let mut lease = pool.checkout_stub().await.expect("checkout");
    assert!(lease.is_pooled(), "size>0 → pooled lease");
    let first_pid = lease.runner_mut().child_pid;
    lease.release().await;

    let mut lease2 = pool.checkout_stub().await.expect("re-checkout");
    let second_pid = lease2.runner_mut().child_pid;
    assert_eq!(
        first_pid, second_pid,
        "a released pooled runner must be reused (same PID) on the next checkout"
    );
    lease2.release().await;
}

#[tokio::test]
async fn retired_microsandbox_lease_is_drained_after_the_response() {
    let pool = RunnerPool::with_config_and_spawn(
        cfg(1, 10, 1),
        RunnerSpawn::try_default().unwrap(),
        Some(MicrosandboxSpawn {
            executable: "fixture-msb".into(),
            image: "fixture".into(),
        }),
    );
    let lease = pool.checkout_stub().await.unwrap();
    pool.finish_response_lease(lease).await;
    // This current-thread test has not yielded to the retirement task.
    assert_eq!(pool.active_leases.load(Ordering::Acquire), 1);
    assert!(pool.state.lock().unwrap().free_slots.is_empty());
    pool.shutdown().await;
    assert_eq!(pool.active_leases.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn explicit_release_returns_one_slot_index() {
    let pool = RunnerPool::with_config(cfg(1, 100, 1));
    let lease = pool.checkout_stub().await.expect("checkout");
    lease.release().await;
    assert_eq!(
        pool.state.lock().expect("pool state").free_slots.len(),
        1,
        "explicit finalization and Drop must not publish the slot twice"
    );
    pool.shutdown().await;
}

/// Evicting a runner kills it and leaves the slot empty; the next checkout
/// spawns a FRESH process (different PID) — crash/fault recovery.
#[tokio::test]
async fn pool_respawns_after_eviction() {
    let pool = RunnerPool::with_config(cfg(1, 100, 4));
    let mut lease = pool.checkout_stub().await.expect("checkout");
    let first_pid = lease.runner_mut().child_pid;
    // Simulate a crash/fault: evict instead of release.
    lease.evict().await;

    let mut lease2 = pool.checkout_stub().await.expect("re-checkout");
    let second_pid = lease2.runner_mut().child_pid;
    assert_ne!(
        first_pid, second_pid,
        "an evicted runner must be replaced by a freshly spawned process"
    );
    lease2.release().await;
}

/// After K executions a pooled runner is recycled: the next checkout spawns a
/// fresh process. With `recycle_after = 2`, the 3rd checkout must see a new PID.
#[tokio::test]
async fn pool_recycles_runner_after_k_executions() {
    let pool = RunnerPool::with_config(cfg(1, 2, 4));
    let mut l1 = pool.checkout_stub().await.expect("checkout 1");
    let pid1 = l1.runner_mut().child_pid;
    l1.release().await; // executions: 1

    let mut l2 = pool.checkout_stub().await.expect("checkout 2");
    let pid2 = l2.runner_mut().child_pid;
    assert_eq!(pid1, pid2, "same runner before the recycle threshold");
    l2.release().await; // executions: 2 → hits recycle_after, runner dropped

    let mut l3 = pool.checkout_stub().await.expect("checkout 3");
    let pid3 = l3.runner_mut().child_pid;
    assert_ne!(
        pid2, pid3,
        "runner must be recycled (fresh PID) after K executions"
    );
    l3.release().await;
}

/// Concurrency: N pooled slots serve N simultaneous leases with N distinct
/// processes (they do not serialize onto one runner). All three leases are
/// acquired and held at the same time, so the pool cannot have handed any
/// pair the same process.
#[tokio::test]
async fn pool_serves_concurrent_leases_with_distinct_runners() {
    let pool = RunnerPool::with_config(cfg(3, 100, 4));
    let a = pool.checkout_stub().await.expect("a");
    let b = pool.checkout_stub().await.expect("b");
    let c = pool.checkout_stub().await.expect("c");
    let mut leases = [a, b, c];
    assert!(
        leases.iter().all(RunnerLease::is_pooled),
        "all three concurrent leases must be served from pooled slots, not overflow"
    );
    let pids: std::collections::HashSet<_> = leases
        .iter_mut()
        .map(|l| l.runner_mut().child_pid)
        .collect();
    assert_eq!(
        pids.len(),
        3,
        "three concurrent pooled leases must hold three distinct processes \
         (N runners serve N concurrent executions without serializing)"
    );
    for l in leases {
        l.release().await;
    }
}

/// Fallback parity: the kill switch (`size == 0`) makes every checkout
/// spawn-per-execution (ephemeral, distinct PIDs, never reused), exactly the
/// pre-pool behavior. With pooling enabled, the same process is reused. This
/// asserts the only behavioral difference the switch introduces is reuse —
/// both paths hand out a working, live runner lease.
#[tokio::test]
async fn kill_switch_matches_spawn_per_execution_behavior() {
    // Disabled: each checkout is a fresh process.
    let disabled = RunnerPool::with_config(cfg(0, 100, 8));
    let mut d1 = disabled.checkout_stub().await.expect("d1");
    let d1_pid = d1.runner_mut().child_pid;
    d1.release().await;
    let mut d2 = disabled.checkout_stub().await.expect("d2");
    let d2_pid = d2.runner_mut().child_pid;
    d2.release().await;
    assert_ne!(
        d1_pid, d2_pid,
        "kill switch must spawn a fresh process per execution (no reuse)"
    );

    // Enabled: the process is reused.
    let enabled = RunnerPool::with_config(cfg(1, 100, 8));
    let mut e1 = enabled.checkout_stub().await.expect("e1");
    let e1_pid = e1.runner_mut().child_pid;
    e1.release().await;
    let mut e2 = enabled.checkout_stub().await.expect("e2");
    let e2_pid = e2.runner_mut().child_pid;
    e2.release().await;
    assert_eq!(e1_pid, e2_pid, "enabled pool reuses the process");
}

/// When all pooled slots are busy, an extra checkout is served by a bounded
/// ephemeral (overflow) runner rather than blocking forever or growing
/// unbounded.
#[tokio::test]
async fn pool_overflows_to_ephemeral_runner_when_saturated() {
    let pool = RunnerPool::with_config(cfg(1, 100, 2));
    let held = pool.checkout_stub().await.expect("hold the only slot");
    assert!(held.is_pooled());

    // Pool is saturated (1 slot, held). The next checkout must still succeed
    // via an ephemeral runner.
    let overflow = pool.checkout_stub().await.expect("overflow checkout");
    assert!(
        !overflow.is_pooled(),
        "a saturated pool must serve overflow via an ephemeral (non-pooled) runner"
    );
    overflow.release().await; // ephemeral → dropped
    held.release().await;
}

/// The kill switch (`size == 0`) disables pooling: every checkout is an
/// ephemeral runner, matching the spawn-per-execution fallback.
#[tokio::test]
async fn pool_disabled_serves_only_ephemeral_runners() {
    let pool = RunnerPool::with_config(cfg(0, 100, 2));
    assert!(pool.config().is_disabled());
    assert_eq!(
        pool.available_overflow_permits(),
        2,
        "disabled pooling must honor configured overflow concurrency"
    );
    let mut a = pool.checkout_stub().await.expect("a");
    let mut b = pool.checkout_stub().await.expect("b");
    assert!(!a.is_pooled(), "disabled pool → ephemeral lease");
    assert!(!b.is_pooled(), "disabled pool → ephemeral lease");
    assert_ne!(
        a.runner_mut().child_pid,
        b.runner_mut().child_pid,
        "each disabled-pool checkout spawns a distinct process"
    );
    a.release().await;
    b.release().await;
}

#[tokio::test]
async fn disabled_pool_respects_low_overflow_limit() {
    let pool = RunnerPool::with_config(cfg(0, 100, 1));
    assert!(pool.config().is_disabled());
    assert_eq!(
        pool.available_overflow_permits(),
        1,
        "disabled pooling must not inflate an explicit low overflow limit"
    );
}

#[tokio::test]
async fn disabled_pool_raises_zero_overflow_to_one_permit() {
    let pool = RunnerPool::with_config(cfg(0, 100, 0));
    assert!(pool.config().is_disabled());
    assert_eq!(
        pool.available_overflow_permits(),
        1,
        "zero overflow is raised only enough to avoid deadlock"
    );
}

/// Dropping a lease without finalizing is fail-safe: the slot index returns
/// to the free-list so the pool is not permanently starved, and the runner is
/// killed (a fresh one spawns next checkout).
#[tokio::test]
async fn dropping_a_lease_returns_the_slot_and_evicts_the_runner() {
    let pool = RunnerPool::with_config(cfg(1, 100, 4));
    let first_pid = {
        let mut lease = pool.checkout_stub().await.expect("checkout");
        let pid = lease.runner_mut().child_pid;
        // Drop without release/evict (e.g. an early `?`).
        drop(lease);
        pid
    };
    // The slot must be reusable, and the runner replaced.
    let mut lease2 = pool
        .checkout_stub()
        .await
        .expect("slot reusable after drop");
    assert_ne!(
        first_pid,
        lease2.runner_mut().child_pid,
        "dropped lease must evict the runner (fresh PID next checkout)"
    );
    lease2.release().await;
}

#[tokio::test]
async fn shutdown_waits_for_leases_and_drains_parked_slots() {
    let pool = Arc::new(RunnerPool::with_config(cfg(1, 100, 1)));
    let lease = pool.checkout_stub().await.expect("checkout");
    let shutdown_pool = Arc::clone(&pool);
    let shutdown = tokio::spawn(async move { shutdown_pool.shutdown().await });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(
        !shutdown.is_finished(),
        "shutdown must wait for active leases"
    );
    lease.release().await;
    shutdown.await.expect("shutdown task");

    assert!(
        pool.slots[0].lock().await.is_none(),
        "parked runner drained"
    );
    let err = pool.checkout_stub().await.err().expect("pool is closed");
    assert!(err.to_string().contains("shutting down"));
}

#[cfg(not(windows))]
#[tokio::test]
async fn checkout_deadline_includes_overflow_semaphore_wait() {
    let pool = RunnerPool::with_config_and_spawn(
        cfg(0, 100, 1),
        RunnerSpawn {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                r#"read hello; echo '{"type":"protocol_version","version":1}'; cat"#.into(),
            ],
        },
        None,
    );
    let first = pool
        .checkout(tokio::time::Instant::now() + std::time::Duration::from_secs(2))
        .await
        .expect("first checkout");
    let err = pool
        .checkout(tokio::time::Instant::now() + std::time::Duration::from_millis(30))
        .await
        .err()
        .expect("second checkout must time out waiting for admission");
    assert_eq!(err.kind(), "queue_saturated");
    assert_eq!(err.to_agent_value()["side_effects"], "none_expected");
    first.release().await;
    pool.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn replaced_external_runner_must_confirm_the_protocol_before_receiving_source() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("runner");
    let install = |version: u32| {
        let staged = dir.path().join("new");
        std::fs::write(&staged, format!("#!/bin/sh\nread hello\nprintf '%s\\n' '{{\"type\":\"protocol_version\",\"version\":{version}}}'\ncat\n")).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::rename(staged, &executable).unwrap();
    };
    install(crate::protocol::RUNNER_PROTOCOL_VERSION);
    let pool = RunnerPool::with_test_spawn(
        cfg(0, 1, 1),
        RunnerSpawn {
            program: executable.clone(),
            args: Vec::new(),
        },
    );
    let deadline = || tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    pool.checkout(deadline()).await.unwrap().release().await;
    install(0);
    assert_eq!(
        pool.checkout(deadline()).await.err().unwrap().kind(),
        "runner_version_mismatch"
    );
    install(crate::protocol::RUNNER_PROTOCOL_VERSION);
    pool.checkout(deadline()).await.unwrap().release().await;
    pool.shutdown().await;
}
