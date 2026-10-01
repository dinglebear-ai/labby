//! Family-specific cache projections; inspection never clones unrelated schemas.
use super::entries::resolve_request_exposure_policy;
use super::{SubjectScopedConnection, UpstreamPool, helpers::SUBJECT_CONN_IDLE_TTL};
use labby_runtime::gateway_config::UpstreamConfig;

impl UpstreamPool {
    async fn project_subject_inventory<T>(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        project: impl FnOnce(&SubjectScopedConnection, bool) -> T,
    ) -> Option<T> {
        let subject = subject?;
        if !self.upstream_config_matches(config) {
            return None;
        }
        let cache = self.subject_connections.read().await;
        let entry = cache.get(&(config.name.clone(), subject.to_owned()))?;
        let connected = config.enabled
            && !entry.peer.is_transport_closed()
            && entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL;
        Some(project(entry, connected))
    }

    pub(crate) async fn cached_subject_tool_inventory(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Vec<crate::upstream::types::UpstreamToolExposureRow> {
        self.project_subject_inventory(config, subject, |entry, connected| {
            let policy = resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
            let mut tools: Vec<_> = entry
                .tools
                .iter()
                .map(|tool| {
                    let matched_by = policy.matched_by(tool.name.as_ref());
                    crate::upstream::types::UpstreamToolExposureRow {
                        name: tool.name.to_string(),
                        description: tool.description.as_ref().map(|description| {
                            crate::gateway::projection::sanitize_tool_text(description, 2048)
                        }),
                        exposed: connected && matched_by.is_some(),
                        matched_by,
                    }
                })
                .collect();
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            tools
        })
        .await
        .unwrap_or_default()
    }

    pub(crate) async fn cached_subject_resource_inventory(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Vec<String> {
        self.project_subject_inventory(config, subject, |entry, _| {
            let mut resources = entry
                .optional_catalogs
                .resources
                .as_ref()
                .map_or_else(Vec::new, |rows| {
                    rows.iter().map(|row| row.uri.clone()).collect()
                });
            resources.sort();
            resources
        })
        .await
        .unwrap_or_default()
    }

    pub(crate) async fn cached_subject_prompt_inventory(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
    ) -> Vec<String> {
        self.project_subject_inventory(config, subject, |entry, _| {
            let mut prompts = entry.optional_catalogs.prompts.clone().unwrap_or_default();
            prompts.sort();
            prompts
        })
        .await
        .unwrap_or_default()
    }
}
