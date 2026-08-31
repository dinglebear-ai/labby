//! `UpstreamConnection` lifecycle: `Debug`, `Drop`, graceful `shutdown`, and the
//! pool's `acquire_peer` accessor.
//!
//! The `UpstreamConnection` and `UpstreamPool` struct definitions stay in
//! `pool.rs`; this descendant module only carries their `impl` bodies, so the
//! private fields are visible without annotation. `shutdown` and `acquire_peer`
//! are promoted to `pub(super)` because they are called from sibling modules
//! (`lifecycle`, `probe`, `ensure`, `tools_call`, `resources_read`,
//! `prompts_get`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[cfg(unix)]
use crate::process::unix::{
    pid_is_alive, terminate_process_group_sigkill, terminate_process_group_sigterm,
};

use tokio::sync::Mutex;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::helpers::{
    STDIO_SHUTDOWN_TIMEOUT, SUBJECT_CONN_IDLE_TTL, SUBJECT_CONN_MAX_ENTRIES,
    SUBJECT_CONN_SWEEP_INTERVAL,
};
use super::logging::capability_name;
use super::{SubjectScopedConnection, UpstreamConnection, UpstreamPool};

/// Evict least-recently-used subject connections until the map holds at most
/// `max_entries`, returning the removed `(upstream_name, connection)` pairs so
/// the caller can shut their peers down cleanly off-lock.
///
/// `protect` is the key about to be (re)inserted by the caller; it is never
/// chosen for eviction so a fresh connect is not torn down moments after it
/// opens.
fn evict_lru_over_cap(
    cache: &mut HashMap<(String, String), SubjectScopedConnection>,
    max_entries: usize,
    protect: &(String, String),
) -> Vec<(String, UpstreamConnection)> {
    let mut evicted = Vec::new();
    while cache.len() > max_entries {
        // Find the least-recently-used key that is not the protected key.
        let lru_key = cache
            .iter()
            .filter(|(k, _)| *k != protect)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(k, _)| k.clone());
        match lru_key {
            Some(key) => {
                if let Some(entry) = cache.remove(&key) {
                    evicted.push((key.0, entry._connection));
                }
            }
            // Only the protected key remains — nothing left to evict.
            None => break,
        }
    }
    evicted
}

impl<H: rmcp::ClientHandler> std::fmt::Debug for UpstreamConnection<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamConnection").finish_non_exhaustive()
    }
}

/// Sync Drop: reap the descendant process tree, then abort any in-process
/// server task. Last-resort abandonment cleanup for stdio upstreams whose
/// connect future was dropped without going through `shutdown()` —
/// discovery timeouts, cancelled `buffer_unordered` futures, pool drops,
/// `insert()` overwrites, etc.
///
/// The async `shutdown()` graceful path takes `self.runtime.pgid` (Unix)
/// or `self.runtime.job` (Windows) and takes
/// `_server_task` before its first `.await` so this Drop no-ops on the
/// graceful path.
///
/// - Unix: `SIGTERM` + `SIGKILL` the process group via `killpg`.
/// - Windows: close the Job Object handle; `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
///   causes the OS to terminate every process in the job (direct child +
///   all descendants).
/// - `_server_task.abort()` runs on all platforms.
impl<H: rmcp::ClientHandler> Drop for UpstreamConnection<H> {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.runtime.pgid.take() {
            // No sleep — Drop must not block. Kernel handles TERM/KILL race.
            if let Err(error) = terminate_process_group_sigterm(pgid) {
                tracing::warn!(
                    target: "upstream.connection",
                    pgid,
                    ?error,
                    "process group SIGTERM failed on drop"
                );
            }
            if let Err(error) = terminate_process_group_sigkill(pgid) {
                tracing::warn!(
                    target: "upstream.connection",
                    pgid,
                    ?error,
                    "process group SIGKILL failed on drop"
                );
            } else {
                tracing::debug!(
                    target: "upstream.connection",
                    pgid,
                    "process group reaped on connection drop"
                );
            }
        }
        #[cfg(windows)]
        {
            drop(self.runtime.job.take());
        }
        if let Some(handle) = self._server_task.take() {
            handle.abort();
        }
    }
}

