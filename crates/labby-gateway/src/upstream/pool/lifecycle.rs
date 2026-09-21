//! Pool lifecycle: selective upstream reconciliation plus `drain_for_swap`, which
//! tears down all connections, probe tasks, and catalog state when the pool is
//! swapped out (e.g. on config reload).

use std::collections::HashSet;
use std::time::Instant;

use futures::{StreamExt, future::join_all};

use labby_runtime::gateway_config::UpstreamConfig;

use super::helpers::CONNECTION_SHUTDOWN_CONCURRENCY;
use super::relay::RelayClientHandler;
use super::{UpstreamConnection, UpstreamPool};

pub(crate) struct UpstreamReconcileGuards {
    _connect_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
}

pub(crate) struct UpstreamReconcileCleanup {
    regular: Vec<(String, UpstreamConnection)>,
    relay: Vec<(String, UpstreamConnection<RelayClientHandler>)>,
    removed_catalog_count: usize,
    cancelled_probe_count: usize,
}

impl UpstreamReconcileCleanup {
    pub(crate) async fn finish(self, reason: &'static str) -> (usize, usize, usize) {
        let regular_count = self.regular.len();
        let relay_count = self.relay.len();
        let regular = futures::stream::iter(self.regular)
            .map(|(name, connection)| async move { connection.shutdown(&name, reason).await })
            .buffer_unordered(CONNECTION_SHUTDOWN_CONCURRENCY)
            .collect::<Vec<_>>();
        let relay = futures::stream::iter(self.relay)
            .map(|(name, connection)| async move { connection.shutdown(&name, reason).await })
            .buffer_unordered(CONNECTION_SHUTDOWN_CONCURRENCY)
            .collect::<Vec<_>>();
        tokio::join!(regular, relay);
        (
            regular_count + relay_count,
            self.removed_catalog_count,
            self.cancelled_probe_count,
        )
    }
}

impl UpstreamPool {
    pub fn runtime_identity_matches(
        &self,
        origin: &Option<String>,
        owner: Option<&crate::upstream::types::UpstreamRuntimeOwner>,
    ) -> bool {
        self.runtime_origin == *origin && self.runtime_owner.as_ref() == owner
    }

    pub(crate) async fn prepare_lazy_upstream_reconcile(
        &self,
        reconnect_names: &HashSet<String>,
    ) -> UpstreamReconcileGuards {
        let mut names = reconnect_names.iter().collect::<Vec<_>>();
        names.sort_unstable();
        let mut connect_guards = Vec::with_capacity(names.len());
        for name in names {
            connect_guards.push(self.lazy_connect_lock(name).await.lock_owned().await);
        }
        UpstreamReconcileGuards {
            _connect_guards: connect_guards,
        }
    }

    pub(crate) async fn apply_lazy_upstream_reconcile(
        &self,
        configs: &[UpstreamConfig],
        reconnect_names: &HashSet<String>,
        _guards: &UpstreamReconcileGuards,
    ) -> UpstreamReconcileCleanup {
        for upstream_name in reconnect_names {
            if let Some(config) = configs.iter().find(|config| config.name == *upstream_name) {
                self.upstream_config_fingerprints.insert(
                    upstream_name.clone(),
                    crate::gateway::code_mode::catalog_cache::fingerprint(config),
                );
            } else {
                self.upstream_config_fingerprints.remove(upstream_name);
            }
        }

        let cancelled_probe_count = {
            let mut tasks = self.probe_tasks.write().await;
            let mut count = 0usize;
            for upstream_name in reconnect_names {
                if let Some(cancel) = tasks.remove(upstream_name) {
                    cancel.cancel();
                    count += 1;
                }
            }
            count
        };

        let mut regular = Vec::new();
        let mut relay = Vec::new();
        for upstream_name in reconnect_names {
            regular.extend(self.detach_subject_connections_for(upstream_name).await);
            relay.extend(self.detach_relay_connections_for(upstream_name).await);
        }

        let (generic, removed_catalog_count) = self
            .remove_connection_catalog_entries(reconnect_names.iter())
            .await;
        regular.extend(generic);
        self.seed_lazy_upstreams(configs).await;
        {
            let mut locks = self.lazy_connect_locks.write().await;
            for name in reconnect_names {
                locks.remove(name);
            }
        }

        UpstreamReconcileCleanup {
            regular,
            relay,
            removed_catalog_count,
            cancelled_probe_count,
        }
    }

