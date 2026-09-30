//! C3 uses the real pool/cache fences; no duplicate session or epoch registry.
use dashmap::DashMap;
use labby_auth::upstream::cache::OauthClientCache;
use labby_gateway::upstream::pool::UpstreamPool;
use std::sync::Arc;

fn cache() -> OauthClientCache {
    OauthClientCache::new(Arc::new(DashMap::new()))
}

#[tokio::test]
async fn subject_invalidation_fences_only_the_affected_observations() {
    let cache = cache();
    let pool = UpstreamPool::new().with_oauth_client_cache(cache.clone());
    let affected = cache.lifecycle_epoch_for("alpha", "owner-a");
    let other_owner = cache.lifecycle_epoch_for("alpha", "owner-b");
    let other_server = cache.lifecycle_epoch_for("beta", "owner-a");
    pool.invalidate_oauth_subject_sessions("alpha", "owner-a", "test.lane-c.subject")
        .await;
    assert!(!affected.is_current());
    assert!(other_owner.is_current());
    assert!(other_server.is_current());
    pool.drain_for_swap("test.lane-c.cleanup").await;
}

#[tokio::test]
async fn upstream_invalidation_fences_all_its_owners_not_other_upstreams() {
    let cache = cache();
    let pool = UpstreamPool::new().with_oauth_client_cache(cache.clone());
    let first = cache.lifecycle_epoch_for("alpha", "owner-a");
    let second = cache.lifecycle_epoch_for("alpha", "owner-b");
    let unrelated = cache.lifecycle_epoch_for("beta", "owner-a");
    pool.invalidate_oauth_upstream_sessions(&["alpha".into()], "test.lane-c.upstream")
        .await;
    assert!(!first.is_current());
    assert!(!second.is_current());
    assert!(unrelated.is_current());
    pool.drain_for_swap("test.lane-c.cleanup").await;
}

#[tokio::test]
async fn same_owner_refresh_requires_a_fresh_observation_not_a_reused_epoch() {
    let cache = cache();
    let before = cache.lifecycle_epoch_for("alpha", "owner-a");
    let barrier = cache.invalidation_barrier();
    {
        let _writer = barrier.write().await;
        cache.advance_subject_epoch("alpha", "owner-a");
    }
    let after = cache.lifecycle_epoch_for("alpha", "owner-a");
    assert!(!before.is_current());
    assert!(after.is_current());
    // This only qualifies a new observation. It is NOT proof of a valid credential,
    // an authorized TaskRoute, durable non-revocation, or permission to dispatch.
}

#[tokio::test]
async fn lifecycle_writer_and_publication_reader_have_a_deterministic_order() {
    let cache = cache();
    let pool = UpstreamPool::new().with_oauth_client_cache(cache.clone());
    let observed = cache.lifecycle_epoch_for("alpha", "owner-a");
    let barrier = cache.invalidation_barrier();
    let reader = barrier.read().await;
    let invalidation =
        pool.invalidate_oauth_subject_sessions("alpha", "owner-a", "test.lane-c.order");
    tokio::pin!(invalidation);
    assert!(futures::poll!(invalidation.as_mut()).is_pending());
    assert!(observed.is_current());
    drop(reader);
    invalidation.await;
    let _publication = barrier.read().await;
    assert!(!observed.is_current());
}

#[tokio::test]
async fn late_observation_after_an_await_cannot_pass_the_existing_epoch_fence() {
    let cache = cache();
    let pool = UpstreamPool::new().with_oauth_client_cache(cache.clone());
    let before_io = cache.lifecycle_epoch_for("alpha", "owner-a");
    let barrier = cache.invalidation_barrier();
    let (finish_io, receive) = tokio::sync::oneshot::channel();
    let publication = async {
        receive.await.unwrap();
        let _guard = barrier.read().await;
        before_io.is_current()
    };
    tokio::pin!(publication);
    assert!(futures::poll!(publication.as_mut()).is_pending());
    pool.invalidate_oauth_subject_sessions("alpha", "owner-a", "test.lane-c.late")
        .await;
    finish_io.send(()).unwrap();
    assert!(!publication.await);
    pool.drain_for_swap("test.lane-c.cleanup").await;
}

#[tokio::test]
async fn replacement_pools_share_credential_fencing_but_not_pool_generation() {
    let cache = cache();
    let old_pool = UpstreamPool::new().with_oauth_client_cache(cache.clone());
    let new_pool = UpstreamPool::new().with_oauth_client_cache(cache.clone());
    assert_ne!(old_pool.revision_label(), new_pool.revision_label());
    let old_observation = cache.lifecycle_epoch_for("alpha", "owner-a");
    new_pool
        .invalidate_oauth_subject_sessions("alpha", "owner-a", "test.lane-c.new-pool")
        .await;
    assert!(!old_observation.is_current());
    old_pool.drain_for_swap("test.lane-c.old-cleanup").await;
    new_pool.drain_for_swap("test.lane-c.new-cleanup").await;
}

#[tokio::test]
async fn new_process_local_epoch_cannot_stand_in_for_durable_revocation() {
    let before_restart = cache();
    let old = before_restart.lifecycle_epoch_for("alpha", "owner-a");
    let barrier = before_restart.invalidation_barrier();
    {
        let _writer = barrier.write().await;
        before_restart.advance_subject_epoch("alpha", "owner-a");
    }
    assert!(!old.is_current());
    let after_restart = cache();
    let new = after_restart.lifecycle_epoch_for("alpha", "owner-a");
    assert!(new.is_current());
    // The new epoch knows nothing about prior revocation. B's durable binding
    // revision/revocation must be checked before C2 can acquire or dispatch.
}

#[cfg(feature = "testkit")]
#[tokio::test]
async fn snapshot_does_not_keep_a_drained_in_process_connection_alive() {
    use super::snapshot::ServerSnapshot;
    use super::snapshot_tests::{context, modern};
    use rmcp::model::ProtocolVersion;
    #[derive(Clone)]
    struct EmptyServer;
    impl rmcp::ServerHandler for EmptyServer {}
    let old_pool = UpstreamPool::new();
    old_pool
        .install_tool_server_for_tests("alpha", EmptyServer)
        .await;
    let retained =
        ServerSnapshot::from_discovery(context(), modern(), ProtocolVersion::V_2026_07_28).unwrap();
    assert_eq!(old_pool.connection_count_for_tests().await, 1);
    old_pool.drain_for_swap("test.lane-c.transport-death").await;
    assert_eq!(old_pool.connection_count_for_tests().await, 0);
    assert!(old_pool.cached_upstream_summary("alpha").await.is_none());
    assert_eq!(retained.raw_discovery().unwrap(), &modern());
    let replacement = UpstreamPool::new();
    replacement
        .install_tool_server_for_tests("alpha", EmptyServer)
        .await;
    assert_ne!(old_pool.revision_label(), replacement.revision_label());
    assert_eq!(replacement.connection_count_for_tests().await, 1);
    replacement.drain_for_swap("test.lane-c.cleanup").await;
}
