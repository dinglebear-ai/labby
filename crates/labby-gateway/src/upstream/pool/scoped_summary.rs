//! Cache-only, identity-scoped operator projections. No peer acquisition or discovery.
use std::sync::Arc;
use std::time::Instant;

use labby_runtime::gateway_config::UpstreamConfig;
use rmcp::{RoleClient, model::Resource, service::Peer};

use super::entries::{
    prompt_exposed, resolve_request_exposure_policy, resolve_request_prompt_exposure_policy,
    resolve_request_resource_exposure_policy, resource_exposed,
};
use super::helpers::SUBJECT_CONN_IDLE_TTL;
use super::{UpstreamPool, helpers::UpstreamCachedSummary};

#[derive(Default)]
pub(crate) struct SubjectOptionalCatalogs {
    /// Full resource rows listed over this subject connection. Shared, not
    /// cloned, on every cache hit.
    pub resources: Option<Arc<[Resource]>>,
    /// When `resources` was listed; `RESOURCE_SNAPSHOT_MAX_AGE` bounds reuse.
    pub resources_listed_at: Option<Instant>,
    pub prompts: Option<Vec<String>>,
}

#[derive(Default)]
pub(crate) struct SubjectSummary {
    pub summary: UpstreamCachedSummary,
    pub connected: bool,
    pub tools_known: bool,
    pub resources_known: bool,
    pub prompts_known: bool,
    /// Sanitized reason this subject's last connect attempt failed, present
    /// only while no live connection has replaced that attempt.
    pub last_error: Option<String>,
}

