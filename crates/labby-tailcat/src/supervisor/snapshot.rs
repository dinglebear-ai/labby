use crate::BridgeError;
use tempfile::TempDir;
use tokio::{task::JoinHandle, time::Instant};

struct SnapshotWorker(JoinHandle<Result<TempDir, BridgeError>>);

impl Drop for SnapshotWorker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn prepare<F>(deadline: Instant, create: F) -> Result<TempDir, BridgeError>
where
    F: FnOnce() -> Result<TempDir, BridgeError> + Send + 'static,
{
    // The worker owns its TempDir until transfer. If this future is dropped or
    // times out, the eventual unclaimed result is dropped and removes the files.
    // A running filesystem syscall cannot be aborted; no helper process exists yet.
    let mut worker = SnapshotWorker(tokio::task::spawn_blocking(create));
    let snapshot = match tokio::time::timeout_at(deadline, &mut worker.0).await {
        Ok(result) => result.map_err(|_| BridgeError::ArtifactUnavailable)??,
        Err(_) => {
            // Prevent a queued worker from starting after the caller's deadline.
            worker.0.abort();
            return Err(BridgeError::StartupTimeout);
        }
    };
    if Instant::now() >= deadline {
        return Err(BridgeError::StartupTimeout);
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)] // Bounded fixture failures must report a test failure.
    use super::*;
    use std::{path::PathBuf, sync::mpsc, time::Duration};

    fn blocked_creation() -> (
        impl FnOnce() -> Result<TempDir, BridgeError> + Send + 'static,
        tokio::sync::oneshot::Receiver<PathBuf>,
        mpsc::Sender<()>,
    ) {
        let (created_tx, created_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let create = move || {
            let snapshot = tempfile::tempdir().map_err(|_| BridgeError::ArtifactUnavailable)?;
            drop(created_tx.send(snapshot.path().to_path_buf()));
            // Test-owned watchdog prevents a regression from deadlocking the suite.
            let _ = release_rx.recv_timeout(Duration::from_secs(1));
            Ok(snapshot)
        };
        (create, created_rx, release_tx)
    }

    async fn assert_removed(path: &std::path::Path) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while path.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("cancelled snapshot remained on disk");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocked_snapshot_creation_leaves_executor_responsive() {
        let (create, mut created, release) = blocked_creation();
        let mut preparation = Box::pin(prepare(Instant::now() + Duration::from_secs(2), create));
        let path = tokio::select! {
            result = &mut preparation => panic!("blocking snapshot completed before executor could observe creation: {result:?}"),
            path = &mut created => path.expect("snapshot creation did not report custody"),
        };
        assert!(path.exists());
        release
            .send(())
            .expect("snapshot worker exited before release");
        let snapshot = preparation.await.expect("snapshot preparation failed");
        assert_eq!(snapshot.path(), path);
        drop(snapshot);
        assert_removed(&path).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocked_snapshot_creation_obeys_startup_deadline_and_cleans_up() {
        let (create, mut created, release) = blocked_creation();
        let mut preparation =
            Box::pin(prepare(Instant::now() + Duration::from_millis(100), create));
        let path = tokio::select! {
            result = &mut preparation => panic!("snapshot deadline did not preserve pending custody: {result:?}"),
            path = &mut created => path.expect("snapshot creation did not report custody"),
        };
        assert!(matches!(
            preparation.await,
            Err(BridgeError::StartupTimeout)
        ));
        release
            .send(())
            .expect("snapshot worker exited before deadline");
        assert_removed(&path).await;
    }

    #[test]
    fn cancelled_queued_snapshot_never_creates_files() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (blocker_started, started) = tokio::sync::oneshot::channel();
            let (release, blocked) = mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = blocker_started.send(());
                let _ = blocked.recv_timeout(Duration::from_secs(5));
            });
            started.await.unwrap();
            let parent = tempfile::tempdir().unwrap();
            let marker = parent.path().join("unexpected-snapshot-write");
            let operation_marker = marker.clone();
            let (executed, retired) = tokio::sync::oneshot::channel();
            let mut preparation = Box::pin(prepare(
                Instant::now() + Duration::from_secs(5),
                move || {
                    let snapshot = tempfile::tempdir().unwrap();
                    std::fs::write(operation_marker, b"cancelled work ran").unwrap();
                    let _ = executed.send(());
                    Ok(snapshot)
                },
            ));
            tokio::select! {
                result = &mut preparation => panic!("snapshot escaped occupied worker pool: {result:?}"),
                () = tokio::task::yield_now() => {},
            }
            drop(preparation);
            release.send(()).unwrap();
            blocker.await.unwrap();
            assert!(retired.await.is_err(), "cancelled queued operation executed");
            assert!(!marker.exists(), "cancelled snapshot wrote files");
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropped_snapshot_preparation_retires_private_snapshot() {
        let (create, mut created, release) = blocked_creation();
        let mut preparation = Box::pin(prepare(Instant::now() + Duration::from_secs(2), create));
        let path = tokio::select! {
            result = &mut preparation => panic!("snapshot completed before cancellation: {result:?}"),
            path = &mut created => path.expect("snapshot creation did not report custody"),
        };
        drop(preparation);
        release
            .send(())
            .expect("snapshot worker exited before cancellation");
        assert_removed(&path).await;
    }
}
