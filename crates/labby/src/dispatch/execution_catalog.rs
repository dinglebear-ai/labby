//! Production ExecutionLoadout catalog projection from canonical owning stores.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use labby_gateway::gateway::manager::GatewayManager;
use labby_gateway::gateway::{
    CapabilityFamily, CapabilityRef, ExecutionCapabilityCatalogProvider, ExecutionLoadoutContext,
    ExecutionLoadoutError,
};
use labby_runtime::artifacts::ArtifactStore;

pub(crate) struct CanonicalExecutionCatalogProvider {
    artifacts: Arc<ArtifactStore>,
    include_installed_plugins: bool,
}

impl CanonicalExecutionCatalogProvider {
    pub(crate) fn new(artifacts: Arc<ArtifactStore>) -> Self {
        Self {
            artifacts,
            include_installed_plugins: true,
        }
    }

    pub(crate) fn production()
    -> Result<Arc<dyn ExecutionCapabilityCatalogProvider>, ExecutionLoadoutError> {
        let store = ArtifactStore::new(labby_runtime::lab_home().join("artifacts"))
            .map_err(catalog_error)?;
        Ok(Arc::new(Self::new(Arc::new(store))))
    }
}

impl ExecutionCapabilityCatalogProvider for CanonicalExecutionCatalogProvider {
    fn members<'a>(
        &'a self,
        manager: &'a GatewayManager,
        context: &'a ExecutionLoadoutContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CapabilityRef>, ExecutionLoadoutError>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut members = manager
                .canonical_upstream_execution_capabilities(context)
                .await?;

            let library = self.artifacts.library_snapshot().map_err(catalog_error)?;
            for record in library.records.values().filter(|record| {
                !record.archived
                    && record.materialized
                    && record.active_revision_id.is_some()
                    && record.ownership.owner_id.as_str() == context.principal.as_str()
            }) {
                members.push(CapabilityRef {
                    provider: "labby".into(),
                    family: CapabilityFamily::Skill,
                    member_id: record.artifact_id.clone(),
                    expected_revision: record.active_revision_id.clone().expect("filtered above"),
                });
            }

            for artifact in self.artifacts.list_records().map_err(catalog_error)? {
                let family = match artifact.descriptor.kind.as_str() {
                    "prompt" => CapabilityFamily::Prompt,
                    "agent" => CapabilityFamily::Agent,
                    "hook" => CapabilityFamily::Hook,
                    _ => continue,
                };
                members.push(CapabilityRef {
                    provider: "labby".into(),
                    family,
                    member_id: artifact.descriptor.id,
                    expected_revision: artifact.current_revision_id,
                });
            }

            let config = manager.current_config().await;
            for (id, revision) in crate::mcp::handlers_resources::execution_loadout_mcp_app_catalog(
                config.code_mode.mcp_ui_enabled,
                config.mcp_apps,
            ) {
                members.push(CapabilityRef {
                    provider: "labby".into(),
                    family: CapabilityFamily::McpApp,
                    member_id: id,
                    expected_revision: revision,
                });
            }
            if self.include_installed_plugins
                && context
                    .allowed_providers
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains("claude-code"))
            {
                for plugin in super::setup::claude_plugins::installed_plugins(false)
                    .await
                    .map_err(catalog_error)?
                    .into_iter()
                    .filter(|plugin| plugin.enabled)
                {
                    members.push(CapabilityRef {
                        provider: "claude-code".into(),
                        family: CapabilityFamily::Plugin,
                        member_id: plugin.id,
                        expected_revision: plugin.version.unwrap_or_else(|| "installed".into()),
                    });
                }
            }
            Ok(members)
        })
    }
}

fn catalog_error(error: impl std::fmt::Display) -> ExecutionLoadoutError {
    ExecutionLoadoutError::Storage {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use labby_gateway::gateway::ExecutionPrincipal;
    use labby_gateway::gateway::manager::GatewayRuntimeHandle;
    use labby_runtime::artifacts::ArtifactImportRequest;

    #[tokio::test]
    async fn generic_artifact_heads_supply_prompt_agent_and_hook_capabilities() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ArtifactStore::new(directory.path().join("artifacts")).unwrap());
        for kind in ["prompt", "agent", "hook"] {
            let source = directory.path().join(format!("{kind}.md"));
            std::fs::write(&source, kind).unwrap();
            store
                .import_local(ArtifactImportRequest::new(kind, "test", kind), &source)
                .unwrap();
        }
        let manager = crate::dispatch::gateway::config_store::test_gateway_manager(
            directory.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        );
        let context = ExecutionLoadoutContext {
            principal: ExecutionPrincipal::new("principal-1").unwrap(),
            allowed_providers: Some(BTreeSet::new()),
        };

        let mut provider = CanonicalExecutionCatalogProvider::new(store);
        provider.include_installed_plugins = false;
        let members = provider.members(&manager, &context).await.unwrap();
        let families = members
            .iter()
            .filter(|member| member.provider == "labby")
            .map(|member| member.family)
            .collect::<BTreeSet<_>>();
        assert!(families.contains(&CapabilityFamily::Prompt));
        assert!(families.contains(&CapabilityFamily::Agent));
        assert!(families.contains(&CapabilityFamily::Hook));
    }
}
