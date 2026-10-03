//! Process-only status observations; authorization and capability health stay caller-local.
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use super::{
    GatewayManager, PersistedGatewayRuntimeEntry, PersistedGatewayRuntimeState,
    PoolPublicationGeneration, UpstreamPool, UpstreamRuntimeMetadata, epoch_now_secs,
    likely_stale_process_groups, matching_processes, persisted_runtime_process_still_matches,
    redacted_gateway_target, registered_runtime_identities, run_runtime_inspection,
    runtime_owner_view, runtime_process_start_ticks, system_time_to_epoch_secs,
    upstream_cleanup_patterns,
};
use labby_runtime::error::ToolError;

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiveIdentity {
    name: String,
    pid: Option<u32>,
    pgid: Option<u32>,
    started_at: Option<SystemTime>,
    generation: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SnapshotKey {
    config_generation: u64,
    pool_generation: PoolPublicationGeneration,
    live: Vec<LiveIdentity>,
}

#[derive(Debug, Default)]
struct RuntimeProcessSnapshot {
    persisted: PersistedGatewayRuntimeState,
    stale_groups: HashMap<String, std::collections::BTreeSet<String>>,
}

struct CachedSnapshot {
    key: SnapshotKey,
    completed_at: tokio::time::Instant,
    snapshot: Arc<RuntimeProcessSnapshot>,
}

/// Coalesces process observations only; callers still compute their own health and scope.
#[derive(Default)]
pub(in crate::gateway) struct RuntimeProcessSnapshotCache {
    state: tokio::sync::Mutex<Option<CachedSnapshot>>,
}

impl RuntimeProcessSnapshotCache {
    async fn get_or_collect<F, Fut>(
        &self,
        key: SnapshotKey,
        collect: F,
    ) -> Result<Arc<RuntimeProcessSnapshot>, ToolError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<RuntimeProcessSnapshot, ToolError>>,
    {
        // Keeping the gate across collection avoids detached leaders. A cancelled
        // builder releases it automatically, and failures never enter the cache.
        let mut state = self.state.lock().await;
        if let Some(cached) = state.as_ref()
            && cached.key == key
            && cached.completed_at.elapsed() < std::time::Duration::from_secs(1)
        {
            return Ok(Arc::clone(&cached.snapshot));
        }
        let snapshot = Arc::new(collect().await?);
        *state = Some(CachedSnapshot {
            key,
            completed_at: tokio::time::Instant::now(),
            snapshot: Arc::clone(&snapshot),
        });
        Ok(snapshot)
    }
}

pub(super) struct RuntimeStatusProcessContext {
    pub(super) config: super::GatewayConfig,
    pub(super) pool: Option<Arc<UpstreamPool>>,
    pub(super) live: BTreeMap<String, UpstreamRuntimeMetadata>,
    snapshot: Arc<RuntimeProcessSnapshot>,
}

impl RuntimeStatusProcessContext {
    pub(super) fn persisted(&self) -> &PersistedGatewayRuntimeState {
        &self.snapshot.persisted
    }
    pub(super) fn stale_groups(&self, name: &str) -> std::collections::BTreeSet<String> {
        self.snapshot
            .stale_groups
            .get(name)
            .cloned()
            .unwrap_or_default()
    }
}

impl GatewayManager {
    pub(super) async fn runtime_status_process_context(
        &self,
    ) -> Result<RuntimeStatusProcessContext, ToolError> {
        // Clone coherent in-memory observations only. No filesystem or process
        // scan runs under the short publication lease.
        let (config, pool, live, key) = {
            let _publication = self.publication_barrier.read().await;
            let config = self.config.read().await.clone();
            let publication = self.runtime.published_pool_snapshot();
            let pool = publication.pool().cloned();
            let live = match pool.as_deref() {
                Some(pool) => pool.upstream_runtime_metadata_snapshot().await,
                None => BTreeMap::new(),
            };
            let mut identities: Vec<_> = live
                .iter()
                .map(|(name, runtime)| LiveIdentity {
                    name: name.clone(),
                    pid: runtime.pid,
                    pgid: runtime.pgid,
                    started_at: runtime.started_at,
                    generation: runtime.generation,
                })
                .collect();
            identities.sort_by(|left, right| left.name.cmp(&right.name));
            let key = SnapshotKey {
                config_generation: self
                    .runtime_config_generation
                    .load(std::sync::atomic::Ordering::Acquire),
                pool_generation: publication.generation(),
                live: identities,
            };
            (config, pool, live, key)
        };
        let snapshot = self
            .runtime_process_status_cache
            .get_or_collect(key, || {
                self.collect_runtime_status_processes(&config, &live)
            })
            .await?;
        Ok(RuntimeStatusProcessContext {
            config,
            pool,
            live,
            snapshot,
        })
    }

    async fn collect_runtime_status_processes(
        &self,
        config: &super::GatewayConfig,
        live: &BTreeMap<String, UpstreamRuntimeMetadata>,
    ) -> Result<RuntimeProcessSnapshot, ToolError> {
        let state = self.load_runtime_state().await?;
        let upstreams = config.upstream.clone();
        let registered = registered_runtime_identities(live.values());
        let live_processes: HashMap<_, _> = live
            .iter()
            .filter_map(|(name, runtime)| {
                runtime
                    .pid
                    .map(|pid| (name.clone(), (pid, runtime.pgid.unwrap_or(pid))))
            })
            .collect();
        let entries: Vec<_> = config
            .upstream
            .iter()
            .filter_map(|upstream| {
                let runtime = live.get(&upstream.name)?;
                Some(PersistedGatewayRuntimeEntry {
                    upstream: upstream.name.clone(),
                    pid: runtime.pid?,
                    pgid: runtime.pgid,
                    started_at_epoch_secs: runtime.started_at.and_then(system_time_to_epoch_secs),
                    process_start_ticks: None,
                    observed_at_epoch_secs: epoch_now_secs(),
                    origin: runtime.origin.clone(),
                    owner: runtime.owner.as_ref().map(runtime_owner_view),
                    transport: Some(
                        if upstream.command.is_some() {
                            "stdio"
                        } else {
                            "http"
                        }
                        .to_string(),
                    ),
                    target: redacted_gateway_target(upstream),
                })
            })
            .collect();
        let snapshot = run_runtime_inspection(move || {
            let mut state = state;
            state
                .entries
                .retain(persisted_runtime_process_still_matches);
            for mut entry in entries {
                state
                    .entries
                    .retain(|old| !(old.upstream == entry.upstream && old.pid == entry.pid));
                entry.process_start_ticks = runtime_process_start_ticks(entry.pid);
                state.entries.push(entry);
            }
            state.reconciled_at_epoch_secs = Some(epoch_now_secs());
            state.entries.sort_by(|left, right| {
                left.upstream
                    .cmp(&right.upstream)
                    .then(left.pid.cmp(&right.pid))
            });
            let patterns: Vec<_> = upstreams
                .iter()
                .filter(|upstream| upstream.command.is_some())
                .flat_map(|upstream| upstream_cleanup_patterns(upstream, false))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let matches = if patterns.is_empty() {
                Vec::new()
            } else {
                matching_processes(&patterns)
            };
            let stale_groups = upstreams
                .iter()
                .filter(|upstream| upstream.command.is_some())
                .map(|upstream| {
                    (
                        upstream.name.clone(),
                        likely_stale_process_groups(
                            upstream,
                            live_processes.get(&upstream.name).copied(),
                            &registered,
                            &matches,
                        ),
                    )
                })
                .collect();
            RuntimeProcessSnapshot {
                persisted: state,
                stale_groups,
            }
        })
        .await?;
        self.persist_runtime_state(&snapshot.persisted).await?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn key() -> SnapshotKey {
        SnapshotKey {
            config_generation: 1,
            pool_generation: PoolPublicationGeneration(1),
            live: vec![],
        }
    }

    #[tokio::test]
    async fn concurrent_runtime_status_reads_collect_one_process_snapshot() {
        let cache = Arc::new(RuntimeProcessSnapshotCache::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let first_cache = Arc::clone(&cache);
        let first_calls = Arc::clone(&calls);
        let first_entered = Arc::clone(&entered);
        let first_release = Arc::clone(&release);
        let first = tokio::spawn(async move {
            first_cache
                .get_or_collect(key(), || async {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    first_entered.notify_one();
                    first_release.notified().await;
                    Ok(RuntimeProcessSnapshot::default())
                })
                .await
                .unwrap()
        });
        entered.notified().await;
        let second_cache = Arc::clone(&cache);
        let second_calls = Arc::clone(&calls);
        let second = tokio::spawn(async move {
            second_cache
                .get_or_collect(key(), || async {
                    second_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(RuntimeProcessSnapshot::default())
                })
                .await
                .unwrap()
        });
        tokio::task::yield_now().await;
        release.notify_one();
        let (first, second) = (first.await.unwrap(), second.await.unwrap());
        assert!(
            Arc::ptr_eq(&first, &second),
            "concurrent readers must share the completed snapshot"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn runtime_snapshot_expiry_and_identity_changes_force_collection() {
        let cache = RuntimeProcessSnapshotCache::default();
        let collect = || async { Ok(RuntimeProcessSnapshot::default()) };
        let first = cache.get_or_collect(key(), collect).await.unwrap();
        let same = cache.get_or_collect(key(), collect).await.unwrap();
        assert!(
            Arc::ptr_eq(&first, &same),
            "fresh matching identity is reusable"
        );
        tokio::time::advance(std::time::Duration::from_millis(1001)).await;
        let expired = cache.get_or_collect(key(), collect).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &expired));
        let mut changed = key();
        changed.config_generation += 1;
        let config = cache
            .get_or_collect(changed.clone(), collect)
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&expired, &config));
        changed.pool_generation = PoolPublicationGeneration(2);
        let pool = cache
            .get_or_collect(changed.clone(), collect)
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&config, &pool));
        changed.live.push(LiveIdentity {
            name: "up".into(),
            pid: Some(1),
            pgid: Some(1),
            started_at: None,
            generation: Some(1),
        });
        let pid = cache
            .get_or_collect(changed.clone(), collect)
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&pool, &pid));
        changed.live[0].generation = Some(2);
        let restarted = cache.get_or_collect(changed, collect).await.unwrap();
        assert!(!Arc::ptr_eq(&pid, &restarted));
    }

    #[tokio::test]
    async fn failed_runtime_snapshot_collection_is_not_cached() {
        let cache = RuntimeProcessSnapshotCache::default();
        cache
            .get_or_collect(key(), || async {
                Err(ToolError::internal_message("collection failed"))
            })
            .await
            .expect_err("collection fails");
        cache
            .get_or_collect(key(), || async { Ok(RuntimeProcessSnapshot::default()) })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancelled_runtime_snapshot_builder_releases_the_gate() {
        let cache = Arc::new(RuntimeProcessSnapshotCache::default());
        let entered = Arc::new(tokio::sync::Notify::new());
        let first_cache = Arc::clone(&cache);
        let first_entered = Arc::clone(&entered);
        let first = tokio::spawn(async move {
            first_cache
                .get_or_collect(key(), || async {
                    first_entered.notify_one();
                    std::future::pending::<Result<RuntimeProcessSnapshot, ToolError>>().await
                })
                .await
        });
        entered.notified().await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            cache.get_or_collect(key(), || async { Ok(RuntimeProcessSnapshot::default()) }),
        )
        .await
        .expect("gate released")
        .expect("replacement snapshot");
    }
}
