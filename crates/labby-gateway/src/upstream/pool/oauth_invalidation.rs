//! Coordinated invalidation of live connections authenticated with upstream OAuth credentials.

use std::collections::HashSet;
use std::sync::Arc;

use super::UpstreamPool;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OAuthSessionInvalidation {
    pub generic_connections: usize,
    pub subject_connections: usize,
    pub relay_connections: usize,
    pub task_routes: usize,
}

impl OAuthSessionInvalidation {
    pub fn total(self) -> usize {
        self.generic_connections
            + self.subject_connections
            + self.relay_connections
            + self.task_routes
    }
}

impl UpstreamPool {
    pub(crate) async fn drain_oauth_client_capacity_evictions(&self) {
        let Some(cache) = &self.oauth_client_cache else {
            return;
        };
        if !cache.has_pending_invalidations() {
            return;
        }
        // The lifecycle writer owns a complete invalidation even if the RPC
        // waiting for this boundary is cancelled while a registry lock is busy.
        let pool = self.clone();
        let cache = cache.clone();
        let drain = tokio::spawn(async move {
            let barrier = Arc::clone(&pool.oauth_invalidation_barrier)
                .write_owned()
                .await;
            let victims = cache.take_pending_invalidations();
            for event in victims {
                if !event.is_current() {
                    continue;
                }
                if let Some(subject) = event.subject {
                    pool.invalidate_oauth_subject_sessions_guarded(
                        &event.upstream,
                        &subject,
                        "oauth.client_cache.invalidate",
                    )
                    .await;
                } else {
                    pool.invalidate_oauth_upstream_sessions_guarded(
                        &[event.upstream],
                        "oauth.refresh.rejected",
                    )
                    .await;
                }
            }
            drop(barrier);
        });
        if let Err(error) = drain.await {
            tracing::warn!(error = %error, "OAuth lifecycle invalidation task failed");
        }
    }

    pub async fn invalidate_oauth_subject_sessions(
        &self,
        upstream: &str,
        subject: &str,
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        let _barrier = self.oauth_invalidation_barrier.write().await;
        self.invalidate_oauth_subject_sessions_guarded(upstream, subject, reason)
            .await
    }

