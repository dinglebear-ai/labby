//! Inventory-only SQLite execution. General store operations retain their policy.

use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::AccessStore;
use crate::access::inventory::{AccessReadDeadline, InventoryError, InventoryResult};

impl AccessStore {
    pub(in crate::access) async fn with_inventory_connection<T, F>(
        &self,
        deadline: AccessReadDeadline,
        operation: F,
    ) -> InventoryResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection, &CancellationToken) -> InventoryResult<T> + Send + 'static,
    {
        let cancellation = CancellationToken::new();
        let _cancel_on_drop = cancellation.clone().drop_guard();
        deadline.check(&cancellation)?;
        let admission_deadline = deadline
            .instant()
            .min(tokio::time::Instant::now() + Duration::from_millis(100));
        let permit = tokio::time::timeout_at(
            admission_deadline,
            Arc::clone(&self.connection_admission).acquire_owned(),
        )
        .await
        .map_err(|_| {
            if deadline.remaining().is_zero() {
                InventoryError::Timeout
            } else {
                InventoryError::QueueSaturated
            }
        })?
        .map_err(|_| InventoryError::WorkerUnavailable)?;
        let connection = Arc::clone(&self.connection);
        let admission = Arc::clone(&self.connection_admission);
        let worker_cancellation = cancellation.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            // Drop order keeps capacity owned until the mutex and cleanup guard
            // have gone away, including when the request future is dropped.
            let _permit = permit;
            deadline.check(&worker_cancellation)?;
            let connection = connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            deadline.check(&worker_cancellation)?;
            let mut guard = InventoryConnection {
                connection,
                admission,
                cleaned: false,
                #[cfg(test)]
                force_cleanup_failure: false,
            };
            let progress_cancellation = worker_cancellation.clone();
            guard
                .connection
                .progress_handler(
                    1_000,
                    Some(move || {
                        deadline.remaining().is_zero() || progress_cancellation.is_cancelled()
                    }),
                )
                .map_err(|_| InventoryError::WorkerUnavailable)?;
            let result = guard
                .connection
                .busy_timeout(deadline.remaining().min(Duration::from_millis(100)))
                .map_err(|_| InventoryError::WorkerUnavailable)
                .and_then(|()| {
                    deadline.check(&worker_cancellation)?;
                    operation(&mut guard.connection, &worker_cancellation)
                });
            // All operation statements/transactions have dropped before cleanup.
            guard.cleanup()?;
            deadline.check(&worker_cancellation)?;
            result
        });
        match tokio::time::timeout_at(deadline.instant(), &mut worker).await {
            Ok(result) => result.map_err(|_| InventoryError::WorkerUnavailable)?,
            Err(_) => {
                cancellation.cancel();
                // Never detach deadline work or release its capacity early.
                match worker.await {
                    Ok(Err(InventoryError::CleanupFailed)) => Err(InventoryError::CleanupFailed),
                    _ => Err(InventoryError::Timeout),
                }
            }
        }
    }
}

struct InventoryConnection<'a> {
    connection: MutexGuard<'a, Connection>,
    admission: Arc<Semaphore>,
    cleaned: bool,
    #[cfg(test)]
    force_cleanup_failure: bool,
}

impl InventoryConnection<'_> {
    fn cleanup(&mut self) -> InventoryResult<()> {
        self.cleaned = true;
        let hook_removed = self.connection.progress_handler(0, None::<fn() -> bool>);
        let busy_restored = self.connection.busy_timeout(super::BUSY_TIMEOUT);
        let cleanup_failed = hook_removed.is_err() || busy_restored.is_err();
        #[cfg(test)]
        let cleanup_failed = cleanup_failed || self.force_cleanup_failure;
        if cleanup_failed {
            self.admission.close();
            tracing::error!(
                service = "access",
                cause = "inventory_cleanup_failed",
                "access inventory connection cleanup failed"
            );
            return Err(InventoryError::CleanupFailed);
        }
        Ok(())
    }
}

