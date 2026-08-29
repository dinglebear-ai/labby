//! Opaque connection/catalog-entry identity for asynchronous capability work.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use std::collections::BTreeSet;
use thiserror::Error;

use super::super::types::UpstreamEntry;
use super::{UpstreamConnection, UpstreamPool};

static NEXT_CONNECTION_INCARNATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ConnectionIncarnation(NonZeroU64);

impl ConnectionIncarnation {
    #[cfg(any(test, feature = "testkit"))]
    pub(super) fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("upstream connection identity is unavailable")]
pub(super) struct ConnectionIncarnationExhausted;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub(super) enum ConnectionCatalogBindingError {
    #[error(transparent)]
    Exhausted(#[from] ConnectionIncarnationExhausted),
    #[error("upstream catalog entry is unavailable")]
    MissingEntry,
}

pub(super) fn next_connection_incarnation()
-> Result<ConnectionIncarnation, ConnectionIncarnationExhausted> {
    NEXT_CONNECTION_INCARNATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .ok()
        .and_then(NonZeroU64::new)
        .map(ConnectionIncarnation)
        .ok_or(ConnectionIncarnationExhausted)
}

pub(super) struct ObservedConnectionCatalogEntry {
    upstream: String,
    pub(super) peer: rmcp::service::Peer<rmcp::RoleClient>,
    incarnation: ConnectionIncarnation,
}

impl ObservedConnectionCatalogEntry {
    pub(super) fn upstream(&self) -> &str {
        &self.upstream
    }

    pub(super) fn incarnation(&self) -> ConnectionIncarnation {
        self.incarnation
    }
}

impl UpstreamPool {
    pub(super) async fn observe_tool_call(
        &self,
        upstream: &str,
        native_name: &str,
        generation: super::ToolCatalogGeneration,
    ) -> Option<ObservedConnectionCatalogEntry> {
        let _binding = self.connection_catalog_binding.read().await;
        let connections = self.connections.read().await;
        let catalog = self.catalog.read().await;
        if !catalog.contains_tool_route(generation, upstream, native_name) {
            return None;
        }
        let connection = connections.get(upstream)?;
        let incarnation = connection.incarnation?;
        (catalog.incarnation(upstream) == Some(incarnation)).then(|| {
            ObservedConnectionCatalogEntry {
                upstream: upstream.to_string(),
                peer: connection.peer.clone(),
                incarnation,
            }
        })
    }

    pub(super) async fn observed_tool_call_is_current(
        &self,
        observed: &ObservedConnectionCatalogEntry,
        generation: super::ToolCatalogGeneration,
        native_name: &str,
    ) -> bool {
        let _binding = self.connection_catalog_binding.read().await;
        let connections = self.connections.read().await;
        let Some(connection) = connections.get(observed.upstream()) else {
            return false;
        };
        if connection.incarnation != Some(observed.incarnation()) {
            return false;
        }
        drop(connections);
        let catalog = self.catalog.read().await;
        catalog.incarnation(observed.upstream()) == Some(observed.incarnation())
            && catalog.contains_tool_route(generation, observed.upstream(), native_name)
    }

