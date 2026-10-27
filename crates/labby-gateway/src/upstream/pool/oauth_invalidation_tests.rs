use super::super::testsupport::{named_test_upstream_config, static_catalog_pool};
use super::*;
use labby_auth::upstream::cache::OauthClientCache;

async fn fixture() -> (UpstreamPool, OauthClientCache) {
    let cache = OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
    let pool = Arc::try_unwrap(static_catalog_pool("alpha").await)
        .ok()
        .expect("single fixture owner")
        .with_oauth_client_cache(cache.clone());
    pool.generic_oauth_subjects
        .write()
        .await
        .insert("alpha".into(), "alice".into());
    let other = static_catalog_pool("beta").await;
    let connection = other.connections.write().await.remove("beta").unwrap();
    pool.connections
        .write()
        .await
        .insert("beta".into(), connection);
    pool.generic_oauth_subjects
        .write()
        .await
        .insert("beta".into(), "bob".into());
    (pool, cache)
}

#[tokio::test]
async fn rejected_refresh_waits_for_boundary_then_detaches_only_its_authority() {
    let (pool, cache) = fixture().await;
    let config = named_test_upstream_config("alpha");
    let before = cache.lifecycle_epoch_for("alpha", "alice");
    let unrelated = cache.lifecycle_epoch_for("beta", "bob");
    let callback = cache
        .refresh_rejection_handler(&config, "alice")
        .await
        .expect("rejection handler");
    let reader = cache.invalidation_barrier().read_owned().await;
    callback(None);
    callback(None);
    // Callback cannot wait for the writer while its own RPC owns a reader.
    assert!(before.is_current());
    assert!(pool.connections.read().await.contains_key("alpha"));
    drop(reader);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pool.drain_oauth_client_capacity_evictions(),
    )
    .await
    .expect("boundary drains promptly");
    assert!(!before.is_current());
    assert!(unrelated.is_current());
    let connections = pool.connections.read().await;
    assert!(!connections.contains_key("alpha"));
    assert!(connections.contains_key("beta"));
    assert!(cache.take_pending_invalidations().is_empty());
}

#[tokio::test]
async fn cancelled_writer_waiter_preserves_queued_rejection() {
    let (pool, cache) = fixture().await;
    let pool = Arc::new(pool);
    let config = named_test_upstream_config("alpha");
    let callback = cache
        .refresh_rejection_handler(&config, "alice")
        .await
        .expect("rejection handler");
    callback(None);
    let writer = cache.invalidation_barrier().write_owned().await;
    let (started, waiting) = tokio::sync::oneshot::channel();
    let draining_pool = Arc::clone(&pool);
    let drain = tokio::spawn(async move {
        let _ = started.send(());
        draining_pool.drain_oauth_client_capacity_evictions().await;
    });
    waiting.await.expect("drain starts");
    // The spawned task polls the drain until the held writer blocks it
    // before this receiver can resume on the single-thread test runtime.
    drain.abort();
    assert!(drain.await.expect_err("drain cancelled").is_cancelled());
    assert!(
        cache.has_pending_invalidations(),
        "cancelled waiter must not consume lifecycle work"
    );
    drop(writer);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pool.drain_oauth_client_capacity_evictions(),
    )
    .await
    .expect("next boundary drains");
    assert!(!pool.connections.read().await.contains_key("alpha"));
    assert!(!cache.has_pending_invalidations());
}