impl UpstreamPool {
    pub(crate) async fn cached_subject_summary(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> SubjectSummary {
        let Some(subject) = subject else {
            return SubjectSummary::default();
        };
        let key = (config.name.clone(), subject.to_owned());
        let last_error = self
            .subject_connect_errors
            .read()
            .await
            .get(&key)
            .filter(|entry| entry.recorded_at.elapsed() < SUBJECT_CONN_IDLE_TTL)
            .map(|entry| entry.message.clone());
        let cache = self.subject_connections.read().await;
        let Some(entry) = cache.get(&key) else {
            return SubjectSummary {
                last_error,
                ..SubjectSummary::default()
            };
        };
        let connected = config.enabled
            && !entry.peer.is_transport_closed()
            && entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL;
        let tool_policy =
            resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
        let resource_policy =
            resolve_request_resource_exposure_policy(&config.name, config.expose_resources.clone());
        let prompt_policy =
            resolve_request_prompt_exposure_policy(&config.name, config.expose_prompts.clone());
        SubjectSummary {
            connected,
            last_error: None,
            tools_known: true,
            resources_known: !config.proxy_resources || entry.optional_catalogs.resources.is_some(),
            prompts_known: !config.proxy_prompts || entry.optional_catalogs.prompts.is_some(),
            summary: UpstreamCachedSummary {
                discovered_tool_count: entry.tools.len(),
                exposed_tool_count: if connected {
                    entry
                        .tools
                        .iter()
                        .filter(|tool| tool_policy.matches(tool.name.as_ref()))
                        .count()
                } else {
                    0
                },
                discovered_resource_count: entry
                    .optional_catalogs
                    .resources
                    .as_ref()
                    .map_or(0, |items| items.len()),
                exposed_resource_count: if connected && config.proxy_resources {
                    entry
                        .optional_catalogs
                        .resources
                        .as_ref()
                        .map_or(0, |items| {
                            items
                                .iter()
                                .filter(|resource| {
                                    resource_exposed(&resource_policy, &resource.uri)
                                })
                                .count()
                        })
                } else {
                    0
                },
                discovered_prompt_count: entry
                    .optional_catalogs
                    .prompts
                    .as_ref()
                    .map_or(0, Vec::len),
                exposed_prompt_count: if connected && config.proxy_prompts {
                    entry.optional_catalogs.prompts.as_ref().map_or(0, |items| {
                        items
                            .iter()
                            .filter(|name| prompt_exposed(&prompt_policy, &config.name, name))
                            .count()
                    })
                } else {
                    0
                },
                ..Default::default()
            },
        }
    }

    pub(super) async fn record_subject_optional_catalog(
        &self,
        name: &str,
        subject: &str,
        peer: &Peer<RoleClient>,
        resources: Option<Vec<Resource>>,
        prompts: Option<Vec<String>>,
    ) {
        // Handshake snapshots are unique to a peer. Reject an awaited reply from
        // a replaced session without writing into its successor's cache.
        let Some(observed) = peer.peer_info() else {
            return;
        };
        let mut cache = self.subject_connections.write().await;
        let Some(entry) = cache.get_mut(&(name.to_owned(), subject.to_owned())) else {
            return;
        };
        if entry.peer.is_transport_closed()
            || !entry
                .peer
                .peer_info()
                .is_some_and(|current| Arc::ptr_eq(&current, &observed))
        {
            return;
        }
        if let Some(resources) = resources {
            entry.optional_catalogs.resources = Some(resources.into());
            entry.optional_catalogs.resources_listed_at = Some(Instant::now());
        }
        if let Some(prompts) = prompts {
            entry.optional_catalogs.prompts = Some(prompts);
        }
    }

    /// Drop every subject's cached resource catalog for `upstream` so the next
    /// subject-scoped listing re-fetches it. Called when the upstream announces
    /// `resources/list_changed`; the subject connections themselves stay warm.
    pub(super) async fn invalidate_subject_resource_catalogs(&self, upstream: &str) -> usize {
        let mut cache = self.subject_connections.write().await;
        let mut cleared = 0usize;
        for ((name, _), entry) in cache.iter_mut() {
            if name == upstream && entry.optional_catalogs.resources.take().is_some() {
                entry.optional_catalogs.resources_listed_at = None;
                cleared += 1;
            }
        }
        cleared
    }

    /// Age a subject's cached resource catalog so freshness tests do not have
    /// to wait out `RESOURCE_SNAPSHOT_MAX_AGE`.
    #[cfg(test)]
    pub(super) async fn age_subject_resource_catalog_for_tests(
        &self,
        upstream: &str,
        subject: &str,
        age: std::time::Duration,
    ) {
        let mut cache = self.subject_connections.write().await;
        if let Some(entry) = cache.get_mut(&(upstream.to_owned(), subject.to_owned())) {
            entry.optional_catalogs.resources_listed_at = Some(
                Instant::now()
                    .checked_sub(age)
                    .expect("test age fits the monotonic clock"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::pool::testsupport::*;

    #[tokio::test]
    async fn summaries_keep_subject_catalogs_isolated_and_empty_known() {
        let pool = static_catalog_pool("alpha").await;
        move_connection_to_subject_cache_with_tools(
            &pool,
            "alpha",
            "alice",
            vec![test_tool("search")],
        )
        .await;
        let other = static_catalog_pool("alpha").await;
        let connection = other.connections.write().await.remove("alpha").unwrap();
        pool.connections
            .write()
            .await
            .insert("alpha".into(), connection);
        move_connection_to_subject_cache_with_tools(&pool, "alpha", "bob", vec![]).await;
        let config = UpstreamConfig {
            name: "alpha".into(),
            proxy_resources: true,
            proxy_prompts: true,
            ..test_upstream_config()
        };
        let alice = pool.cached_subject_summary(&config, Some("alice")).await;
        let bob = pool.cached_subject_summary(&config, Some("bob")).await;
        assert!(alice.connected && bob.connected);
        assert_eq!(alice.summary.discovered_tool_count, 1);
        assert_eq!(bob.summary.discovered_tool_count, 0);
        assert!(bob.tools_known);
        assert!(!bob.resources_known);
        let bob_peer = pool
            .subject_connections
            .read()
            .await
            .get(&("alpha".into(), "bob".into()))
            .unwrap()
            .peer
            .clone();
        pool.record_subject_optional_catalog("alpha", "bob", &bob_peer, Some(vec![]), Some(vec![]))
            .await;
        let bob = pool.cached_subject_summary(&config, Some("bob")).await;
        assert!(bob.resources_known && bob.prompts_known);
        assert!(
            !pool
                .cached_subject_summary(&config, Some("alice"))
                .await
                .resources_known
        );
        let missing = pool.cached_subject_summary(&config, Some("unknown")).await;
        assert!(!missing.connected && !missing.tools_known);
        assert_eq!(missing.summary.discovered_tool_count, 0);
        let absent = pool.cached_subject_summary(&config, None).await;
        assert!(!absent.connected && !absent.tools_known);
    }
}