    pub(crate) async fn invalidate_oauth_subject_sessions_guarded(
        &self,
        upstream: &str,
        subject: &str,
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        if let Some(cache) = &self.oauth_client_cache {
            cache.advance_subject_epoch(upstream, subject);
            cache.evict_subject(upstream, subject);
        }
        let subject_connection = self
            .subject_connections
            .write()
            .await
            .remove(&(upstream.to_string(), subject.to_string()));
        self.subject_connect_errors
            .write()
            .await
            .remove(&(upstream.to_owned(), subject.to_owned()));
        let generic_connection = {
            let matches = self
                .generic_oauth_subjects
                .read()
                .await
                .get(upstream)
                .is_some_and(|cached| cached == subject);
            if matches {
                self.generic_oauth_subjects.write().await.remove(upstream);
                self.remove_connection_binding(upstream).await
            } else {
                None
            }
        };
        let relay_connections = {
            let mut cache = self.relay_connections.write().await;
            let keys = cache
                .keys()
                .filter(|(name, _, cached_subject, _)| {
                    name == upstream && cached_subject.as_deref() == Some(subject)
                })
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key.0, entry)))
                .collect::<Vec<_>>()
        };
        let task_routes = self
            .invalidate_task_routes_for_oauth_subject(upstream, subject, reason)
            .await;
        let counts = OAuthSessionInvalidation {
            generic_connections: usize::from(generic_connection.is_some()),
            subject_connections: usize::from(subject_connection.is_some()),
            relay_connections: relay_connections.len(),
            task_routes,
        };
        let subject_upstream = upstream.to_string();
        let subject_shutdown = async move {
            if let Some(connection) = subject_connection {
                connection
                    ._connection
                    .shutdown(&subject_upstream, reason)
                    .await;
            }
        };
        let generic_upstream = upstream.to_string();
        let generic_shutdown = async move {
            if let Some(connection) = generic_connection {
                connection.shutdown(&generic_upstream, reason).await;
            }
        };
        let relay_shutdown = futures::future::join_all(relay_connections.into_iter().map(
            |(name, connection)| async move {
                connection._connection.shutdown(&name, reason).await;
            },
        ));
        // Peer shutdown can perform transport I/O. Keep it outside the global
        // credential writer span: all publication-capable cache entries have
        // already been detached above, so shutdown is cleanup, not fencing.
        let shutdown_barrier = Arc::clone(&self.oauth_invalidation_barrier);
        tokio::spawn(async move {
            let after_writer = shutdown_barrier.read().await;
            drop(after_writer);
            tokio::join!(generic_shutdown, subject_shutdown, relay_shutdown);
        });
        counts
    }

    /// Close live peers only for upstreams backed by the shared Google credential.
    pub async fn invalidate_oauth_upstream_sessions(
        &self,
        upstreams: &[String],
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        let _barrier = self.oauth_invalidation_barrier.write().await;
        self.invalidate_oauth_upstream_sessions_guarded(upstreams, reason)
            .await
    }

    pub(crate) async fn invalidate_oauth_upstream_sessions_guarded(
        &self,
        upstreams: &[String],
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        let upstreams = upstreams.iter().map(String::as_str).collect::<HashSet<_>>();
        if let Some(cache) = &self.oauth_client_cache {
            for upstream in &upstreams {
                cache.advance_upstream_epoch(upstream);
                cache.evict_upstream(upstream);
            }
        }
        let subject_connections = {
            let mut cache = self.subject_connections.write().await;
            let keys = cache
                .keys()
                .filter(|(name, _)| upstreams.contains(name.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key.0, entry)))
                .collect::<Vec<_>>()
        };
        let generic_connections = {
            let names = self
                .generic_oauth_subjects
                .read()
                .await
                .keys()
                .filter(|name| upstreams.contains(name.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let mut removed = Vec::new();
            for name in names {
                self.generic_oauth_subjects.write().await.remove(&name);
                if let Some(connection) = self.remove_connection_binding(&name).await {
                    removed.push((name, connection));
                }
            }
            removed
        };
        let relay_connections = {
            let mut cache = self.relay_connections.write().await;
            let keys = cache
                .keys()
                .filter(|(name, _, subject, _)| {
                    subject.is_some() && upstreams.contains(name.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key.0, entry)))
                .collect::<Vec<_>>()
        };
        let task_routes = self
            .invalidate_oauth_task_routes_for_upstreams(&upstreams, reason)
            .await;
        let counts = OAuthSessionInvalidation {
            generic_connections: generic_connections.len(),
            subject_connections: subject_connections.len(),
            relay_connections: relay_connections.len(),
            task_routes,
        };
        let subject_shutdown = futures::future::join_all(subject_connections.into_iter().map(
            |(name, connection)| async move {
                connection._connection.shutdown(&name, reason).await;
            },
        ));
        let generic_shutdown = futures::future::join_all(
            generic_connections
                .into_iter()
                .map(|(name, connection)| async move { connection.shutdown(&name, reason).await }),
        );
        let relay_shutdown = futures::future::join_all(relay_connections.into_iter().map(
            |(name, connection)| async move {
                connection._connection.shutdown(&name, reason).await;
            },
        ));
        // See the subject-scoped path above: detach under the lifecycle writer,
        // then let transport shutdown proceed after the caller releases it.
        let shutdown_barrier = Arc::clone(&self.oauth_invalidation_barrier);
        tokio::spawn(async move {
            let after_writer = shutdown_barrier.read().await;
            drop(after_writer);
            tokio::join!(generic_shutdown, subject_shutdown, relay_shutdown);
        });
        counts
    }
}

#[cfg(test)]
mod rejection_tests {
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
}