impl<H: rmcp::ClientHandler> UpstreamConnection<H> {
    pub(crate) async fn shutdown(mut self, upstream_name: &str, reason: &'static str) {
        // Clone runtime BEFORE taking pgid / job so subsequent log
        // lines still surface the original values.
        let runtime = self.runtime.clone();
        // INVARIANT: take pgid (Unix) / job (Windows) BEFORE any
        // `.await` so the consuming Drop no-ops on the graceful path.
        #[cfg(unix)]
        let runtime_pgid = self.runtime.pgid.take();
        #[cfg(windows)]
        let runtime_job = self.runtime.job.take();
        let started = Instant::now();
        let result = self
            ._client_service
            .close_with_timeout(STDIO_SHUTDOWN_TIMEOUT)
            .await;
        if let Some(server_task) = self._server_task.take() {
            server_task.abort();
        }

        let mut termination_failures = Vec::new();

        #[cfg(unix)]
        if let (Some(pid), Some(pgid)) = (runtime.pid, runtime_pgid)
            && pid_is_alive(pid)
        {
            if let Err(error) = terminate_process_group_sigterm(pgid)
                && error != nix::errno::Errno::ESRCH
            {
                termination_failures.push(format!("SIGTERM process group {pgid}: {error}"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if pid_is_alive(pid)
                && let Err(error) = terminate_process_group_sigkill(pgid)
                && error != nix::errno::Errno::ESRCH
            {
                termination_failures.push(format!("SIGKILL process group {pgid}: {error}"));
            }
        }

        // Windows: close the Job Object handle. The OS terminates every
        // process in the job (direct child + grandchildren) because we set
        // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE at creation time. No async
        // sleep needed — the kernel handles tree termination synchronously.
        #[cfg(windows)]
        if let Some(job) = runtime_job
            && let Err(error) = job.close()
        {
            termination_failures.push(format!(
                "{} failed with Win32 error {}",
                error.operation, error.code
            ));
        }

        match (result, termination_failures.is_empty()) {
            (Ok(Some(_)), true) => tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.connection.shutdown",
                event = "finish",
                operation = "connection.shutdown",
                upstream = upstream_name,
                reason,
                pid = ?runtime.pid,
                pgid = ?runtime.pgid,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream connection shutdown finished"
            ),
            (Ok(Some(_)), false) => tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.connection.shutdown",
                event = "error",
                operation = "process_tree.terminate",
                upstream = upstream_name,
                reason,
                pid = ?runtime.pid,
                pgid = ?runtime.pgid,
                errors = ?termination_failures,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream MCP connection closed but process-tree termination failed"
            ),
            (Ok(None), _) => tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.connection.shutdown",
                event = "timeout",
                operation = "connection.shutdown",
                upstream = upstream_name,
                reason,
                pid = ?runtime.pid,
                pgid = ?runtime.pgid,
                termination_errors = ?termination_failures,
                timeout_ms = STDIO_SHUTDOWN_TIMEOUT.as_millis(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream connection shutdown timed out"
            ),
            (Err(error), _) => tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.connection.shutdown",
                event = "error",
                operation = "connection.shutdown",
                upstream = upstream_name,
                reason,
                pid = ?runtime.pid,
                pgid = ?runtime.pgid,
                error = %error,
                termination_errors = ?termination_failures,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream connection shutdown failed"
            ),
        }
    }
}