    pub async fn reconcile_lazy_upstreams(
        &self,
        configs: &[UpstreamConfig],
        reconnect_names: &HashSet<String>,
        reason: &'static str,
    ) {
        let started = Instant::now();
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.reconcile",
            event = "start",
            operation = "pool.reconcile",
            reason,
            reconnect_count = reconnect_names.len(),
            "upstream pool selective reconcile start"
        );

        let guards = self.prepare_lazy_upstream_reconcile(reconnect_names).await;
        let cleanup = self
            .apply_lazy_upstream_reconcile(configs, reconnect_names, &guards)
            .await;
        drop(guards);
        let (drained_connection_count, removed_catalog_count, cancelled_probe_count) =
            cleanup.finish(reason).await;

        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.reconcile",
            event = "finish",
            operation = "pool.reconcile",
            reason,
            elapsed_ms = started.elapsed().as_millis(),
            removed_catalog_count,
            drained_connection_count,
            cancelled_probe_count,
            "upstream pool selective reconcile finish"
        );
    }

    pub async fn drain_for_swap(&self, reason: &'static str) {
        let _invocations = self.invocation_barrier.write().await;
        let started = Instant::now();
        let catalog_count = self.catalog.read().await.len();
        let connection_count = self.connections.read().await.len();
        let probe_task_count = self.probe_tasks.read().await.len();
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.drain",
            event = "start",
            operation = "pool.drain",
            reason,
            pool_size = catalog_count,
            connection_count,
            probe_task_count,
            "upstream pool drain start"
        );

        // Cancel the background subject-connection sweep task (P-H2) before
        // evicting connections so it does not race the drain.
        if let Some(cancel) = self.subject_sweep_task.write().await.take() {
            cancel.cancel();
        }
        let cancelled_probe_count = {
            let mut tasks = self.probe_tasks.write().await;
            let count = tasks.len();
            for cancel in tasks.values() {
                cancel.cancel();
            }
            tasks.clear();
            count
        };
        self.lifecycle_tasks.close();
        self.lifecycle_tasks.wait().await;

        // Evict all subject-scoped cached connections first so the
        // per-`(upstream, subject)` handles are dropped before the pool-level
        // connections are shut down (P-C1 cleanup).
        self.evict_all_subject_connections().await;

        // Evict all cached relay connections (and reap any stdio children they
        // hold) before the pool-level connections are torn down.
        self.evict_all_relay_connections().await;
        self.cancel_all_upstream_subscriptions().await;

        // Cancel and await stale-while-revalidate work before dropping the cache.
        // This prevents detached refreshes from outliving the pool generation and
        // complements the cache epoch fence that rejects late publication.
        self.skills_refresh_cancel.cancel();
        self.skills_refresh_tasks.close();
        self.skills_refresh_tasks.wait().await;

        // Drop every cached skill catalog. A snapshot that outlived the drain
        // would describe skills belonging to connections this pool no longer
        // holds, and a later read against it could route to a detached
        // upstream — the catalog must not survive the config it came from.
        self.clear_all_cached_skills().await;

        let (drained, drained_catalog_count) = self.drain_connection_catalog_bindings().await;
        let drained_connection_count = {
            let count = drained.len();
            // Shut down all connections in parallel so an N-upstream pool
            // drains in ~1 shutdown timeout rather than N × shutdown timeout
            // (P-H2).
            let futs: Vec<_> = drained
                .into_iter()
                .map(|(upstream_name, connection)| async move {
                    connection.shutdown(&upstream_name, reason).await;
                })
                .collect();
            join_all(futs).await;
            count
        };
        self.resource_upstreams.write().await.clear();

        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.drain",
            event = "finish",
            operation = "pool.drain",
            reason,
            elapsed_ms = started.elapsed().as_millis(),
            drained_catalog_count,
            drained_connection_count,
            cancelled_probe_count,
            "upstream pool drain finish"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::*;
    use super::UpstreamPool;

    /// P-H2: after `drain_for_swap`, the drained pool's connections, catalog,
    /// and subject-connection cache are all empty.  The purpose of the test is
    /// to confirm the parallel-drain path reaches completion correctly (not just
    /// that the serial path works) and that subject connections are evicted as
    /// part of the drain.
    #[tokio::test]
    async fn drain_for_swap_clears_all_pool_state() {
        let pool = static_catalog_pool("alpha").await;

        // Confirm the pool is non-empty before draining.
        assert_eq!(pool.connection_count_for_tests().await, 1);
        assert!(pool.cached_upstream_summary("alpha").await.is_some());

        pool.drain_for_swap("test.drain").await;

        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert!(pool.cached_upstream_summary("alpha").await.is_none());
        assert!(pool.subject_connections.read().await.is_empty());
    }

    /// P-H2: `drain_for_swap` on a pool with multiple upstreams completes
    /// correctly in parallel — all connections and catalog entries removed.
    #[tokio::test]
    async fn drain_for_swap_clears_multiple_upstreams_in_parallel() {
        // Build a pool with two independent upstreams using the in-process fixture.
        let pool = static_catalog_pool("alpha").await;
        pool.insert_live_tool_server_for_tests(
            "beta",
            std::sync::Arc::new(tokio::sync::RwLock::new(vec!["beta.tool".to_string()])),
        )
        .await;

        assert_eq!(pool.connection_count_for_tests().await, 2);

        pool.drain_for_swap("test.multi_drain").await;

        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert_eq!(pool.catalog.read().await.len(), 0);
    }

    /// P-H2 + pool-swap semantics: a FRESH pool installed BEFORE the old pool is
    /// drained is immediately accessible — the new pool's upstreams are reachable
    /// even while the old pool is being shut down.
    ///
    /// This exercises the build-first / swap / drain-after pattern introduced by
    /// P-H2 at the `GatewayManager` level, verified here at the pool level by
    /// simulating the swap sequence with two pools and an `Arc<RwLock<_>>` swap
    /// handle (analogous to `GatewayRuntimeHandle`).
    #[tokio::test]
    async fn selective_reconcile_advances_config_fence_before_late_publication() {
        use std::collections::HashSet;

        let pool = UpstreamPool::new();
        let old = named_test_upstream_config("alpha");
        pool.seed_lazy_upstreams(std::slice::from_ref(&old)).await;
        assert!(pool.upstream_config_matches(&old));

        let mut next = old.clone();
        next.url = Some("http://127.0.0.1:9999/mcp".to_string());
        let changed = HashSet::from(["alpha".to_string()]);
        let guards = pool.prepare_lazy_upstream_reconcile(&changed).await;
        let cleanup = pool
            .apply_lazy_upstream_reconcile(std::slice::from_ref(&next), &changed, &guards)
            .await;

        assert!(!pool.upstream_config_matches(&old));
        assert!(pool.upstream_config_matches(&next));
        drop(guards);
        cleanup.finish("test.config_fence").await;
    }

    #[tokio::test]
    async fn selective_reconcile_removes_deleted_config_fence() {
        use std::collections::HashSet;

        let pool = UpstreamPool::new();
        let old = named_test_upstream_config("alpha");
        pool.seed_lazy_upstreams(std::slice::from_ref(&old)).await;
        let changed = HashSet::from(["alpha".to_string()]);
        let guards = pool.prepare_lazy_upstream_reconcile(&changed).await;
        let cleanup = pool
            .apply_lazy_upstream_reconcile(&[], &changed, &guards)
            .await;

        assert!(!pool.upstream_config_matches(&old));
        drop(guards);
        cleanup.finish("test.config_removed").await;
    }

    #[tokio::test]
    async fn fresh_pool_reachable_while_old_pool_drains() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        // Old pool: has upstream "alpha".
        let old_pool = static_catalog_pool("alpha").await;
        assert_eq!(old_pool.connection_count_for_tests().await, 1);

        // Fresh pool: has upstream "beta" (simulates a reload that changed one upstream).
        let fresh_pool = static_catalog_pool("beta").await;

        // Simulate the swap: install fresh_pool into the shared handle FIRST.
        let handle: Arc<RwLock<Option<Arc<_>>>> =
            Arc::new(RwLock::new(Some(Arc::clone(&fresh_pool))));

        // Fresh pool is immediately reachable after swap.
        let live = handle.read().await.clone().expect("pool is live");
        assert!(live.cached_upstream_summary("beta").await.is_some());
        assert!(live.cached_upstream_summary("alpha").await.is_none());

        // Now drain the old pool (happens after the swap in the real reload path).
        old_pool.drain_for_swap("test.after_swap_drain").await;

        // Fresh pool (beta) is still intact — drain only affected old_pool.
        let live = handle
            .read()
            .await
            .clone()
            .expect("pool still live after drain");
        assert!(live.cached_upstream_summary("beta").await.is_some());
        assert_eq!(live.connection_count_for_tests().await, 1);
    }
}
