//! Code Mode warm-runner pool (Perf H1).
//!
//! Pools the **OS process** of the Code Mode runner, not the JS runtime. Each
//! pooled runner builds a FRESH `javy::Runtime` per `Start` (runner-side), so JS
//! heap-state isolation comes from a brand-new runtime with no
//! globals, no `__labPendingToolCalls`, and no captured data from a prior
//! caller. Done permits reuse only with native reply-consumption proof and no
//! pending host calls. Unqualified Done preserves success but evicts, like Error,
//! because old pipe frames can survive a dropped runtime.
//!
//! Concurrency model: a bounded set of long-lived runners, one execution per
//! runner at a time. `size` permits gate access to pooled runners; when all are
//! busy the caller spawns a bounded ephemeral (overflow) runner instead of
//! queueing unboundedly. A runner that reports Error/unqualified Done, crashes, times out, or reaches the
//! recycle-after-K threshold is evicted and a fresh one is spawned.
//!
//! Kill switch: `LABBY_CODE_MODE_POOL_SIZE=0` disables pooling entirely and the
//! drive layer falls back to the historical spawn-per-execution path.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{Mutex, Notify, Semaphore};

use crate::error::ToolError;

use super::pool::config::PoolConfig;
use super::pool::runner_handle::PooledRunner;

async fn shutdown_runner(runner: PooledRunner, action: &'static str) {
    if let Err(error) = runner.shutdown().await {
        tracing::warn!(
            surface = "dispatch",
            service = "code_mode",
            action,
            error = %error,
            "Code Mode runner shutdown did not complete cleanly"
        );
    }
}

static MICROSANDBOX_ADMISSION: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn microsandbox_admission() -> Arc<Semaphore> {
    Arc::clone(
        MICROSANDBOX_ADMISSION
            .get_or_init(|| Arc::new(Semaphore::new(config::microsandbox_max_runners()))),
    )
}

fn admission_timeout() -> ToolError {
    tracing::warn!(
        surface = "dispatch",
        service = "code_mode",
        action = "pool.saturated",
        "Code Mode runner admission expired while all permits were in use"
    );
    ToolError::Sdk {
        sdk_kind: "queue_saturated".into(),
        message: "Code Mode runner capacity is exhausted; retry later".into(),
    }
}

fn timeout_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "timeout".to_string(),
        message: "Code Mode execution timed out".to_string(),
    }
}

pub(crate) async fn spawn_before_deadline(
    spawn: &RunnerSpawn,
    microsandbox: Option<&MicrosandboxSpawn>,
    deadline: tokio::time::Instant,
) -> Result<PooledRunner, ToolError> {
    let permit = if microsandbox.is_some() {
        Some(
            tokio::time::timeout_at(deadline, microsandbox_admission().acquire_owned())
                .await
                .map_err(|_| admission_timeout())?
                .map_err(|_| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: "Code Mode Microsandbox admission semaphore closed".to_string(),
                })?,
        )
    } else {
        None
    };
    let mut runner =
        tokio::time::timeout_at(deadline, PooledRunner::spawn(spawn, microsandbox, permit))
            .await
            .map_err(|_| timeout_error())??;
    runner.check_protocol_version(deadline).await?;
    Ok(runner)
}

struct PoolState {
    free_slots: VecDeque<usize>,
    refilling: std::collections::BTreeSet<usize>,
    shutting_down: bool,
}

pub(crate) mod config;
mod lease;
pub(crate) use lease::RunnerLease;
mod microsandbox;
pub(crate) mod runner_handle;
#[cfg(test)]
mod test_support;

/// Host-configurable runner re-invocation: the program to exec and the args to
/// pass it. The pool re-execs this per spawned runner.
///
/// The default is the current executable plus the canonical
/// `["internal", "code-mode-runner"]` args, so a labby binary hosts its own
/// runner; a different host binary can supply its own program/args seam.
#[derive(Debug, Clone)]
pub struct RunnerSpawn {
    /// Executable used to launch the runner subprocess.
    pub program: std::path::PathBuf,
    /// Arguments passed to the runner executable.
    pub args: Vec<String>,
}