    pub(super) async fn observe_prompt_call(
        &self,
        upstream: &str,
        native_name: &str,
        generation: super::PromptCatalogGeneration,
    ) -> Option<ObservedConnectionCatalogEntry> {
        let _binding = self.connection_catalog_binding.read().await;
        let connections = self.connections.read().await;
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream)?;
        if !entry.prompt_health.is_routable()
            || !catalog.contains_prompt_route(generation, upstream, native_name)
        {
            return None;
        }
        let connection = connections.get(upstream)?;
        let incarnation = connection.incarnation?;
        (catalog.incarnation(upstream) == Some(incarnation)).then(|| {
            ObservedConnectionCatalogEntry {
                upstream: upstream.to_string(),
                peer: connection.peer.clone(),
                incarnation,
            }
        })
    }
    pub(super) async fn observe_routable_prompt_connections(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<ObservedConnectionCatalogEntry> {
        let _binding = self.connection_catalog_binding.read().await;
        let connections = self.connections.read().await;
        let catalog = self.catalog.read().await;
        let mut observed = catalog
            .iter()
            .filter(|(name, entry)| {
                allowed.is_none_or(|allowed| allowed.contains(*name))
                    && entry
                        .health_for(super::super::types::UpstreamCapability::Prompts)
                        .is_routable()
            })
            .filter_map(|(upstream, _)| {
                let connection = connections.get(upstream)?;
                let incarnation = connection.incarnation?;
                (catalog.incarnation(upstream) == Some(incarnation)).then(|| {
                    ObservedConnectionCatalogEntry {
                        upstream: upstream.clone(),
                        peer: connection.peer.clone(),
                        incarnation,
                    }
                })
            })
            .collect::<Vec<_>>();
        observed.sort_by(|left, right| left.upstream.cmp(&right.upstream));
        observed
    }

    pub(super) async fn observe_routable_resource_connections(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<ObservedConnectionCatalogEntry> {
        let _binding = self.connection_catalog_binding.read().await;
        let connections = self.connections.read().await;
        let catalog = self.catalog.read().await;
        // Match the existing catalog -> resource-routing lock order used by
        // lazy seeding while the binding mutex pins connection incarnations.
        let resource_upstreams = self.resource_upstreams.read().await;
        let mut observed = catalog
            .iter()
            .filter(|(name, entry)| {
                resource_upstreams.contains(name)
                    && allowed.is_none_or(|allowed| allowed.contains(*name))
                    && entry
                        .health_for(super::super::types::UpstreamCapability::Resources)
                        .is_routable()
            })
            .filter_map(|(upstream, _)| {
                let connection = connections.get(upstream)?;
                let incarnation = connection.incarnation?;
                (catalog.incarnation(upstream) == Some(incarnation)).then(|| {
                    ObservedConnectionCatalogEntry {
                        upstream: upstream.clone(),
                        peer: connection.peer.clone(),
                        incarnation,
                    }
                })
            })
            .collect::<Vec<_>>();
        observed.sort_by(|left, right| left.upstream.cmp(&right.upstream));
        observed
    }

    pub(super) async fn observed_entry_is_current(
        &self,
        observed: &ObservedConnectionCatalogEntry,
    ) -> bool {
        let _binding = self.connection_catalog_binding.read().await;
        let connections = self.connections.read().await;
        let Some(connection) = connections.get(&observed.upstream) else {
            return false;
        };
        if connection.incarnation != Some(observed.incarnation) {
            return false;
        }
        drop(connections);
        let catalog = self.catalog.read().await;
        catalog.incarnation(&observed.upstream) == Some(observed.incarnation)
            && catalog.contains_key(&observed.upstream)
    }

    pub(super) async fn remove_connection_catalog_entries<'a>(
        &self,
        upstreams: impl IntoIterator<Item = &'a String>,
    ) -> (Vec<(String, UpstreamConnection)>, usize) {
        let _binding = self.connection_catalog_binding.write().await;
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let mut drained = Vec::new();
        let mut removed_entries = 0usize;
        for upstream in upstreams {
            if let Some(connection) = connections.remove(upstream) {
                drained.push((upstream.clone(), connection));
            }
            removed_entries += usize::from(catalog.remove(upstream).is_some());
            catalog.remove_incarnation(upstream);
        }
        (drained, removed_entries)
    }

    pub(super) async fn drain_connection_catalog_bindings(
        &self,
    ) -> (Vec<(String, UpstreamConnection)>, usize) {
        let _binding = self.connection_catalog_binding.write().await;
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let drained = connections.drain().collect::<Vec<_>>();
        let catalog_count = catalog.len();
        catalog.clear();
        catalog.clear_incarnations();
        (drained, catalog_count)
    }

    /// Lock order for every structural binding mutation is the stable pool-wide
    /// binding write lock, then generic connections, then the catalog. All guards
    /// are acquired before mutation, so cancellation cannot publish half a pair.
    pub(super) async fn install_connection_and_apply_entry(
        &self,
        upstream: String,
        mut connection: UpstreamConnection,
        apply: impl FnOnce(&mut UpstreamEntry),
    ) -> Result<Option<UpstreamConnection>, ConnectionCatalogBindingError> {
        let _binding = self.connection_catalog_binding.write().await;
        let incarnation = next_connection_incarnation()?;
        connection.incarnation = Some(incarnation);
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let entry = catalog
            .get_mut(&upstream)
            .ok_or(ConnectionCatalogBindingError::MissingEntry)?;
        apply(entry);
        let previous = connections.insert(upstream.clone(), connection);
        catalog.bind_incarnation(&upstream, incarnation);
        Ok(previous)
    }

    pub(super) async fn remove_connection_binding(
        &self,
        upstream: &str,
    ) -> Option<UpstreamConnection> {
        let _binding = self.connection_catalog_binding.write().await;
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let connection = connections.remove(upstream);
        catalog.remove_incarnation(upstream);
        connection
    }

    pub(super) async fn install_catalog_entry_without_connection(
        &self,
        upstream: String,
        entry: UpstreamEntry,
    ) -> Option<UpstreamConnection> {
        let _binding = self.connection_catalog_binding.write().await;
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let connection = connections.remove(&upstream);
        catalog.insert(upstream.clone(), entry);
        catalog.remove_incarnation(&upstream);
        connection
    }

    pub(super) async fn replace_catalog_entry_without_connection(
        &self,
        upstream: String,
        replace: impl FnOnce(Option<UpstreamEntry>) -> UpstreamEntry,
    ) -> Option<UpstreamConnection> {
        let _binding = self.connection_catalog_binding.write().await;
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let connection = connections.remove(&upstream);
        let entry = replace(catalog.remove(&upstream));
        catalog.insert(upstream.clone(), entry);
        catalog.remove_incarnation(&upstream);
        connection
    }

    pub(super) async fn install_connection_catalog_entry(
        &self,
        upstream: String,
        mut connection: UpstreamConnection,
        entry: UpstreamEntry,
    ) -> Result<Option<UpstreamConnection>, ConnectionIncarnationExhausted> {
        let _binding = self.connection_catalog_binding.write().await;
        let incarnation = next_connection_incarnation()?;
        connection.incarnation = Some(incarnation);
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let previous = connections.insert(upstream.clone(), connection);
        catalog.insert(upstream.clone(), entry);
        catalog.bind_incarnation(&upstream, incarnation);
        Ok(previous)
    }

    pub(super) async fn remove_connection_catalog_entry(
        &self,
        upstream: &str,
    ) -> (Option<UpstreamConnection>, Option<UpstreamEntry>) {
        let _binding = self.connection_catalog_binding.write().await;
        let mut connections = self.connections.write().await;
        let mut catalog = self.catalog_write().await;
        let connection = connections.remove(upstream);
        let entry = catalog.remove(upstream);
        catalog.remove_incarnation(upstream);
        (connection, entry)
    }

    pub(super) async fn observe_connection_catalog_entry(
        &self,
        upstream: &str,
    ) -> Option<ObservedConnectionCatalogEntry> {
        let _binding = self.connection_catalog_binding.read().await;
        let (peer, incarnation) = {
            let connections = self.connections.read().await;
            let connection = connections.get(upstream)?;
            (connection.peer.clone(), connection.incarnation?)
        };
        let catalog = self.catalog.read().await;
        (catalog.contains_key(upstream) && catalog.incarnation(upstream) == Some(incarnation)).then(
            || ObservedConnectionCatalogEntry {
                upstream: upstream.to_string(),
                peer,
                incarnation,
            },
        )
    }

    pub(super) async fn apply_to_observed_entry<R>(
        &self,
        observed: &ObservedConnectionCatalogEntry,
        apply: impl FnOnce(&mut UpstreamEntry) -> R,
    ) -> Option<R> {
        self.apply_to_observed_catalog(observed, |catalog| {
            catalog.get_mut(&observed.upstream).map(apply)
        })
        .await
        .flatten()
    }

    pub(super) async fn apply_to_observed_prompt_call<R>(
        &self,
        observed: &ObservedConnectionCatalogEntry,
        generation: super::PromptCatalogGeneration,
        native_name: &str,
        apply: impl FnOnce(&mut UpstreamEntry) -> R,
    ) -> Option<R> {
        self.apply_to_observed_catalog(observed, |catalog| {
            if !catalog.contains_prompt_route(generation, observed.upstream(), native_name) {
                return None;
            }
            catalog.get_mut(observed.upstream()).map(apply)
        })
        .await
        .flatten()
    }

    pub(super) async fn apply_to_observed_tool_call<R>(
        &self,
        observed: &ObservedConnectionCatalogEntry,
        generation: super::ToolCatalogGeneration,
        native_name: &str,
        apply: impl FnOnce(&mut UpstreamEntry) -> R,
    ) -> Option<R> {
        self.apply_to_observed_catalog(observed, |catalog| {
            if !catalog.contains_tool_route(generation, observed.upstream(), native_name) {
                return None;
            }
            catalog.get_mut(observed.upstream()).map(apply)
        })
        .await
        .flatten()
    }

    pub(super) async fn observe_resource_call(
        &self,
        upstream: &str,
        native_uri: &str,
        generation: super::ResourceCatalogGeneration,
    ) -> Option<ObservedConnectionCatalogEntry> {
        let _binding = self.connection_catalog_binding.read().await;
        let (peer, incarnation) = {
            let connections = self.connections.read().await;
            let connection = connections.get(upstream)?;
            (connection.peer.clone(), connection.incarnation?)
        };
        let catalog = self.catalog.read().await;
        if catalog.incarnation(upstream) != Some(incarnation)
            || !catalog.contains_resource_route(generation, upstream, native_uri)
        {
            return None;
        }
        Some(ObservedConnectionCatalogEntry {
            upstream: upstream.to_string(),
            peer,
            incarnation,
        })
    }

    pub(super) async fn apply_to_observed_resource_call<R>(
        &self,
        observed: &ObservedConnectionCatalogEntry,
        generation: super::ResourceCatalogGeneration,
        native_uri: &str,
        apply: impl FnOnce(&mut UpstreamEntry) -> R,
    ) -> Option<R> {
        let _binding = self.connection_catalog_binding.write().await;
        let connections = self.connections.read().await;
        let connection = connections.get(observed.upstream())?;
        if connection.incarnation != Some(observed.incarnation) {
            return None;
        }
        drop(connections);
        let mut catalog = self.catalog_write().await;
        if catalog.incarnation(observed.upstream()) != Some(observed.incarnation)
            || !catalog.contains_resource_route(generation, observed.upstream(), native_uri)
        {
            return None;
        }
        catalog.get_mut(observed.upstream()).map(apply)
    }

    pub(super) async fn apply_to_observed_catalog<R>(
        &self,
        observed: &ObservedConnectionCatalogEntry,
        apply: impl FnOnce(&mut super::catalog_publication::CatalogState) -> R,
    ) -> Option<R> {
        let _binding = self.connection_catalog_binding.write().await;
        let connections = self.connections.read().await;
        let connection = connections.get(&observed.upstream)?;
        if connection.incarnation != Some(observed.incarnation) {
            return None;
        }
        drop(connections);
        let mut catalog = self.catalog_write().await;
        if catalog.incarnation(&observed.upstream) != Some(observed.incarnation) {
            return None;
        }
        catalog
            .contains_key(&observed.upstream)
            .then(|| apply(&mut catalog))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    use crate::upstream::pool::testsupport::{StaticCatalogServer, catalog_pool_with_server};

    #[test]
    fn checked_allocator_never_returns_zero_or_wraps() {
        let counter = AtomicU64::new(u64::MAX);
        assert!(
            counter
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .is_err()
        );
    }

    #[test]
    fn production_structural_paths_use_binding_coordinator() {
        for (name, source) in [
            ("discover", include_str!("discover.rs")),
            ("ensure", include_str!("ensure.rs")),
            ("lifecycle", include_str!("lifecycle.rs")),
            ("oauth_invalidation", include_str!("oauth_invalidation.rs")),
            ("probe", include_str!("probe.rs")),
            ("registration", include_str!("registration.rs")),
        ] {
            assert!(
                !source.contains("self.connections.write().await.insert")
                    && !source.contains("self.connections.write().await.remove")
                    && !source.contains("let mut connections = self.connections.write().await"),
                "{name} bypasses connection/catalog binding coordinator"
            );
        }
    }

    #[tokio::test]
    async fn cancelled_install_cannot_publish_half_a_binding() {
        let pool = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let original = pool
            .observe_connection_catalog_entry("alpha")
            .await
            .expect("original binding");
        let replacement = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let (connection, entry) = replacement.remove_connection_catalog_entry("alpha").await;

        let catalog_guard = pool.catalog.write().await;
        let installing_pool = Arc::clone(&pool);
        let install = tokio::spawn(async move {
            installing_pool
                .install_connection_catalog_entry(
                    "alpha".to_string(),
                    connection.expect("replacement connection"),
                    entry.expect("replacement entry"),
                )
                .await
        });
        tokio::task::yield_now().await;
        install.abort();
        drop(catalog_guard);
        assert!(install.await.is_err());
        assert_eq!(
            pool.apply_to_observed_entry(&original, |entry| entry.resource_count)
                .await,
            Some(0),
            "cancelled commit must leave the original pair intact"
        );
    }

    #[tokio::test]
    async fn same_connection_object_aba_rejects_stale_observation() {
        let pool = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let stale = pool
            .observe_connection_catalog_entry("alpha")
            .await
            .expect("initial binding");
        let (connection_a, entry_a) = pool.remove_connection_catalog_entry("alpha").await;

        let replacement = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let (connection_b, entry_b) = replacement.remove_connection_catalog_entry("alpha").await;
        pool.install_connection_catalog_entry(
            "alpha".to_string(),
            connection_b.expect("B connection"),
            entry_b.expect("B entry"),
        )
        .await
        .expect("B identity");
        drop(pool.remove_connection_catalog_entry("alpha").await);
        pool.install_connection_catalog_entry(
            "alpha".to_string(),
            connection_a.expect("A connection"),
            entry_a.expect("A entry"),
        )
        .await
        .expect("reinstalled A identity");

        assert!(
            pool.apply_to_observed_entry(&stale, |entry| entry.resource_count = 99)
                .await
                .is_none(),
            "A-B-same-object-A must reject stale A"
        );
        assert_ne!(pool.catalog.read().await["alpha"].resource_count, 99);
    }

    #[tokio::test]
    async fn current_apply_survives_unrelated_catalog_mutation() {
        let pool = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let observed = pool
            .observe_connection_catalog_entry("alpha")
            .await
            .expect("binding");
        let mut unrelated = pool.catalog.read().await["alpha"].clone();
        unrelated.name = "beta".into();
        pool.catalog_write().await.insert("beta".into(), unrelated);

        assert_eq!(
            pool.apply_to_observed_entry(&observed, |entry| {
                entry.resource_count = 7;
                entry.resource_count
            })
            .await,
            Some(7)
        );
    }
}