impl UpstreamPool {
    pub(super) async fn acquire_peer(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
        requested_operation: &'static str,
    ) -> Option<rmcp::service::Peer<rmcp::RoleClient>> {
        let acquire_started = Instant::now();
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.acquire",
            event = "start",
            operation = "connection.acquire",
            requested_operation,
            upstream = %upstream_name,
            capability = capability_name(capability),
            "upstream pool acquire start"
        );
        let connections = self.connections.read().await;
        let connection_count = connections.len();
        let peer = connections.get(upstream_name).map(|conn| conn.peer.clone());
        drop(connections);
        let pool_size = self.catalog.read().await.len();
        let elapsed_ms = acquire_started.elapsed().as_millis();
        if peer.is_some() {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.acquire",
                event = "finish",
                operation = "connection.acquire",
                requested_operation,
                upstream = %upstream_name,
                capability = capability_name(capability),
                elapsed_ms,
                pool_size,
                connection_count,
                "upstream pool acquire finish"
            );
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.acquire",
                event = "empty",
                operation = "connection.acquire",
                requested_operation,
                upstream = %upstream_name,
                capability = capability_name(capability),
                elapsed_ms,
                kind = "upstream_not_connected",
                pool_size,
                connection_count,
                "upstream pool acquire empty"
            );
        }
        peer
    }

    /// Return a cached per-`(upstream, subject)` peer, or open a new
    /// connection and cache it (P-C1).
    ///
    /// Fast path: the subject-connection cache is checked under a write lock
    /// so TTL eviction can happen inline.  If the cached entry is still fresh
    /// it is returned immediately without touching the network.
    ///
    /// Slow path: a per-`(upstream, subject)` mutex prevents concurrent first
    /// requests from opening duplicate connections (mirrors the
    /// `lazy_connect_locks` gate used by the normal non-OAuth pool path).
    /// After acquiring the lock the cache is re-checked, then
    /// `connect_upstream_with_client` is called and the result is stored.
    pub(super) async fn acquire_or_connect_subject(
        &self,
        config: &UpstreamConfig,
        subject: &str,
    ) -> anyhow::Result<(
        rmcp::service::Peer<rmcp::RoleClient>,
        Vec<rmcp::model::Tool>,
    )> {
        self.drain_oauth_client_capacity_evictions().await;
        self.acquire_or_connect_subject_guarded(config, subject)
            .await
    }

    /// Subject connection acquisition when the caller already holds the OAuth
    /// invalidation read barrier for the complete checked invocation.
    pub(super) async fn acquire_or_connect_subject_guarded(
        &self,
        config: &UpstreamConfig,
        subject: &str,
    ) -> anyhow::Result<(
        rmcp::service::Peer<rmcp::RoleClient>,
        Vec<rmcp::model::Tool>,
    )> {
        use super::connect::connect_upstream_with_client;

        let key = (config.name.clone(), subject.to_string());
        let lifecycle_epoch = self.oauth_lifecycle_epoch();

        // Fast path: check cache with inline TTL eviction (write lock allows
        // removing the stale entry atomically).
        {
            let mut cache = self.subject_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL {
                    entry.last_used = Instant::now();
                    return Ok((entry.peer.clone(), entry.tools.clone()));
                }
                // Entry is stale — evict it; the slow path will reconnect.
                cache.remove(&key);
            }
        }

        // First subject-scoped connect on this pool also arms the background
        // sweep task (idempotent — no-op once armed). This keeps the sweep dormant
        // until the OAuth/subject path is actually exercised (P-H2).
        self.ensure_subject_sweep_task().await;

        // Slow path: acquire the per-key single-flight lock so only one
        // concurrent caller opens a new connection.
        let connect_lock: Arc<Mutex<()>> = {
            let mut locks = self.subject_connect_locks.write().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = connect_lock.lock().await;

        let result = async {
            // Re-check after acquiring the lock — another waiter may have
            // already opened and cached the connection.
            {
                let mut cache = self.subject_connections.write().await;
                if let Some(entry) = cache.get_mut(&key) {
                    if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL {
                        entry.last_used = Instant::now();
                        return Ok((entry.peer.clone(), entry.tools.clone()));
                    }
                    cache.remove(&key);
                }
            }

            // Open a new connection, reusing the pool-level shared HTTP client.
            let (conn, tools) = connect_upstream_with_client(
                config,
                Some(subject),
                self.oauth_client_cache.as_ref(),
                self.runtime_origin.as_deref(),
                self.runtime_owner.as_ref(),
                Some(&self.shared_http_client),
            )
            .await?;

            let peer = conn.peer.clone();
            let cached_tools = tools.clone();
            // Network I/O completes without holding the lifecycle barrier.
            // Only the atomic epoch check plus cache publication is fenced.
            let _oauth_publication = self.oauth_publication_guard(lifecycle_epoch).await?;
            // Enforce the LRU cap BEFORE inserting so a burst of unique subjects
            // can't push the live-peer (and FD) count past the bound. Evicted
            // peers are shut down cleanly off-lock (P-H2).
            let evicted = {
                let mut cache = self.subject_connections.write().await;
                let evicted = evict_lru_over_cap(&mut cache, SUBJECT_CONN_MAX_ENTRIES - 1, &key);
                cache.insert(
                    key.clone(),
                    SubjectScopedConnection {
                        _connection: conn,
                        peer: peer.clone(),
                        tools: cached_tools,
                        last_used: Instant::now(),
                    },
                );
                evicted
            };
            for (name, evicted_conn) in evicted {
                evicted_conn
                    .shutdown(&name, "subject.cache.lru_evict")
                    .await;
            }
            Ok((peer, tools))
        }
        .await;

        // Bound `subject_connect_locks` growth: evict the lock entry once the
        // connect attempt has COMPLETED (success or error) and this is the sole
        // remaining reference (Arc strong_count == 2: map + our clone).
        // Eviction must not happen before the connect — a caller arriving
        // mid-connect would otherwise insert a fresh lock, acquire it
        // immediately, and race a duplicate connect, defeating single-flight.
        // After completion the cache is already populated (success) or a retry
        // is acceptable (error). New clones require the map write lock we hold
        // here, so the strong_count check cannot race an incoming waiter.
        {
            let mut locks = self.subject_connect_locks.write().await;
            if let Some(entry) = locks.get(&key)
                && Arc::strong_count(entry) <= 2
            {
                locks.remove(&key);
            }
        }

        result
    }

    /// Evict all subject-scoped connections for a single upstream.
    ///
    /// Called when an upstream is updated or removed so stale cached
    /// connections are not reused after the config changes.
    pub(super) async fn evict_subject_connections_for(&self, upstream_name: &str) {
        self.subject_connections
            .write()
            .await
            .retain(|(name, _), _| name != upstream_name);
    }

    /// Evict the cached connection for a single `(upstream, subject)` pair.
    ///
    /// Called when a subject-scoped request fails so the next attempt
    /// reconnects instead of reusing a dead peer until the idle TTL expires.
    /// Without this, a cached-but-broken peer stays sticky: the fast path
    /// refreshes `last_used` on every hit, so a client that keeps retrying a
    /// dead connection would never let it age out.
    pub(super) async fn evict_subject_connection(&self, upstream_name: &str, subject: &str) {
        let key = (upstream_name.to_string(), subject.to_string());
        self.subject_connections.write().await.remove(&key);
    }

    /// Evict all subject-scoped connections.
    ///
    /// Called during pool drain so cached connections are torn down cleanly
    /// before the pool is swapped out.
    pub(super) async fn evict_all_subject_connections(&self) {
        self.subject_connections.write().await.clear();
    }

    /// Sweep the subject-connection cache once (P-H2).
    ///
    /// Removes every `subject_connections` entry whose `last_used` exceeds the
    /// idle TTL (shutting its peer down cleanly), then prunes orphan
    /// `subject_connect_locks` — lock entries with no live connection and no
    /// other strong reference (`Arc::strong_count == 1`, i.e. only the map's
    /// own clone). Returns `(connections_evicted, locks_pruned)` for logging
    /// and tests.
    ///
    /// Eviction of locks is conservative: a lock currently held by an in-flight
    /// connect has `strong_count >= 2` (the connector holds a clone), so it is
    /// never pruned out from under a single-flight gate.
    pub(super) async fn sweep_subject_connections(&self) -> (usize, usize) {
        // Phase 1: drain idle-TTL-expired connection entries under the write
        // lock, but shut their peers down OUTSIDE the lock so a slow shutdown
        // does not stall fast-path cache hits.
        let expired = {
            let mut cache = self.subject_connections.write().await;
            let stale_keys: Vec<(String, String)> = cache
                .iter()
                .filter(|(_, entry)| entry.last_used.elapsed() >= SUBJECT_CONN_IDLE_TTL)
                .map(|(key, _)| key.clone())
                .collect();
            stale_keys
                .into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key.0, entry._connection)))
                .collect::<Vec<_>>()
        };
        let connections_evicted = expired.len();
        for (name, conn) in expired {
            conn.shutdown(&name, "subject.cache.sweep").await;
        }

        // Phase 2: prune orphan single-flight locks. Hold both locks so the
        // strong_count check cannot race a connect inserting/cloning a lock.
        let locks_pruned = {
            let cache = self.subject_connections.read().await;
            let mut locks = self.subject_connect_locks.write().await;
            let before = locks.len();
            locks.retain(|key, lock| {
                // Keep locks that still gate a live connection, or that another
                // task currently holds (in-flight connect: strong_count >= 2).
                cache.contains_key(key) || Arc::strong_count(lock) > 1
            });
            before - locks.len()
        };

        if connections_evicted > 0 || locks_pruned > 0 {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.subject.sweep",
                event = "finish",
                operation = "subject.cache.sweep",
                connections_evicted,
                locks_pruned,
                "subject connection cache sweep finished"
            );
        }
        (connections_evicted, locks_pruned)
    }

    /// Spawn the background subject-connection sweep loop (P-H2).
    ///
    /// Mirrors the cancelable-task pattern used by `ensure_probe_task`: the
    /// returned-by-side-effect task lives on `subject_sweep_task` and is
    /// cancelled by `drain_for_swap`. Idempotent — a second call while a task
    /// is already registered is a no-op.
    pub(super) async fn ensure_subject_sweep_task(&self) {
        {
            let mut slot = self.subject_sweep_task.write().await;
            if slot.is_some() {
                return;
            }
            *slot = Some(tokio_util::sync::CancellationToken::new());
        }
        let cancel = self
            .subject_sweep_task
            .read()
            .await
            .clone()
            .expect("sweep token just inserted");

        let pool = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(SUBJECT_CONN_SWEEP_INTERVAL) => {
                        pool.sweep_subject_connections().await;
                        pool.sweep_relay_connections().await;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use std::sync::Arc;

    use super::super::SubjectScopedConnection;
    use super::super::testsupport::*;

    /// P-C1: inserting a `SubjectScopedConnection` into the cache and calling
    /// `acquire_or_connect_subject` again with the same `(upstream, subject)`
    /// key returns the cached peer — no new connection is opened.
    ///
    /// The test exercises the fast path of `acquire_or_connect_subject` by
    /// directly populating the `subject_connections` map (simulating what the
    /// slow path would have stored on a first request) and verifying that:
    ///
    /// 1. The cache has exactly one entry after the fast-path hit.
    /// 2. The returned peer is the same handle as the one we inserted.
    /// 3. No new entry was added (cache size stays at 1).
    #[tokio::test]
    async fn subject_scoped_connection_cache_hit_reuses_entry() {
        use std::time::Instant;

        let pool = static_catalog_pool("alpha").await;

        // Grab the existing in-process connection from the pool (the "alpha"
        // upstream inserted by `static_catalog_pool`).
        let peer = pool
            .connections
            .read()
            .await
            .get("alpha")
            .expect("alpha connection present")
            .peer
            .clone();

        // Manually insert a SubjectScopedConnection for (alpha, alice).
        // Remove the whole UpstreamConnection from the pool so we can move it
        // into SubjectScopedConnection — UpstreamConnection implements Drop so
        // its fields cannot be moved out individually.
        let alpha_conn = pool
            .connections
            .write()
            .await
            .remove("alpha")
            .expect("remove alpha for reuse");
        let tools = vec![rmcp::model::Tool::new(
            "alpha.tool".to_string(),
            "a test tool",
            Arc::new(serde_json::Map::new()),
        )];
        pool.subject_connections.write().await.insert(
            ("alpha".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                _connection: alpha_conn,
                peer: peer.clone(),
                tools: tools.clone(),
                last_used: Instant::now(),
            },
        );

        assert_eq!(pool.subject_connections.read().await.len(), 1);

        // Now verify that the cache already has the entry and evict_subject works.
        pool.evict_subject_connections_for("alpha").await;
        assert_eq!(pool.subject_connections.read().await.len(), 0);
    }

    /// Clearing credentials must remove the initialized subject-scoped MCP
    /// peer, not only the token-producing client cache. Otherwise the old peer
    /// can keep executing until its idle TTL expires.
    #[tokio::test]
    async fn oauth_subject_invalidation_evicts_matching_subject_connection() {
        use std::time::Instant;

        let pool = static_catalog_pool("alpha").await;
        let alpha_conn = pool
            .connections
            .write()
            .await
            .remove("alpha")
            .expect("alpha connection");
        let peer = alpha_conn.peer.clone();
        pool.subject_connections.write().await.insert(
            ("alpha".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                _connection: alpha_conn,
                peer,
                tools: vec![],
                last_used: Instant::now(),
            },
        );

        let invalidated = pool
            .invalidate_oauth_subject_sessions("alpha", "alice", "oauth.credentials.clear")
            .await;

        assert_eq!(invalidated.subject_connections, 1);
        assert!(pool.subject_connections.read().await.is_empty());
    }

    #[tokio::test]
    async fn oauth_client_capacity_eviction_removes_the_same_live_subject_peer() {
        use std::time::Instant;

        use labby_auth::upstream::cache::OauthClientCache;
        use labby_runtime::gateway_config::{
            UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
        };
        use rmcp::transport::{AuthClient, AuthorizationManager};

        drop(rustls::crypto::ring::default_provider().install_default());
        let auth_manager = AuthorizationManager::new("http://localhost")
            .await
            .expect("authorization manager");
        let auth_client = Arc::new(AuthClient::new(reqwest::Client::new(), auth_manager));
        let client_cache =
            OauthClientCache::new(Arc::new(dashmap::DashMap::new())).with_client_capacity(2);
        let pool = Arc::try_unwrap(static_catalog_pool("alpha").await)
            .ok()
            .expect("test pool has one owner")
            .with_oauth_client_cache(client_cache.clone());

        let keys = [
            ("alpha", "alice"),
            ("beta", "bob"),
            ("gamma", "carol"),
            ("delta", "dana"),
            ("epsilon", "erin"),
            ("zeta", "zoe"),
            ("eta", "eve"),
        ];
        for (index, (upstream, subject)) in keys.iter().enumerate() {
            let connection = if index == 0 {
                pool.connections
                    .write()
                    .await
                    .remove(*upstream)
                    .expect("alpha connection")
            } else {
                static_catalog_pool(upstream)
                    .await
                    .connections
                    .write()
                    .await
                    .remove(*upstream)
                    .expect("fixture connection")
            };
            let peer = connection.peer.clone();
            pool.subject_connections.write().await.insert(
                ((*upstream).to_string(), (*subject).to_string()),
                SubjectScopedConnection {
                    _connection: connection,
                    peer,
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
            let mut config = named_test_upstream_config(upstream);
            config.url = Some(format!("https://{upstream}.example/mcp"));
            config.oauth = Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration: UpstreamOauthRegistration::Preregistered {
                    client_id: format!("{upstream}-client"),
                    client_secret_env: None,
                },
                scopes: None,
                credential: Default::default(),
                prefer_client_metadata_document: None,
            });
            client_cache
                .publish_prebuilt(&config, subject, Arc::clone(&auth_client))
                .expect("publish client");
        }

        assert_eq!(client_cache.ready_client_count(), 2);
        assert_eq!(pool.subject_connections.read().await.len(), keys.len());
        let epoch_before = client_cache.lifecycle_epoch();
        pool.drain_oauth_client_capacity_evictions().await;
        assert!(client_cache.lifecycle_epoch() > epoch_before);
        assert_eq!(client_cache.ready_client_count(), 2);
        assert_eq!(pool.subject_connections.read().await.len(), 2);

        let live = pool.subject_connections.read().await;
        for (upstream, subject) in keys {
            assert_eq!(
                live.contains_key(&(upstream.to_string(), subject.to_string())),
                client_cache.contains_ready_client(upstream, subject),
                "capacity drain must invalidate every evicted live subject peer"
            );
        }
    }

    #[tokio::test]
    async fn oauth_invalidation_evicts_generic_oauth_peer_but_preserves_raw_peer() {
        let oauth_pool = static_catalog_pool("oauth").await;
        oauth_pool
            .generic_oauth_subjects
            .write()
            .await
            .insert("oauth".to_string(), "alice".to_string());
        let raw_pool = static_catalog_pool("raw").await;
        let raw_connection = raw_pool
            .connections
            .write()
            .await
            .remove("raw")
            .expect("raw connection");
        oauth_pool
            .connections
            .write()
            .await
            .insert("raw".to_string(), raw_connection);

        let invalidated = oauth_pool
            .invalidate_oauth_upstream_sessions(&["oauth".to_string()], "oauth.provider.revoke")
            .await;

        assert_eq!(invalidated.generic_connections, 1);
        assert!(!oauth_pool.connections.read().await.contains_key("oauth"));
        assert!(oauth_pool.connections.read().await.contains_key("raw"));
    }

    #[tokio::test]
    async fn allowlist_subject_revocation_closes_generic_peer_before_next_use() {
        let pool = static_catalog_pool("shared-google").await;
        pool.generic_oauth_subjects
            .write()
            .await
            .insert("shared-google".to_string(), "removed-subject".to_string());
        assert!(pool.connections.read().await.contains_key("shared-google"));

        let invalidated = pool
            .invalidate_oauth_subject_sessions(
                "shared-google",
                "removed-subject",
                "auth.allowed_user.remove",
            )
            .await;

        assert_eq!(invalidated.generic_connections, 1);
        assert!(
            !pool.connections.read().await.contains_key("shared-google"),
            "the next upstream use must not reuse the peer authenticated by the removed credential"
        );
        assert!(
            !pool
                .generic_oauth_subjects
                .read()
                .await
                .contains_key("shared-google")
        );
    }

    /// P-C1: `evict_subject_connections_for` removes only entries keyed to the
    /// given upstream, leaving other upstreams' subject connections intact.
    ///
    /// The cache stores `SubjectScopedConnection` entries keyed by
    /// `(upstream_name, subject)`.  This test seeds the cache with two subjects
    /// on "alpha" and one subject on "beta", then evicts "alpha" and confirms
    /// only the "beta" entry survives.
    ///
    /// We use the `subject_connections` map directly because constructing a full
    /// `SubjectScopedConnection` requires a live async service; instead the test
    /// inserts the minimal structure needed to verify the eviction key predicate.
    #[tokio::test]
    async fn evict_subject_connections_for_is_scoped_to_upstream() {
        use std::time::Instant;

        // Build a pool with an "alpha" in-process service so we can reuse its
        // service handle for the SubjectScopedConnection `_connection` field.
        let pool = static_catalog_pool("alpha").await;

        // Drain the pool's connection map to get the alpha UpstreamConnection.
        let alpha_conn = pool
            .connections
            .write()
            .await
            .remove("alpha")
            .expect("alpha connection");
        let alpha_peer = alpha_conn.peer.clone();

        // Seed subject_connections with two different upstream keys.
        {
            let mut cache = pool.subject_connections.write().await;
            // (alpha, alice)
            cache.insert(
                ("alpha".to_string(), "alice".to_string()),
                SubjectScopedConnection {
                    _connection: alpha_conn,
                    peer: alpha_peer.clone(),
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
        }

        // Build a separate in-process "beta" service for the second entry.
        let beta_pool = static_catalog_pool("beta").await;
        let beta_conn = beta_pool
            .connections
            .write()
            .await
            .remove("beta")
            .expect("beta connection");
        let beta_peer = beta_conn.peer.clone();

        {
            let mut cache = pool.subject_connections.write().await;
            // (beta, alice)
            cache.insert(
                ("beta".to_string(), "alice".to_string()),
                SubjectScopedConnection {
                    _connection: beta_conn,
                    peer: beta_peer.clone(),
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
        }

        assert_eq!(pool.subject_connections.read().await.len(), 2);

        // Evict only "alpha" entries.
        pool.evict_subject_connections_for("alpha").await;

        let remaining: Vec<_> = pool
            .subject_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, "beta");
    }

    /// P-H2: `sweep_subject_connections` removes entries whose `last_used`
    /// exceeds `SUBJECT_CONN_IDLE_TTL` (simulated by back-dating `last_used`)
    /// while leaving fresh entries in place. Without the sweep these idle
    /// entries would leak one live peer + FD per subject forever.
    #[tokio::test]
    async fn sweep_evicts_idle_ttl_expired_subject_connections() {
        use std::time::{Duration, Instant};

        use super::super::helpers::SUBJECT_CONN_IDLE_TTL;

        let pool = static_catalog_pool("alpha").await;
        let stale_conn = pool
            .connections
            .write()
            .await
            .remove("alpha")
            .expect("alpha connection for stale entry");
        let stale_peer = stale_conn.peer.clone();

        // Fresh entry: borrow beta's connection so it survives the sweep.
        let beta_pool = static_catalog_pool("beta").await;
        let fresh_conn = beta_pool
            .connections
            .write()
            .await
            .remove("beta")
            .expect("beta connection for fresh entry");
        let fresh_peer = fresh_conn.peer.clone();

        {
            let mut cache = pool.subject_connections.write().await;
            // Stale: last_used well past the idle TTL.
            cache.insert(
                ("alpha".to_string(), "alice".to_string()),
                SubjectScopedConnection {
                    _connection: stale_conn,
                    peer: stale_peer,
                    tools: vec![],
                    last_used: Instant::now()
                        .checked_sub(SUBJECT_CONN_IDLE_TTL + Duration::from_mins(1))
                        .expect("instant in range"),
                },
            );
            // Fresh: just used.
            cache.insert(
                ("beta".to_string(), "bob".to_string()),
                SubjectScopedConnection {
                    _connection: fresh_conn,
                    peer: fresh_peer,
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
        }

        assert_eq!(pool.subject_connections.read().await.len(), 2);

        let (evicted, _) = pool.sweep_subject_connections().await;
        assert_eq!(evicted, 1, "exactly the stale entry should be evicted");

        let remaining: Vec<_> = pool
            .subject_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, "beta");
    }

    /// P-H2: the sweep prunes orphan `subject_connect_locks` — lock entries with
    /// no live connection and no in-flight holder (`Arc::strong_count == 1`) —
    /// but preserves locks that still gate a live cached connection.
    #[tokio::test]
    async fn sweep_prunes_orphan_subject_connect_locks() {
        use std::time::Instant;

        let pool = static_catalog_pool("alpha").await;
        let conn = pool
            .connections
            .write()
            .await
            .remove("alpha")
            .expect("alpha connection");
        let peer = conn.peer.clone();

        // A live cached connection for (alpha, alice) plus its gating lock.
        pool.subject_connections.write().await.insert(
            ("alpha".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                _connection: conn,
                peer,
                tools: vec![],
                last_used: Instant::now(),
            },
        );
        {
            let mut locks = pool.subject_connect_locks.write().await;
            // Lock that still gates a live connection — must be kept.
            locks.insert(
                ("alpha".to_string(), "alice".to_string()),
                Arc::new(tokio::sync::Mutex::new(())),
            );
            // Orphan lock: no matching connection, no other holder — must be pruned.
            locks.insert(
                ("alpha".to_string(), "ghost".to_string()),
                Arc::new(tokio::sync::Mutex::new(())),
            );
        }

        let (_, pruned) = pool.sweep_subject_connections().await;
        assert_eq!(pruned, 1, "only the orphan lock should be pruned");

        let remaining: Vec<_> = pool
            .subject_connect_locks
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].1, "alice");
    }

    /// P-H2: `evict_lru_over_cap` selects least-recently-used entries for
    /// eviction, never touches the protected (about-to-be-inserted) key, and
    /// drives the map down to the cap. Exercising the selection logic directly
    /// keeps the test light (no need to open 256+ live peers to hit the real
    /// `SUBJECT_CONN_MAX_ENTRIES` bound).
    #[tokio::test]
    async fn evict_lru_over_cap_drops_least_recently_used_and_spares_protected() {
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        use super::evict_lru_over_cap;

        // Mint three live connections from three pools.
        let mut conns = Vec::new();
        for name in ["a", "b", "c"] {
            let p = static_catalog_pool(name).await;
            let c = p
                .connections
                .write()
                .await
                .remove(name)
                .expect("connection present");
            conns.push((name, c));
        }

        let now = Instant::now();
        let mut cache: HashMap<(String, String), SubjectScopedConnection> = HashMap::new();
        // last_used ages: a = oldest, b = middle, c = newest.
        for (offset_secs, (name, conn)) in conns.into_iter().enumerate() {
            let peer = conn.peer.clone();
            cache.insert(
                (name.to_string(), "subj".to_string()),
                SubjectScopedConnection {
                    _connection: conn,
                    peer,
                    tools: vec![],
                    last_used: now
                        .checked_sub(Duration::from_secs(300 - offset_secs as u64 * 100))
                        .expect("instant in range"),
                },
            );
        }

        // Cap the map at 2, protecting "c" (the newest / pretend just-inserted).
        let protect = ("c".to_string(), "subj".to_string());
        let evicted = evict_lru_over_cap(&mut cache, 2, &protect);

        // Exactly one eviction: the least-recently-used "a".
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].0, "a");
        assert_eq!(cache.len(), 2);
        assert!(cache.contains_key(&("b".to_string(), "subj".to_string())));
        assert!(cache.contains_key(&protect));

        // Clean up evicted peers so their server tasks stop.
        for (name, conn) in evicted {
            conn.shutdown(&name, "test.cleanup").await;
        }
    }
}
