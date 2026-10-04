//! Fence cancelled operator probes against newer successful tool observations.
use super::{SubjectConnectErrorEntry, UpstreamPool, helpers::SUBJECT_CONN_MAX_ENTRIES};
use labby_auth::upstream::cache::OAuthLifecycleEpoch;
use labby_runtime::gateway_config::UpstreamConfig;
use rmcp::{RoleClient, service::Peer};
use std::{collections::HashMap, sync::Arc, time::Instant};

pub(crate) struct SubjectToolObservationFence {
    peer: Option<Peer<RoleClient>>,
    revision: u64,
}

impl UpstreamPool {
    pub(crate) async fn subject_tool_observation_fence(
        &self,
        name: &str,
        subject: &str,
    ) -> SubjectToolObservationFence {
        let cache = self.subject_connections.read().await;
        let entry = cache.get(&(name.to_owned(), subject.to_owned()));
        SubjectToolObservationFence {
            peer: entry.map(|entry| entry.peer.clone()),
            revision: entry.map_or(0, |entry| entry.optional_catalogs.tools_revision),
        }
    }

    /// Record an outer timeout only if the config, credentials, peer and last
    /// successful observation still describe the probe that was cancelled.
    pub(crate) async fn record_subject_tool_probe_failure(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        fence: &SubjectToolObservationFence,
        epoch: Option<&OAuthLifecycleEpoch>,
        error: &str,
    ) -> bool {
        let _publication = self.oauth_invalidation_barrier.read().await;
        if epoch.is_some_and(|epoch| !epoch.is_current()) || !self.upstream_config_matches(config) {
            return false;
        }
        let key = (config.name.clone(), subject.to_owned());
        let cache = self.subject_connections.read().await;
        let current = cache.get(&key);
        let same_observation = match (current, fence.peer.as_ref()) {
            (None, None) => true,
            (Some(entry), Some(peer)) => {
                entry.optional_catalogs.tools_revision == fence.revision
                    && match (entry.peer.peer_info(), peer.peer_info()) {
                        (Some(current), Some(observed)) => Arc::ptr_eq(&current, &observed),
                        _ => false,
                    }
            }
            _ => false,
        };
        if !same_observation {
            return false;
        }
        // Keep the subject read lock through the error write. Successful refresh
        // publishes under the subject write lock, then clears this same error.
        let mut errors = self.subject_connect_errors.write().await;
        errors.insert(
            key,
            SubjectConnectErrorEntry {
                message: labby_runtime::redact::sanitize_error_text(error, 512),
                recorded_at: Instant::now(),
            },
        );
        prune_subject_connect_errors(&cache, &mut errors);
        true
    }
}

