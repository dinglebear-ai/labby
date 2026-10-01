//! Cache-only global capability observations, with availability independent of numeric compatibility fields.
use super::entries::{prompt_exposed, resource_exposed};
use super::{UpstreamPool, helpers::RESOURCE_SNAPSHOT_MAX_AGE};
impl UpstreamPool {
    pub(crate) async fn cached_global_observation(
        &self,
        name: &str,
    ) -> crate::gateway::view_models::CapabilityObservation {
        use crate::gateway::view_models::{
            CapabilityFamilyObservation as Family, CapabilityObservation as Observation,
        };
        let listening = self.subscription_resources.read().await.contains_key(name);
        let cached_skills = self
            .skills_cache
            .read()
            .await
            .get(&(name.to_owned(), None))
            .map(|cached| cached.read_snapshot());
        let connected = self
            .connections
            .read()
            .await
            .get(name)
            .is_some_and(|entry| !entry.peer.is_transport_closed());
        let catalog = self.catalog.read().await;
        let Some(entry) = catalog.get(name) else {
            return Observation::default();
        };
        let mut summary =
            super::helpers::catalog_entry_summary(entry, catalog.resource_rows_withheld(name));
        // Compatibility counts retain their historical shape, but observation
        // exposure is calculated from the actual current proxy policy.
        summary.exposed_tool_count = if connected && entry.tool_health.is_routable() {
            summary.exposed_tool_count
        } else {
            0
        };
        summary.exposed_resource_count = if connected
            && entry.proxy_resources
            && entry.resource_health.is_routable()
            && !catalog.resource_rows_withheld(name)
        {
            entry
                .resource_uris
                .iter()
                .filter(|uri| resource_exposed(&entry.resource_exposure_policy, uri))
                .count()
        } else {
            0
        };
        summary.exposed_prompt_count = if connected && entry.prompt_health.is_routable() {
            entry
                .prompt_names
                .iter()
                .filter(|prompt| prompt_exposed(&entry.prompt_exposure_policy, name, prompt))
                .count()
        } else {
            0
        };
        summary.exposed_skill_count = if connected {
            summary.exposed_skill_count
        } else {
            0
        };
        let family = |known: bool, stale: bool, error: bool, discovered, exposed| {
            Family::from_snapshot(
                known.then_some((discovered, exposed)),
                connected,
                stale,
                error.then(|| "Capability discovery failed; refresh to retry.".to_owned()),
            )
        };
        let resources_at = catalog.resource_snapshot_listed_at(name);
        Observation {
            tools: family(
                entry.supports_skills.is_some() || !entry.tools.is_empty(),
                false,
                entry.tool_last_error.is_some(),
                summary.discovered_tool_count,
                summary.exposed_tool_count,
            ),
            resources: family(
                resources_at.is_some(),
                resources_at.is_some_and(|at| at.elapsed() >= RESOURCE_SNAPSHOT_MAX_AGE)
                    && !listening,
                entry.resource_last_error.is_some() || catalog.resource_rows_withheld(name),
                summary.discovered_resource_count,
                summary.exposed_resource_count,
            ),
            prompts: family(
                catalog.has_prompt_snapshot(name),
                false,
                entry.prompt_last_error.is_some(),
                summary.discovered_prompt_count,
                summary.exposed_prompt_count,
            ),
            skills: family(
                entry.supports_skills == Some(false) || cached_skills.is_some(),
                cached_skills
                    .as_ref()
                    .is_some_and(|cached| !cached.is_fresh()),
                entry.skill_last_error.is_some(),
                summary.discovered_skill_count,
                summary.exposed_skill_count,
            ),
            ..Observation::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::*;
    use crate::gateway::view_models::CapabilityObservationState as State;
    use crate::upstream::types::ToolExposurePolicy;
    #[tokio::test]
    async fn global_observation_keeps_empty_unobserved_and_filtered_families_distinct() {
        let pool = static_catalog_pool("alpha").await;
        let initial = pool.cached_global_observation("alpha").await;
        assert_eq!(initial.tools.state, State::Unknown);
        assert_eq!(initial.skills.discovered, None);
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").unwrap();
            // This marker is set by a completed initialize/tools handshake,
            // independent of whether the measured tool list is empty.
            entry.supports_skills = Some(false);
            entry.resource_exposure_policy =
                ToolExposurePolicy::from_optional(Some(vec!["file:///tmp/upstream-one".into()]))
                    .unwrap();
        }
        let empty = pool.cached_global_observation("alpha").await;
        assert_eq!(empty.tools.state, State::Known);
        assert_eq!(empty.tools.discovered, Some(0));
        assert_eq!(empty.skills.discovered, Some(0));
        pool.list_upstream_resources().await;
        let observed = pool.cached_global_observation("alpha").await;
        assert_eq!(observed.resources.discovered, Some(2));
        assert_eq!(observed.resources.exposed, Some(1));
    }
    #[tokio::test]
    async fn disconnected_global_skills_are_not_exposed() {
        let pool = static_catalog_pool("alpha").await;
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").unwrap();
            entry.supports_skills = Some(false);
            entry.proxy_skills = true;
            entry.skill_count = 1;
            entry.skill_names = vec!["cached-skill".into()];
        }
        pool.connections.write().await.remove("alpha");
        let observed = pool.cached_global_observation("alpha").await;
        assert_eq!(observed.skills.state, State::Stale);
        assert_eq!(observed.skills.discovered, Some(1));
        assert_eq!(observed.skills.exposed, Some(0));
    }
}
