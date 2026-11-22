use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit};

use super::{PoolConfig, PoolState, PooledRunner, RunnerPool, shutdown_runner};

impl RunnerPool {
    /// An evicted runner and its replacement share the existing lease. Shutdown
    /// waits for the lease, and a slot becomes available only after refill.
    pub(crate) async fn finish_response_lease(&self, mut lease: RunnerLease) {
        let runner = lease.runner.as_ref().expect("completed lease has a runner");
        if runner.reuse_ready
            && matches!(&lease.kind, LeaseKind::Pooled { recycle_after, .. }
            if runner.executions.saturating_add(1) < *recycle_after)
        {
            lease.release().await;
            return;
        }
        if let LeaseKind::Pooled { index, state, .. } = &lease.kind {
            state
                .lock()
                .expect("pool state lock")
                .refilling
                .insert(*index);
        }
        let spawn = self.spawn.clone();
        let microsandbox = self.microsandbox.clone();
        drop(tokio::spawn(async move {
            if let Some(retired) = lease.runner.take() {
                shutdown_runner(retired, "pool.retire").await;
            }
            if let LeaseKind::Pooled { slot, state, .. } = &lease.kind
                && !state.lock().expect("pool state lock").shutting_down
            {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
                match super::spawn_before_deadline(&spawn, microsandbox.as_ref(), deadline).await {
                    Ok(replacement) => *slot.lock().await = Some(replacement),
                    Err(error) => {
                        tracing::warn!(kind = error.kind(), "background runner refill failed")
                    }
                }
            }
            lease.evict().await;
        }));
    }
}

/// Exclusive lease on one runner for a single execution.
///
/// On `release` a pooled runner is returned to its slot (unless it hit the
/// recycle threshold); an ephemeral runner is simply dropped (killed). On
/// `evict` the runner is always dropped and a pooled slot is left empty to be
/// respawned on next checkout. Dropping the lease without finalizing also
/// evicts (the runner is dropped and the slot left empty) — fail-safe.
pub(crate) struct RunnerLease {
    kind: LeaseKind,
    pub(super) runner: Option<PooledRunner>,
}

enum LeaseKind {
    Pooled {
        slot: Arc<Mutex<Option<PooledRunner>>>,
        state: Arc<StdMutex<PoolState>>,
        active_leases: Arc<AtomicUsize>,
        drained: Arc<Notify>,
        index: usize,
        recycle_after: u64,
        /// Set once the slot index has been returned to the free-list so neither
        /// an explicit finalize nor the `Drop` fail-safe pushes it twice.
        returned: bool,
    },
    Ephemeral {
        _permit: OwnedSemaphorePermit,
        active_leases: Arc<AtomicUsize>,
        drained: Arc<Notify>,
        returned: bool,
    },
}

impl RunnerLease {
    pub(super) fn pooled(
        slot: Arc<Mutex<Option<PooledRunner>>>,
        state: Arc<StdMutex<PoolState>>,
        active_leases: Arc<AtomicUsize>,
        drained: Arc<Notify>,
        index: usize,
        runner: Option<PooledRunner>,
        config: PoolConfig,
    ) -> Self {
        Self {
            kind: LeaseKind::Pooled {
                slot,
                state,
                active_leases,
                drained,
                index,
                recycle_after: config.recycle_after,
                returned: false,
            },
            runner,
        }
    }

    pub(super) fn ephemeral(
        runner: Option<PooledRunner>,
        permit: OwnedSemaphorePermit,
        active_leases: Arc<AtomicUsize>,
        drained: Arc<Notify>,
    ) -> Self {
        Self {
            kind: LeaseKind::Ephemeral {
                _permit: permit,
                active_leases,
                drained,
                returned: false,
            },
            runner,
        }
    }

    /// Mutable access to the leased runner for driving one execution.
    pub(crate) fn runner_mut(&mut self) -> &mut PooledRunner {
        self.runner
            .as_mut()
            .expect("runner present for the lease lifetime")
    }

