//! Cache-only, identity-scoped operator projections. No peer acquisition or discovery.
use std::sync::Arc;
use std::time::Instant;

use labby_runtime::gateway_config::UpstreamConfig;
use rmcp::{RoleClient, model::Resource, service::Peer};

use super::entries::{
    prompt_exposed, resolve_request_exposure_policy, resolve_request_prompt_exposure_policy,
    resolve_request_resource_exposure_policy, resource_exposed,
};
use super::helpers::{RESOURCE_SNAPSHOT_MAX_AGE, SUBJECT_CONN_IDLE_TTL};
use super::{UpstreamPool, helpers::UpstreamCachedSummary};

#[derive(Default)]
pub(crate) struct SubjectOptionalCatalogs {
    /// Full resource rows listed over this subject connection. Shared, not
    /// cloned, on every cache hit.
    pub resources: Option<Arc<[Resource]>>,
    /// Advances only when this peer publishes a successful tools/list.
    pub tools_revision: u64,
    /// When `resources` was listed; `RESOURCE_SNAPSHOT_MAX_AGE` bounds reuse.
    pub resources_listed_at: Option<Instant>,
    pub prompts: Option<Vec<String>>,
    pub resources_error: Option<String>,
    pub prompts_error: Option<String>,
}

#[derive(Default)]
pub(crate) struct SubjectSummary {
    pub summary: UpstreamCachedSummary,
    pub connected: bool,
    pub tools_known: bool,
    pub resources_known: bool,
    pub prompts_known: bool,
    pub resources_stale: bool,
    pub resources_error: Option<String>,
    pub prompts_error: Option<String>,
    pub skills_observation: crate::gateway::view_models::CapabilityFamilyObservation,
    /// Sanitized reason the credential's last acquisition or catalog refresh failed.
    /// Successful acquisition/refresh clears it; failed refresh retains prior counts.
    pub last_error: Option<String>,
}

impl SubjectSummary {
    pub(crate) fn observation(&self) -> crate::gateway::view_models::CapabilityObservation {
        use crate::gateway::view_models::{
            CapabilityFamilyObservation as Family, CapabilityObservation as Observation,
            CapabilityObservationScope as Scope,
        };
        let family = |known: bool, stale, error: Option<&String>, discovered, exposed| {
            Family::from_snapshot(
                known.then_some((discovered, exposed)),
                self.connected,
                stale,
                error.cloned(),
            )
        };
        Observation {
            scope: Scope::Credential,
            tools: family(
                self.tools_known,
                false,
                self.last_error.as_ref(),
                self.summary.discovered_tool_count,
                self.summary.exposed_tool_count,
            ),
            resources: family(
                self.resources_known,
                self.resources_stale,
                self.resources_error.as_ref(),
                self.summary.discovered_resource_count,
                self.summary.exposed_resource_count,
            ),
            prompts: family(
                self.prompts_known,
                false,
                self.prompts_error.as_ref(),
                self.summary.discovered_prompt_count,
                self.summary.exposed_prompt_count,
            ),
            skills: self.skills_observation.clone(),
        }
    }
}

impl UpstreamPool {
    pub(crate) async fn cached_subject_summary(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> SubjectSummary {
        if !self.upstream_config_matches(config) {
            return SubjectSummary::default();
        }
        let Some(subject) = subject else {
            return SubjectSummary::default();
        };
        let key = (config.name.clone(), subject.to_owned());
        let skills = self
            .skills_cache
            .read()
            .await
            .get(&(config.name.clone(), Some(subject.to_owned())))
            .map(|cached| cached.read_snapshot());
        // Hold one subject snapshot while reading its paired error, matching
        // refresh/failure publication's subject-then-error lock order.
        let cache = self.subject_connections.read().await;
        let last_error = self
            .subject_connect_errors
            .read()
            .await
            .get(&key)
            .filter(|entry| {
                cache.contains_key(&key) || entry.recorded_at.elapsed() < SUBJECT_CONN_IDLE_TTL
            })
            .map(|entry| entry.message.clone());
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
        let skills_observation = match skills {
            Some(skills) => {
                use crate::gateway::view_models::{
                    CapabilityFamilyObservation as Family, CapabilityObservationState as State,
                };
                let policy = super::entries::resolve_request_skill_exposure_policy(
                    &config.name,
                    config.expose_skills.clone(),
                );
                Family::observed(
                    if connected && skills.is_fresh() {
                        State::Known
                    } else {
                        State::Stale
                    },
                    skills.skills.discovered_count,
                    if connected && config.proxy_skills {
                        skills
                            .skills
                            .skills
                            .iter()
                            .filter(|skill| policy.matches(&skill.name))
                            .count()
                    } else {
                        0
                    },
                )
            }
            None if entry.peer.peer_info().is_some()
                && !super::skills_list::peer_declares_skills(&entry.peer) =>
            {
                use crate::gateway::view_models::CapabilityFamilyObservation as Family;
                Family::from_snapshot(Some((0, 0)), connected, false, None)
            }
            None => Default::default(),
        };
        SubjectSummary {
            skills_observation: skills_observation.clone(),
            connected,
            last_error,
            tools_known: true,
            resources_known: entry.optional_catalogs.resources.is_some(),
            resources_stale: entry
                .optional_catalogs
                .resources_listed_at
                .is_some_and(|at| at.elapsed() >= RESOURCE_SNAPSHOT_MAX_AGE),
            resources_error: entry.optional_catalogs.resources_error.clone(),
            prompts_error: entry.optional_catalogs.prompts_error.clone(),
            prompts_known: entry.optional_catalogs.prompts.is_some(),
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
                discovered_skill_count: skills_observation.discovered.unwrap_or_default(),
                exposed_skill_count: skills_observation.exposed.unwrap_or_default(),
                supports_skills: entry
                    .peer
                    .peer_info()
                    .map(|_| super::skills_list::peer_declares_skills(&entry.peer)),
            },
        }
    }

    pub(super) async fn record_subject_optional_failure(
        &self,
        name: &str,
        subject: &str,
        peer: &Peer<RoleClient>,
        resources: bool,
        error: &str,
    ) {
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
        let error = Some(labby_runtime::redact::sanitize_error_text(error, 512));
        if resources {
            entry.optional_catalogs.resources_error = error;
        } else {
            entry.optional_catalogs.prompts_error = error;
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
            entry.optional_catalogs.resources_error = None;
            entry.optional_catalogs.resources = Some(resources.into());
            entry.optional_catalogs.resources_listed_at = Some(Instant::now());
        }
        if let Some(prompts) = prompts {
            entry.optional_catalogs.prompts_error = None;
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
#[path = "scoped_summary_tests.rs"]
mod tests;
