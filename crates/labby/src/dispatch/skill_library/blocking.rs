//! Bounded blocking-work and deterministic fault seams for Skill Library dispatch.

#![allow(dead_code, reason = "consumed by the Wave 3 Skill Library dispatcher")]

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Semaphore, oneshot};

/// Dedicated bounded executor for filesystem and durable Artifact work.
#[derive(Clone)]
pub(crate) struct BoundedBlockingExecutor {
    permits: Arc<Semaphore>,
    queue_deadline: Duration,
    execution_deadline: Duration,
}

impl BoundedBlockingExecutor {
    pub(crate) fn new(
        max_in_flight: usize,
        queue_deadline: Duration,
        execution_deadline: Duration,
    ) -> Result<Self, BlockingConfigError> {
        if max_in_flight == 0 || queue_deadline.is_zero() || execution_deadline.is_zero() {
            return Err(BlockingConfigError);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(max_in_flight)),
            queue_deadline,
            execution_deadline,
        })
    }

    /// Run one synchronous operation without holding an async registry/auth lock or lease.
    ///
    /// The owned permit moves into the blocking closure. Dropping or timing out the caller
    /// therefore cannot admit replacement work while the abandoned operation is still running.
    pub(crate) async fn run<T, E, F>(
        &self,
        operation: &'static str,
        work: F,
    ) -> Result<T, BlockingError<E>>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        let permit = self.acquire(operation).await?;
        self.execute(operation, permit, work).await
    }

    /// Admit capacity first, then perform final async authorization and build the sync work.
    ///
    /// This ordering prevents a valid authorization snapshot from aging in the queue. The
    /// admission closure must return an owned `'static` work closure, so no auth lease, registry
    /// guard, or async lock can cross into blocking execution.
    pub(crate) async fn run_after_admission<T, E, A, AF, F>(
        &self,
        operation: &'static str,
        admit: A,
    ) -> Result<T, BlockingError<E>>
    where
        T: Send + 'static,
        E: Send + 'static,
        A: FnOnce() -> AF,
        AF: Future<Output = Result<F, E>>,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        let permit = self.acquire(operation).await?;
        let work = admit().await.map_err(BlockingError::Operation)?;
        self.execute(operation, permit, work).await
    }

    async fn acquire<E>(
        &self,
        operation: &'static str,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, BlockingError<E>> {
        tokio::time::timeout(
            self.queue_deadline,
            Arc::clone(&self.permits).acquire_owned(),
        )
        .await
        .map_err(|_| BlockingError::Busy { operation })?
        .map_err(|_| BlockingError::WorkerFailed { operation })
    }

    async fn execute<T, E, F>(
        &self,
        operation: &'static str,
        permit: tokio::sync::OwnedSemaphorePermit,
        work: F,
    ) -> Result<T, BlockingError<E>>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce() -> Result<T, E> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            drop(sender.send(work()));
        });
        match tokio::time::timeout(self.execution_deadline, receiver).await {
            Err(_) => Err(BlockingError::Timeout { operation }),
            Ok(Err(_)) => Err(BlockingError::WorkerFailed { operation }),
            Ok(Ok(Err(error))) => Err(BlockingError::Operation(error)),
            Ok(Ok(Ok(value))) => Ok(value),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("blocking executor requires non-zero capacity and deadlines")]