    /// Whether this lease is backed by a pooled (long-lived, reused) runner.
    #[cfg(test)]
    pub(crate) fn is_pooled(&self) -> bool {
        matches!(self.kind, LeaseKind::Pooled { .. })
    }

    /// Return the runner after a clean execution.
    ///
    /// A pooled runner is parked back in its slot unless it has reached the
    /// recycle-after-K threshold, in which case it is dropped (killed) and the
    /// slot left empty so the next checkout spawns a fresh process. Either way
    /// the slot index returns to the free-list. An ephemeral runner is dropped.
    pub(crate) async fn release(mut self) {
        let runner = self.runner.take();
        match (&mut self.kind, runner) {
            (
                LeaseKind::Pooled {
                    slot,
                    state,
                    active_leases,
                    drained,
                    index,
                    recycle_after,
                    returned,
                },
                Some(mut runner),
            ) => {
                runner.executions = runner.executions.saturating_add(1);
                if runner.executions >= *recycle_after {
                    tracing::debug!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "pool.recycle",
                        executions = runner.executions,
                        "recycling pooled Code Mode runner after threshold"
                    );
                    // Drop (kill) the runner; leave the slot empty to respawn.
                    shutdown_runner(runner, "pool.recycle").await;
                } else {
                    *slot.lock().await = Some(runner);
                }
                return_slot(state, *index, drained);
                finish_lease(active_leases, drained, returned);
            }
            // Ephemeral runner, or pooled with no runner (already taken): drop.
            (_, Some(runner)) => shutdown_runner(runner, "pool.release_ephemeral").await,
            (_, None) => {}
        }
        self.finish();
    }

    /// Discard the runner after a crash, timeout, or protocol fault.
    ///
    /// The runner is always dropped (killed); a pooled slot is left empty (the
    /// runner is never returned) so the next checkout spawns a fresh replacement.
    /// The slot index still returns to the free-list so the slot is reusable.
    pub(crate) async fn evict(mut self) {
        if let Some(runner) = self.runner.take() {
            shutdown_runner(runner, "pool.evict").await;
        }
        if let LeaseKind::Pooled {
            state,
            drained,
            index,
            returned,
            ..
        } = &mut self.kind
            && !*returned
        {
            return_slot(state, *index, drained);
        }
        self.finish();
    }

    fn finish(&mut self) {
        match &mut self.kind {
            LeaseKind::Pooled {
                active_leases,
                drained,
                returned,
                ..
            }
            | LeaseKind::Ephemeral {
                active_leases,
                drained,
                returned,
                ..
            } => finish_lease(active_leases, drained, returned),
        }
    }
}

/// Return a claimed slot index to the free-list exactly once.
fn return_slot(state: &StdMutex<PoolState>, index: usize, changed: &Notify) {
    let mut state = state.lock().expect("pool state lock");
    state.free_slots.push_back(index);
    if state.refilling.remove(&index) || !state.refilling.is_empty() {
        changed.notify_waiters();
    }
}

fn lease_finished(active_leases: &AtomicUsize, drained: &Notify) {
    if active_leases.fetch_sub(1, Ordering::AcqRel) == 1 {
        drained.notify_waiters();
    }
}

fn finish_lease(active_leases: &AtomicUsize, drained: &Notify, returned: &mut bool) {
    if !*returned {
        *returned = true;
        lease_finished(active_leases, drained);
    }
}

impl Drop for RunnerLease {
    fn drop(&mut self) {
        // Fail-safe: if neither release nor evict ran (e.g. early `?`), the
        // runner is dropped here (killed) and the pooled slot is left empty (we
        // never parked the runner). This is the conservative eviction path —
        // never park a runner whose execution state we did not verify. The slot
        // index still returns to the free-list so the slot is reusable.
        drop(self.runner.take());
        if let LeaseKind::Pooled {
            state,
            drained,
            index,
            returned,
            ..
        } = &mut self.kind
            && !*returned
        {
            return_slot(state, *index, drained);
        }
        self.finish();
    }
}
