//! Coalesced, cancellation-safe automatic artifact retention passes.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Caller holds STORE_MUTATION; remember only completed passes so cancelled
/// admission remains eligible for retry. Pressure-triggered pruning bypasses
/// this routine coalescing while preserving the same storage mutation lock.
pub(super) async fn prune_once(root: &Path, pass: impl Future<Output = ()>) -> bool {
    static COMPLETED: OnceLock<tokio::sync::Mutex<VecDeque<PathBuf>>> = OnceLock::new();
    run_retention_once(
        COMPLETED.get_or_init(|| tokio::sync::Mutex::new(VecDeque::new())),
        root,
        pass,
    )
    .await
}

async fn run_retention_once(
    completed: &tokio::sync::Mutex<VecDeque<PathBuf>>,
    root: &Path,
    pass: impl Future<Output = ()>,
) -> bool {
    let mut completed = completed.lock().await;
    if completed.iter().any(|path| path == root) {
        return false;
    }
    pass.await;
    while completed.len() >= 1024 {
        completed.pop_front();
    }
    completed.push_back(root.to_owned());
    true
}

#[cfg(test)]
mod retention_once_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn automatic_retention_runs_once_and_cancellation_does_not_complete_it() {
        let completed = tokio::sync::Mutex::new(VecDeque::new());
        let calls = AtomicUsize::new(0);
        let root = Path::new("one-run");
        for _ in 0..3 {
            run_retention_once(&completed, root, async {
                calls.fetch_add(1, Ordering::Relaxed);
            })
            .await;
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let cancelled = Path::new("cancelled-run");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                run_retention_once(&completed, cancelled, std::future::pending())
            )
            .await
            .is_err()
        );
        assert!(!completed.lock().await.iter().any(|path| path == cancelled));
        run_retention_once(&completed, cancelled, async {
            calls.fetch_add(1, Ordering::Relaxed);
        })
        .await;
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn concurrent_automatic_retention_is_coalesced() {
        let completed = tokio::sync::Mutex::new(VecDeque::new());
        let calls = AtomicUsize::new(0);
        let root = Path::new("concurrent-run");
        let pass = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            tokio::task::yield_now().await;
        };
        tokio::join!(
            run_retention_once(&completed, root, pass()),
            run_retention_once(&completed, root, pass())
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