/// Validated Microsandbox host configuration for one runner process.
#[derive(Debug, Clone)]
pub(crate) struct MicrosandboxSpawn {
    pub(crate) executable: std::path::PathBuf,
    pub(crate) image: String,
}

impl RunnerSpawn {
    /// Resolve the default self-hosted Code Mode runner invocation.
    pub fn try_default() -> Result<Self, ToolError> {
        Self::try_for_backend(false)
    }

    pub(crate) fn try_for_backend(for_mount: bool) -> Result<Self, ToolError> {
        Ok(Self {
            program: super::runner_exe::resolve_runner_exe(
                for_mount,
                std::env::var_os("LABBY_CODE_MODE_RUNNER_EXE").map(Into::into),
            )?,
            args: vec!["internal".into(), "code-mode-runner".into()],
        })
    }
}

/// A bounded pool of long-lived Code Mode runner processes.
///
/// Slot ownership is tracked by an explicit free-list of indices rather than by
/// scanning mutex states, so two concurrent checkouts can never select the same
/// slot. A slot index is popped on checkout and pushed back on lease finalize.
pub struct RunnerPool {
    config: PoolConfig,
    /// Host-supplied runner re-invocation (program + args).
    spawn: RunnerSpawn,
    microsandbox: Option<MicrosandboxSpawn>,
    /// One slot per pooled runner. `None` = empty slot to (re)spawn into. The
    /// outer `Mutex` is only ever held briefly to move the runner in/out; the
    /// free-list guarantees single-owner access for the lease lifetime.
    slots: Vec<Arc<Mutex<Option<PooledRunner>>>>,
    /// Indices of currently-idle slots. Popping is the atomic "claim a slot"
    /// operation; the popped index is held by the lease and pushed back on
    /// finalize.
    state: Arc<StdMutex<PoolState>>,
    active_leases: Arc<AtomicUsize>,
    drained: Arc<Notify>,
    /// Bounds simultaneous ephemeral (overflow) runners when the pool is saturated.
    overflow_permits: Arc<Semaphore>,
}

impl RunnerPool {
    /// Build a pool from the environment-derived config and the default runner
    /// spawn seam (`current_exe()` + `["internal", "code-mode-runner"]`). When
    /// pooling is disabled (`size == 0`) the pool holds no slots and every
    /// checkout returns an ephemeral runner — i.e. the spawn-per-execution
    /// fallback.
    pub fn from_env() -> Result<Self, ToolError> {
        let (spawn, microsandbox) = super::runner_backend::resolve_runner_spawn()?;
        Ok(Self::with_config_and_spawn(
            PoolConfig::from_env(),
            spawn,
            microsandbox,
        ))
    }

    /// Build a pool with an explicit, host-supplied runner spawn seam.
    pub fn with_spawn(spawn: RunnerSpawn) -> Self {
        Self::with_config_and_spawn(PoolConfig::from_env(), spawn, None)
    }

    #[cfg(test)]
    pub(crate) fn with_config(config: PoolConfig) -> Self {
        Self::with_config_and_spawn(
            config,
            RunnerSpawn::try_default().expect("test process must expose current executable"),
            None,
        )
    }

    fn with_config_and_spawn(
        mut config: PoolConfig,
        spawn: RunnerSpawn,
        microsandbox: Option<MicrosandboxSpawn>,
    ) -> Self {
        if microsandbox.is_some() {
            config.size = config.size.min(config::microsandbox_max_runners());
        }
        let slots = (0..config.size)
            .map(|_| Arc::new(Mutex::new(None)))
            .collect::<Vec<_>>();
        let free_slots = (0..config.size).collect::<VecDeque<_>>();
        // Overflow permits bound simultaneous ephemeral runners. Honor explicit
        // low values even when pooling is disabled so the kill switch can also
        // reduce resource pressure; only avoid a zero-permit deadlock.
        let overflow = config.max_overflow.max(1);
        Self {
            config,
            spawn,
            microsandbox,
            slots,
            state: Arc::new(StdMutex::new(PoolState {
                free_slots,
                refilling: Default::default(),
                shutting_down: false,
            })),
            active_leases: Arc::new(AtomicUsize::new(0)),
            drained: Arc::new(Notify::new()),
            overflow_permits: Arc::new(Semaphore::new(overflow)),
        }
    }