impl Drop for InventoryConnection<'_> {
    fn drop(&mut self) {
        if !self.cleaned {
            drop(self.cleanup());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::store::map_sqlite_error;

    const LONG_SQL: &str = "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n";

    async fn ordinary_connection_is_clean(store: &AccessStore) {
        store.with_connection(|connection| {
            let busy: i64 = connection.query_row("PRAGMA busy_timeout", [], |row| row.get(0)).map_err(map_sqlite_error)?;
            assert_eq!(busy, 5_000);
            // More than 1,000 VM steps would hit an inherited cancelled hook.
            let sum: i64 = connection.query_row("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<2000) SELECT sum(x) FROM n", [], |row| row.get(0)).map_err(map_sqlite_error)?;
            assert_eq!(sum, 2_001_000);
            Ok(())
        }).await.unwrap();
        assert_eq!(store.connection_admission.available_permits(), 1);
    }

    #[tokio::test]
    async fn inventory_admission_is_bounded_without_spawning_sql() {
        let (_directory, store, _) = crate::access::test_support::fixture().await;
        let permit = store
            .connection_admission
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = called.clone();
        let result = store
            .with_inventory_connection(AccessReadDeadline::server(), move |_, _| {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .await;
        assert!(
            matches!(result, Err(InventoryError::QueueSaturated)),
            "{result:?}"
        );
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        let result = store
            .with_inventory_connection(
                AccessReadDeadline::after(Duration::from_millis(5)),
                |_, _| Ok(()),
            )
            .await;
        assert!(matches!(result, Err(InventoryError::Timeout)), "{result:?}");
        drop(permit);
        ordinary_connection_is_clean(&store).await;
    }

    #[tokio::test]
    async fn inventory_long_sql_deadline_awaits_cleanup_and_next_caller_succeeds() {
        let (_directory, store, _) = crate::access::test_support::fixture().await;
        let result = store
            .with_inventory_connection(
                AccessReadDeadline::after(Duration::from_millis(20)),
                |connection, _| {
                    connection
                        .query_row(LONG_SQL, [], |row| row.get::<_, i64>(0))
                        .map_err(map_sqlite_error)
                        .map_err(Into::into)
                },
            )
            .await;
        assert!(matches!(result, Err(InventoryError::Timeout)), "{result:?}");
        ordinary_connection_is_clean(&store).await;
    }

    #[tokio::test]
    async fn inventory_request_drop_cancels_sql_and_retains_worker_permit() {
        let (_directory, store, _) = crate::access::test_support::fixture().await;
        let (started, observed_start) = tokio::sync::oneshot::channel();
        let (release, wait_release) = std::sync::mpsc::sync_channel(0);
        let worker_store = store.clone();
        let request = tokio::spawn(async move {
            worker_store
                .with_inventory_connection(
                    AccessReadDeadline::server(),
                    move |connection, cancellation| {
                        started.send(()).unwrap();
                        wait_release.recv_timeout(Duration::from_secs(2)).unwrap();
                        assert!(cancellation.is_cancelled());
                        let result = connection.query_row(LONG_SQL, [], |row| row.get::<_, i64>(0));
                        assert!(result.is_err());
                        Ok(())
                    },
                )
                .await
        });
        observed_start.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(store.connection_admission.available_permits(), 0);
        let waiting = store
            .with_inventory_connection(
                AccessReadDeadline::after(Duration::from_millis(5)),
                |_, _| Ok(()),
            )
            .await;
        assert!(matches!(waiting, Err(InventoryError::Timeout)));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), ordinary_connection_is_clean(&store))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn inventory_error_and_panic_restore_operation_hooks_and_busy_timeout() {
        let (_directory, store, _) = crate::access::test_support::fixture().await;
        let result: InventoryResult<()> = store
            .with_inventory_connection(AccessReadDeadline::server(), |_, _| {
                Err(InventoryError::RevisionConflict)
            })
            .await;
        assert!(matches!(result, Err(InventoryError::RevisionConflict)));
        ordinary_connection_is_clean(&store).await;
        let result: InventoryResult<()> = store
            .with_inventory_connection(AccessReadDeadline::server(), |_, _| {
                panic!("intentional inventory worker panic")
            })
            .await;
        assert!(matches!(result, Err(InventoryError::WorkerUnavailable)));
        ordinary_connection_is_clean(&store).await;
    }

    #[tokio::test]
    async fn inventory_success_restores_configuration_and_sort_offset_is_interruptible() {
        let (_directory, store, _) = crate::access::test_support::fixture().await;
        let busy = store
            .with_inventory_connection(AccessReadDeadline::server(), |connection, _| {
                connection
                    .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
                    .map_err(map_sqlite_error)
                    .map_err(Into::into)
            })
            .await
            .unwrap();
        assert!((1..=100).contains(&busy));
        ordinary_connection_is_clean(&store).await;
        let result = store
            .with_inventory_connection(
                AccessReadDeadline::after(Duration::from_millis(20)),
                |connection, _| {
                    connection.query_row(
                        "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000)
                         SELECT x FROM n ORDER BY x DESC LIMIT 1 OFFSET 100000",
                        [],
                        |row| row.get::<_, i64>(0),
                    ).map_err(map_sqlite_error).map_err(Into::into)
                },
            )
            .await;
        assert!(matches!(result, Err(InventoryError::Timeout)), "{result:?}");
        ordinary_connection_is_clean(&store).await;
    }

    #[tokio::test]
    async fn inventory_cleanup_failure_closes_admission_before_releasing_capacity() {
        let (_directory, store, _) = crate::access::test_support::fixture().await;
        let permit = store
            .connection_admission
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        {
            let connection = store.connection.lock().unwrap();
            let mut guard = InventoryConnection {
                connection,
                admission: store.connection_admission.clone(),
                cleaned: false,
                // Exercise the failure disposition after the real restoration
                // APIs. Their ordinary valid-connection path cannot force an error.
                force_cleanup_failure: true,
            };
            assert!(matches!(
                guard.cleanup(),
                Err(InventoryError::CleanupFailed)
            ));
            assert!(store.connection_admission.is_closed());
            assert_eq!(store.connection_admission.available_permits(), 0);
        }
        drop(permit);
        let result = store
            .with_inventory_connection(AccessReadDeadline::server(), |_, _| Ok(()))
            .await;
        assert!(matches!(result, Err(InventoryError::WorkerUnavailable)));
    }

    #[test]
    fn inventory_late_blocking_start_checks_original_deadline_before_sql() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_directory, store, _) = crate::access::test_support::fixture().await;
            let (started, observed_start) = tokio::sync::oneshot::channel();
            let (release, wait_release) = std::sync::mpsc::sync_channel(0);
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                wait_release.recv_timeout(Duration::from_secs(2)).unwrap();
            });
            observed_start.await.unwrap();
            let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observed = called.clone();
            let deadline = AccessReadDeadline::after(Duration::from_millis(5));
            let request = store.with_inventory_connection(deadline, move |_, _| {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            });
            tokio::pin!(request);
            // Drive admission and queue the worker behind our owned blocker.
            assert!(matches!(
                futures::poll!(&mut request),
                std::task::Poll::Pending
            ));
            tokio::time::sleep_until(deadline.instant()).await;
            release.send(()).unwrap();
            blocker.await.unwrap();
            assert!(matches!(request.await, Err(InventoryError::Timeout)));
            assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
            ordinary_connection_is_clean(&store).await;
        });
    }
}