pub(crate) struct BlockingConfigError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum BlockingError<E> {
    #[error("Skill Library blocking queue is busy for {operation}")]
    Busy { operation: &'static str },
    #[error("Skill Library blocking work timed out for {operation}")]
    Timeout { operation: &'static str },
    #[error("Skill Library blocking worker failed for {operation}")]
    WorkerFailed { operation: &'static str },
    #[error("Skill Library blocking operation failed")]
    Operation(E),
}

/// Stable fault boundaries spanning durable commit and generation publication.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FaultStage {
    BeforeCommit,
    AfterCommitBeforeSwap,
    AfterSwapBeforeResponse,
    DiskWrite,
    FileSync,
    RenameCommit,
    ParentSync,
}

/// Injectable failpoint contract. Production uses [`NoFaultInjector`].
pub(crate) trait FaultInjector: Send + Sync {
    fn check(&self, stage: FaultStage) -> Result<(), InjectedFault>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoFaultInjector;

impl FaultInjector for NoFaultInjector {
    fn check(&self, _stage: FaultStage) -> Result<(), InjectedFault> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("injected Skill Library fault at {stage:?}")]
pub(crate) struct InjectedFault {
    pub(crate) stage: FaultStage,
}

/// Durable filesystem boundary used for stable failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiskBoundary {
    Write,
    FileSync,
    RenameCommit,
    ParentSync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiskFailureKind {
    NoSpace,
    Io,
}

/// Redacted disk failure: it carries no host path or OS-controlled message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Skill Library disk {kind:?} failure at {boundary:?}")]
pub(crate) struct DiskBoundaryError {
    pub(crate) boundary: DiskBoundary,
    pub(crate) kind: DiskFailureKind,
}

pub(crate) fn classify_disk_error(
    boundary: DiskBoundary,
    error: &std::io::Error,
) -> DiskBoundaryError {
    let kind =
        if error.kind() == std::io::ErrorKind::StorageFull || error.raw_os_error() == Some(28) {
            DiskFailureKind::NoSpace
        } else {
            DiskFailureKind::Io
        };
    DiskBoundaryError { boundary, kind }
}

impl<E: fmt::Debug> BlockingError<E> {
    #[cfg(test)]
    fn operation_error(&self) -> Option<&E> {
        match self {
            Self::Operation(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn executor(capacity: usize, queue_ms: u64, execution_ms: u64) -> BoundedBlockingExecutor {
        BoundedBlockingExecutor::new(
            capacity,
            Duration::from_millis(queue_ms),
            Duration::from_millis(execution_ms),
        )
        .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn single_worker_heartbeat_continues_during_max_legal_blocking_work() {
        let executor = executor(1, 100, 2_000);
        let heartbeat = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&heartbeat);
        let ticker = tokio::spawn(async move {
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(5)).await;
                observed.fetch_add(1, Ordering::SeqCst);
            }
        });
        let value = executor
            .run("max_legal", || {
                std::thread::sleep(Duration::from_millis(120));
                Ok::<_, ()>(7)
            })
            .await
            .unwrap();
        ticker.await.unwrap();
        assert_eq!(value, 7);
        assert!(heartbeat.load(Ordering::SeqCst) >= 10);
    }

    #[tokio::test]
    async fn saturation_is_busy_and_cancellation_does_not_leak_a_permit() {
        let executor = executor(1, 25, 2_000);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_executor = executor.clone();
        let first = tokio::spawn(async move {
            first_executor
                .run("first", move || {
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    Ok::<_, ()>(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        first.abort();
        let saturated = executor.run("second", || Ok::<_, ()>(())).await;
        assert!(matches!(
            saturated,
            Err(BlockingError::Busy {
                operation: "second"
            })
        ));
        release_tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        executor.run("third", || Ok::<_, ()>(())).await.unwrap();
    }

    #[tokio::test]
    async fn final_admission_runs_after_queue_wait_and_can_stop_revoked_work() {
        let executor = executor(1, 500, 500);
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_executor = executor.clone();
        let first = tokio::spawn(async move {
            first_executor
                .run("holder", move || {
                    let _ = started_tx.send(());
                    release_rx.recv().unwrap();
                    Ok::<_, &'static str>(())
                })
                .await
        });
        started_rx.await.unwrap();
        let revoked = Arc::new(AtomicBool::new(false));
        let work_ran = Arc::new(AtomicBool::new(false));
        let queued_executor = executor.clone();
        let observed_revocation = Arc::clone(&revoked);
        let work_marker = Arc::clone(&work_ran);
        let queued = tokio::spawn(async move {
            queued_executor
                .run_after_admission("commit", move || async move {
                    if observed_revocation.load(Ordering::SeqCst) {
                        return Err("revoked");
                    }
                    Ok(move || {
                        work_marker.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                })
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        revoked.store(true, Ordering::SeqCst);
        release_tx.send(()).unwrap();
        first.await.unwrap().unwrap();
        let result = queued.await.unwrap();
        assert!(matches!(result, Err(BlockingError::Operation("revoked"))));
        assert!(!work_ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn timeout_retains_capacity_until_slow_fsync_finishes() {
        let executor = executor(1, 20, 20);
        let finished = Arc::new(AtomicBool::new(false));
        let marker = Arc::clone(&finished);
        let timed_out = executor
            .run("file_sync", move || {
                std::thread::sleep(Duration::from_millis(100));
                marker.store(true, Ordering::SeqCst);
                Ok::<_, DiskBoundaryError>(())
            })
            .await;
        assert!(matches!(
            timed_out,
            Err(BlockingError::Timeout {
                operation: "file_sync"
            })
        ));
        assert!(matches!(
            executor
                .run("replacement", || Ok::<_, DiskBoundaryError>(()))
                .await,
            Err(BlockingError::Busy { .. })
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(finished.load(Ordering::SeqCst));
        executor
            .run("recovered", || Ok::<_, DiskBoundaryError>(()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn enospc_and_other_disk_errors_are_typed_and_redacted() {
        let executor = executor(1, 100, 100);
        let error = executor
            .run("write", || {
                let io = std::io::Error::from_raw_os_error(28);
                Err::<(), _>(classify_disk_error(DiskBoundary::Write, &io))
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.operation_error(),
            Some(&DiskBoundaryError {
                boundary: DiskBoundary::Write,
                kind: DiskFailureKind::NoSpace
            })
        );
        assert!(!error.to_string().contains('/'));

        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secret/path");
        assert_eq!(
            classify_disk_error(DiskBoundary::ParentSync, &io),
            DiskBoundaryError {
                boundary: DiskBoundary::ParentSync,
                kind: DiskFailureKind::Io
            }
        );
    }

    #[test]
    fn fault_stages_are_exact_and_production_is_inert() {
        let injector: Arc<dyn FaultInjector> = Arc::new(NoFaultInjector);
        for stage in [
            FaultStage::BeforeCommit,
            FaultStage::AfterCommitBeforeSwap,
            FaultStage::AfterSwapBeforeResponse,
            FaultStage::DiskWrite,
            FaultStage::FileSync,
            FaultStage::RenameCommit,
            FaultStage::ParentSync,
        ] {
            injector.check(stage).unwrap();
        }
    }
}
