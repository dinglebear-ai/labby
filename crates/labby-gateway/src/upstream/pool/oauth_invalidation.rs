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
    pub(super) async fn drain_oauth_client_capacity_evictions(&self) {
        let Some(cache) = &self.oauth_client_cache else {
            return;
        };
        let victims = cache.take_capacity_evictions();
        if victims.is_empty() {
            return;
        }
        let barrier = Arc::clone(&self.oauth_invalidation_barrier)
            .write_owned()
            .await;
        cache.advance_lifecycle_epoch();
        for (upstream, subject) in victims {
            self.invalidate_oauth_subject_sessions_guarded(
                &upstream,
                &subject,
                "oauth.client_cache.capacity",
            )
            .await;
        }
        drop(barrier);
    }

    pub async fn invalidate_oauth_subject_sessions(
        &self,
        upstream: &str,
        subject: &str,
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        let _barrier = self.oauth_invalidation_barrier.write().await;
        if let Some(cache) = &self.oauth_client_cache {
            cache.advance_lifecycle_epoch();
        }
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
            cache.evict_subject(upstream, subject);
        }
        let subject_connection = self
            .subject_connections
            .write()
            .await
            .remove(&(upstream.to_string(), subject.to_string()));
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
        if let Some(cache) = &self.oauth_client_cache {
            cache.advance_lifecycle_epoch();
        }
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