/// Live diagnostics are bounded by the subject cache. Use the remaining
/// capacity for orphan failures, pruning those first so memory pressure never
/// changes a retained failed measurement into a successful one.
pub(super) fn prune_subject_connect_errors(
    cache: &HashMap<(String, String), super::SubjectScopedConnection>,
    errors: &mut HashMap<(String, String), SubjectConnectErrorEntry>,
) {
    while errors.len() > SUBJECT_CONN_MAX_ENTRIES {
        let oldest = errors
            .iter()
            .filter(|(key, _)| !cache.contains_key(*key))
            .min_by_key(|(_, entry)| entry.recorded_at)
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else {
            break;
        };
        errors.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::*;
    use super::*;
    use labby_runtime::gateway_config::{
        UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
    };

    fn config() -> UpstreamConfig {
        UpstreamConfig {
            name: "alpha".into(),
            oauth: Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration: UpstreamOauthRegistration::Dynamic,
                scopes: None,
                additional_endpoint_origins: vec![],
                credential: Default::default(),
                prefer_client_metadata_document: None,
            }),
            ..test_upstream_config()
        }
    }

    #[tokio::test]
    async fn cancelled_probe_cannot_overwrite_newer_success_on_same_peer() {
        let pool = catalog_pool_with_server("alpha", SlowResponseServer).await;
        let config = config();
        pool.register_upstream_config_for_tests(&config);
        move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", vec![]).await;
        let fence = pool.subject_tool_observation_fence("alpha", "alice").await;
        pool.reprobe_tools_for_upstream_as(&config, Some("alice"), None)
            .await
            .unwrap();
        assert!(
            !pool
                .record_subject_tool_probe_failure(&config, "alice", &fence, None, "late timeout")
                .await
        );
        let current = pool.cached_subject_summary(&config, Some("alice")).await;
        assert!(current.last_error.is_none());
        assert_eq!(
            current.observation().tools.state,
            crate::gateway::view_models::CapabilityObservationState::Known
        );
    }

    #[tokio::test]
    async fn cancelled_probe_cannot_write_into_replacement_peer() {
        let pool = static_catalog_pool("alpha").await;
        let config = config();
        pool.register_upstream_config_for_tests(&config);
        move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", vec![]).await;
        let fence = pool.subject_tool_observation_fence("alpha", "alice").await;
        let replacement = static_catalog_pool("alpha").await;
        let connection = replacement
            .connections
            .write()
            .await
            .remove("alpha")
            .unwrap();
        pool.connections
            .write()
            .await
            .insert("alpha".into(), connection);
        move_connection_to_subject_cache_with_tools(
            &pool,
            "alpha",
            "alice",
            vec![test_tool("new-peer")],
        )
        .await;
        assert!(
            !pool
                .record_subject_tool_probe_failure(&config, "alice", &fence, None, "late timeout")
                .await
        );
        let current = pool.cached_subject_summary(&config, Some("alice")).await;
        assert!(current.last_error.is_none());
        assert_eq!(current.summary.discovered_tool_count, 1);
    }

    #[tokio::test]
    async fn cancelled_probe_cannot_publish_after_config_or_epoch_replacement() {
        let cache =
            labby_auth::upstream::cache::OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
        let fixture = static_catalog_pool("alpha").await;
        let pool = fixture
            .as_ref()
            .clone()
            .with_oauth_client_cache(cache.clone());
        let config = config();
        pool.register_upstream_config_for_tests(&config);
        move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", vec![]).await;
        let fence = pool.subject_tool_observation_fence("alpha", "alice").await;
        let epoch = cache.lifecycle_epoch_for("alpha", "alice");
        let mut replacement = config.clone();
        replacement.url = Some("https://replacement.invalid/mcp".into());
        pool.register_upstream_config_for_tests(&replacement);
        assert!(
            !pool
                .record_subject_tool_probe_failure(
                    &config,
                    "alice",
                    &fence,
                    Some(&epoch),
                    "late timeout"
                )
                .await
        );
        pool.register_upstream_config_for_tests(&config);
        {
            let _writer = cache.invalidation_barrier().write_owned().await;
            cache.advance_subject_epoch("alpha", "alice");
        }
        assert!(
            !pool
                .record_subject_tool_probe_failure(
                    &config,
                    "alice",
                    &fence,
                    Some(&epoch),
                    "late timeout"
                )
                .await
        );
        assert!(pool.subject_connect_errors.read().await.is_empty());
    }

    #[tokio::test]
    async fn credential_invalidation_clears_cancelled_probe_failure() {
        let pool = static_catalog_pool("alpha").await;
        let config = config();
        pool.register_upstream_config_for_tests(&config);
        move_connection_to_subject_cache_with_tools(&pool, "alpha", "alice", vec![]).await;
        let fence = pool.subject_tool_observation_fence("alpha", "alice").await;
        assert!(
            pool.record_subject_tool_probe_failure(&config, "alice", &fence, None, "timeout")
                .await
        );
        pool.invalidate_oauth_subject_sessions("alpha", "alice", "test.credentials_changed")
            .await;
        assert!(
            pool.cached_subject_summary(&config, Some("alice"))
                .await
                .last_error
                .is_none()
        );
    }
    #[tokio::test]
    async fn failed_observation_survives_error_ttl_while_catalog_is_retained() {
        let pool = static_catalog_pool("alpha").await;
        let config = config();
        pool.register_upstream_config_for_tests(&config);
        move_connection_to_subject_cache_with_tools(
            &pool,
            "alpha",
            "alice",
            vec![test_tool("retained")],
        )
        .await;
        let fence = pool.subject_tool_observation_fence("alpha", "alice").await;
        assert!(
            pool.record_subject_tool_probe_failure(&config, "alice", &fence, None, "timeout")
                .await
        );
        pool.subject_connect_errors
            .write()
            .await
            .get_mut(&("alpha".into(), "alice".into()))
            .unwrap()
            .recorded_at = Instant::now()
            .checked_sub(
                super::super::helpers::SUBJECT_CONN_IDLE_TTL + std::time::Duration::from_secs(1),
            )
            .unwrap();
        let observed = pool
            .cached_subject_summary(&config, Some("alice"))
            .await
            .observation();
        assert_eq!(
            observed.tools.state,
            crate::gateway::view_models::CapabilityObservationState::Failed
        );
        assert_eq!(observed.tools.discovered, Some(1));
        pool.sweep_subject_connections().await;
        assert_eq!(
            pool.cached_subject_summary(&config, Some("alice"))
                .await
                .observation()
                .tools
                .state,
            crate::gateway::view_models::CapabilityObservationState::Failed
        );
    }
    #[tokio::test]
    async fn cold_failure_pressure_cannot_clear_a_retained_catalog_failure() {
        let pool = static_catalog_pool("alpha").await;
        let config = config();
        pool.register_upstream_config_for_tests(&config);
        move_connection_to_subject_cache_with_tools(
            &pool,
            "alpha",
            "alice",
            vec![test_tool("retained")],
        )
        .await;
        let fence = pool.subject_tool_observation_fence("alpha", "alice").await;
        assert!(
            pool.record_subject_tool_probe_failure(&config, "alice", &fence, None, "warm timeout")
                .await
        );
        for index in 0..=SUBJECT_CONN_MAX_ENTRIES {
            let subject = format!("cold-{index}");
            let fence = pool.subject_tool_observation_fence("alpha", &subject).await;
            assert!(
                pool.record_subject_tool_probe_failure(
                    &config,
                    &subject,
                    &fence,
                    None,
                    "cold timeout"
                )
                .await
            );
        }
        let observed = pool
            .cached_subject_summary(&config, Some("alice"))
            .await
            .observation();
        assert_eq!(
            observed.tools.state,
            crate::gateway::view_models::CapabilityObservationState::Failed
        );
        assert_eq!(observed.tools.discovered, Some(1));
        // Ordinary connection failures share the same budget as cancelled probes.
        assert!(
            pool.acquire_or_connect_subject(&config, "cold-connect")
                .await
                .is_err()
        );
        assert_eq!(
            pool.cached_subject_summary(&config, Some("alice"))
                .await
                .observation()
                .tools
                .state,
            crate::gateway::view_models::CapabilityObservationState::Failed
        );
        pool.sweep_subject_connections().await;
        assert_eq!(
            pool.cached_subject_summary(&config, Some("alice"))
                .await
                .observation()
                .tools
                .state,
            crate::gateway::view_models::CapabilityObservationState::Failed
        );
        assert!(pool.subject_connect_errors.read().await.len() <= SUBJECT_CONN_MAX_ENTRIES);
    }
}