    #[cfg(test)]
    pub(crate) fn config(&self) -> PoolConfig {
        self.config
    }

    #[cfg(test)]
    pub(crate) fn available_overflow_permits(&self) -> usize {
        self.overflow_permits.available_permits()
    }

    /// Check out a runner for one execution.
    ///
    /// Returns a [`RunnerLease`] that owns exclusive access to one runner for the
    /// duration of a single execution. Claims an idle pooled slot first; on
    /// saturation spawns a bounded ephemeral runner. The lease MUST be finalized
    /// via [`RunnerLease::release`] / [`RunnerLease::evict`] (or dropped, which
    /// evicts) so the slot index returns to the free-list.
    pub(crate) async fn checkout(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<RunnerLease, ToolError> {
        // Pooled fast path: atomically claim a free slot index.
        if !self.config.is_disabled() {
            let claimed = loop {
                let changed = self.drained.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                {
                    let mut state = self.state.lock().expect("pool state lock");
                    ensure_running(&state)?;
                    if let Some(index) = state.free_slots.pop_front() {
                        self.active_leases.fetch_add(1, Ordering::AcqRel);
                        break Some(index);
                    }
                    if state.refilling.is_empty() {
                        break None;
                    }
                }
                tokio::time::timeout_at(deadline, changed)
                    .await
                    .map_err(|_| admission_timeout())?;
            };
            if let Some(index) = claimed {
                // Own the reservation before the first suspension point. Dropping
                // checkout during spawn must return the slot and active count.
                let mut lease = RunnerLease::pooled(
                    Arc::clone(&self.slots[index]),
                    Arc::clone(&self.state),
                    Arc::clone(&self.active_leases),
                    Arc::clone(&self.drained),
                    index,
                    None,
                    self.config,
                );
                lease.runner = Some(self.take_or_spawn_slot(index, deadline).await?);
                return Ok(lease);
            }
        }

        // Saturated (or disabled): spawn a bounded ephemeral runner.
        tracing::debug!(
            surface = "dispatch",
            service = "code_mode",
            action = "pool.overflow",
            pool_size = self.config.size,
            "pool saturated; spawning ephemeral Code Mode runner"
        );
        self.checkout_fresh(deadline).await
    }

    /// Replace a stale runner under its original lease. Using the same slot
    /// prevents a background refill from taking the last guest permit needed
    /// for an explicitly authorized, pre-activity retry.
    pub(crate) async fn replace_for_retry(
        &self,
        lease: &mut RunnerLease,
        deadline: tokio::time::Instant,
    ) -> Result<(), ToolError> {
        if let Some(stale) = lease.runner.take() {
            tokio::time::timeout_at(deadline, shutdown_runner(stale, "pool.retry_fresh"))
                .await
                .map_err(|_| timeout_error())?;
        }
        ensure_running(&self.state.lock().expect("pool state lock"))?;
        lease.runner =
            Some(spawn_before_deadline(&self.spawn, self.microsandbox.as_ref(), deadline).await?);
        Ok(())
    }

    /// Admit a fresh overflow or standalone runner.
    pub(crate) async fn checkout_fresh(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<RunnerLease, ToolError> {
        let permit =
            tokio::time::timeout_at(deadline, Arc::clone(&self.overflow_permits).acquire_owned())
                .await
                .map_err(|_| admission_timeout())?
                .map_err(|_| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: "Code Mode runner pool overflow semaphore closed".to_string(),
                })?;
        {
            let state = self.state.lock().expect("pool state lock");
            ensure_running(&state)?;
            self.active_leases.fetch_add(1, Ordering::AcqRel);
        }
        let mut lease = RunnerLease::ephemeral(
            None,
            permit,
            Arc::clone(&self.active_leases),
            Arc::clone(&self.drained),
        );
        lease.runner =
            Some(spawn_before_deadline(&self.spawn, self.microsandbox.as_ref(), deadline).await?);
        Ok(lease)
    }

    /// Take the runner out of a claimed slot, spawning a fresh one if the slot is
    /// empty (first use or post-eviction).
    async fn take_or_spawn_slot(
        &self,
        index: usize,
        deadline: tokio::time::Instant,
    ) -> Result<PooledRunner, ToolError> {
        let mut guard = self.slots[index].lock().await;
        match guard.take() {
            Some(runner) if runner.microsandbox_lifetime_elapsed() => {
                drop(
                    tokio::time::timeout_at(
                        deadline,
                        shutdown_runner(runner, "pool.microsandbox_lifetime"),
                    )
                    .await,
                );
                if tokio::time::Instant::now() >= deadline {
                    return Err(timeout_error());
                }
                spawn_before_deadline(&self.spawn, self.microsandbox.as_ref(), deadline).await
            }
            Some(runner) => Ok(runner),
            None => spawn_before_deadline(&self.spawn, self.microsandbox.as_ref(), deadline).await,
        }
    }

    /// Stop admitting work, wait for outstanding leases, then explicitly reap
    /// every parked runner. Safe to call more than once.
    pub async fn shutdown(&self) {
        self.state.lock().expect("pool state lock").shutting_down = true;
        loop {
            // Register first so the final lease cannot notify between the
            // count observation and waiter registration.
            let drained = self.drained.notified();
            if self.active_leases.load(Ordering::Acquire) == 0 {
                break;
            }
            drained.await;
        }
        for slot in &self.slots {
            if let Some(runner) = slot.lock().await.take() {
                shutdown_runner(runner, "pool.shutdown").await;
            }
        }
    }

    /// Test-only checkout that spawns stub (non-protocol) runners so the pool's
    /// lease / free-list / recycle / eviction bookkeeping and PID reuse can be
    /// exercised without the real labby binary.
    #[cfg(test)]
    pub(crate) async fn checkout_stub(&self) -> Result<RunnerLease, ToolError> {
        if !self.config.is_disabled() {
            let claimed = {
                let mut state = self.state.lock().expect("pool state lock");
                ensure_running(&state)?;
                let claimed = state.free_slots.pop_front();
                if claimed.is_some() {
                    self.active_leases.fetch_add(1, Ordering::AcqRel);
                }
                claimed
            };
            if let Some(index) = claimed {
                let mut guard = self.slots[index].lock().await;
                let runner = match guard.take() {
                    Some(runner) => runner,
                    None => PooledRunner::spawn_stub()?,
                };
                drop(guard);
                return Ok(RunnerLease::pooled(
                    Arc::clone(&self.slots[index]),
                    Arc::clone(&self.state),
                    Arc::clone(&self.active_leases),
                    Arc::clone(&self.drained),
                    index,
                    Some(runner),
                    self.config,
                ));
            }
        }
        let permit = Arc::clone(&self.overflow_permits)
            .acquire_owned()
            .await
            .expect("overflow semaphore open");
        {
            let state = self.state.lock().expect("pool state lock");
            ensure_running(&state)?;
            self.active_leases.fetch_add(1, Ordering::AcqRel);
        }
        Ok(RunnerLease::ephemeral(
            Some(PooledRunner::spawn_stub()?),
            permit,
            Arc::clone(&self.active_leases),
            Arc::clone(&self.drained),
        ))
    }
}

fn ensure_running(state: &PoolState) -> Result<(), ToolError> {
    if state.shutting_down {
        Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner pool is shutting down".to_string(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