#[tokio::test]
async fn cancelled_boundaries_share_one_owned_drain() {
    let (pool, cache) = fixture().await;
    let pool = Arc::new(pool);
    let callback = cache
        .refresh_rejection_handler(&named_test_upstream_config("alpha"), "alice")
        .await
        .expect("rejection handler");
    let epoch = cache.lifecycle_epoch_for("alpha", "alice");
    callback(None);
    let writer = cache.invalidation_barrier().write_owned().await;
    let baseline = Arc::strong_count(&pool.connections);
    for _ in 0..32 {
        let (started, waiting) = tokio::sync::oneshot::channel();
        let caller_pool = Arc::clone(&pool);
        let caller = tokio::spawn(async move {
            let _ = started.send(());
            caller_pool.drain_oauth_client_capacity_evictions().await;
        });
        waiting.await.expect("caller polls to held writer");
        caller.abort();
        assert!(caller.await.expect_err("caller cancelled").is_cancelled());
    }
    // Each detached worker owns a pool clone. Cancelled callers must leave
    // only one worker retaining the live connection registry.
    assert_eq!(Arc::strong_count(&pool.connections), baseline + 1);
    assert!(epoch.is_current());
    drop(writer);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while pool.connections.read().await.contains_key("alpha") {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("shared worker completes without any surviving caller");
    assert!(!epoch.is_current());
    assert!(pool.connections.read().await.contains_key("beta"));
    assert!(!cache.has_pending_invalidations());
}

#[tokio::test]
async fn cancelled_boundary_finishes_invalidation_after_epoch_advance() {
    let (pool, cache) = fixture().await;
    let pool = Arc::new(pool);
    let config = named_test_upstream_config("alpha");
    let callback = cache
        .refresh_rejection_handler(&config, "alice")
        .await
        .expect("rejection handler");
    let epoch = cache.lifecycle_epoch_for("alpha", "alice");
    let connections = pool.connections.write().await;
    callback(None);
    let draining_pool = Arc::clone(&pool);
    let drain = tokio::spawn(async move {
        draining_pool.drain_oauth_client_capacity_evictions().await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while epoch.is_current() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("invalidation reaches the blocked registry after advancing its epoch");
    drain.abort();
    assert!(drain.await.expect_err("boundary cancelled").is_cancelled());
    assert!(connections.contains_key("alpha"));
    drop(connections);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if !pool.connections.read().await.contains_key("alpha") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned invalidation completes after caller cancellation");
    assert!(pool.connections.read().await.contains_key("beta"));
}

#[tokio::test]
async fn queued_rejection_cannot_invalidate_newer_authorization() {
    let (pool, cache) = fixture().await;
    let config = named_test_upstream_config("alpha");
    let callback = cache
        .refresh_rejection_handler(&config, "alice")
        .await
        .expect("rejection handler");
    callback(None);
    {
        let _writer = cache.invalidation_barrier().write_owned().await;
        // Authorization replacement uses this existing lifecycle fence.
        cache.advance_subject_epoch("alpha", "alice");
    }
    let replacement = cache.lifecycle_epoch_for("alpha", "alice");
    pool.drain_oauth_client_capacity_evictions().await;
    assert!(
        replacement.is_current(),
        "old rejection must not advance the new epoch"
    );
    assert!(pool.connections.read().await.contains_key("alpha"));
    assert!(
        pool.generic_oauth_subjects
            .read()
            .await
            .contains_key("alpha")
    );
}

/// Per-pool synchronization, compiled out of production drain workers.
#[derive(Default)]
pub(crate) struct DrainProbe {
    starts: std::sync::atomic::AtomicUsize,
    active: std::sync::atomic::AtomicUsize,
    final_snapshot: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

impl DrainProbe {
    pub(super) fn started(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        assert_eq!(self.active.fetch_add(1, SeqCst), 0, "only one drain worker");
        self.starts.fetch_add(1, SeqCst);
    }

    pub(super) async fn finishing(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        if self.starts.load(SeqCst) == 1 {
            self.final_snapshot.notify_one();
            self.resume.notified().await;
        }
        assert_eq!(self.active.fetch_sub(1, SeqCst), 1);
    }
}

#[tokio::test]
async fn final_empty_snapshot_handoff_drains_arrivals_and_reuses_completed_slot() {
    use std::sync::atomic::Ordering::SeqCst;

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let (mut pool, cache) = fixture().await;
        let probe = Arc::new(DrainProbe::default());
        pool.oauth_invalidation_drain_probe = Some(Arc::clone(&probe));
        let pool = Arc::new(pool);
        let alpha = named_test_upstream_config("alpha");
        let beta = named_test_upstream_config("beta");
        let first = cache.lifecycle_epoch_for("alpha", "alice");
        let second = cache.lifecycle_epoch_for("beta", "bob");
        let unrelated = cache.lifecycle_epoch_for("alpha", "charlie");
        cache
            .refresh_rejection_handler(&alpha, "alice")
            .await
            .unwrap()(None);
        let caller_pool = Arc::clone(&pool);
        let caller = tokio::spawn(async move {
            caller_pool.drain_oauth_client_capacity_evictions().await;
        });

        // The worker has taken its final empty snapshot but has not completed.
        // This arrival must be picked up by the boundary's completion recheck.
        probe.final_snapshot.notified().await;
        assert!(!first.is_current());
        assert!(second.is_current());
        assert!(!cache.has_pending_invalidations());
        assert_eq!(probe.starts.load(SeqCst), 1);
        assert_eq!(probe.active.load(SeqCst), 1);
        cache.refresh_rejection_handler(&beta, "bob").await.unwrap()(None);
        assert!(cache.has_pending_invalidations());
        probe.resume.notify_one();
        caller.await.expect("boundary completes");
        assert!(!second.is_current(), "handoff arrival was drained");
        assert!(!cache.has_pending_invalidations());
        assert_eq!(probe.starts.load(SeqCst), 2);
        assert_eq!(probe.active.load(SeqCst), 0);
        assert!(pool.connections.read().await.is_empty());
        assert!(unrelated.is_current());

        // A later authorization's rejection must replace the retained completed
        // shared future, rather than returning with its new event still queued.
        let later = cache.lifecycle_epoch_for("alpha", "alice");
        assert!(later.is_current());
        cache
            .refresh_rejection_handler(&alpha, "alice")
            .await
            .unwrap()(None);
        pool.drain_oauth_client_capacity_evictions().await;
        assert!(!later.is_current());
        assert!(!cache.has_pending_invalidations());
        assert_eq!(probe.starts.load(SeqCst), 3);
        assert_eq!(probe.active.load(SeqCst), 0);
        assert!(unrelated.is_current());
        pool.drain_oauth_client_capacity_evictions().await;
        assert_eq!(probe.starts.load(SeqCst), 3, "empty drain does not spawn");
    })
    .await
    .expect("deterministic drain handoff completes");
}
