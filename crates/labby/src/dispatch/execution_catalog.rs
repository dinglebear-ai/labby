//! Production ExecutionLoadout catalog projection from canonical owning stores.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use labby_gateway::gateway::manager::GatewayManager;
use labby_gateway::gateway::{
    CapabilityFamily, CapabilityRef, ExecutionCapabilityCatalogProvider, ExecutionLoadoutContext,
    ExecutionLoadoutError,
};
use labby_runtime::artifacts::{ArtifactStore, PublicationState, Visibility};

pub(crate) struct CanonicalExecutionCatalogProvider {
    artifacts: Arc<ArtifactStore>,
    include_installed_plugins: bool,
    #[cfg(test)]
    test_plugins: Option<Vec<super::setup::claude_plugins::InstalledPlugin>>,
}

impl CanonicalExecutionCatalogProvider {
    pub(crate) fn new(artifacts: Arc<ArtifactStore>) -> Self {
        Self {
            artifacts,
            include_installed_plugins: true,
            #[cfg(test)]
            test_plugins: None,
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
                if artifact.publication.state != PublicationState::Published
                    || artifact.publication.visibility != Visibility::Public
                {
                    continue;
                }
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
                #[cfg(test)]
                let plugins = if let Some(plugins) = &self.test_plugins {
                    plugins.clone()
                } else {
                    super::setup::claude_plugins::installed_plugins(false)
                        .await
                        .map_err(catalog_error)?
                };
                #[cfg(not(test))]
                let plugins = super::setup::claude_plugins::installed_plugins(false)
                    .await
                    .map_err(catalog_error)?;
                for (id, version) in plugins
                    .into_iter()
                    .filter(|plugin| plugin.enabled)
                    .filter_map(|plugin| plugin.version.map(|version| (plugin.id, version)))
                {
                    members.push(CapabilityRef {
                        provider: "claude-code".into(),
                        family: CapabilityFamily::Plugin,
                        member_id: id,
                        expected_revision: version,
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
    use labby_runtime::artifacts::{ArtifactImportRequest, ArtifactRecord};

    fn set_publication(
        store: &ArtifactStore,
        artifact_id: &str,
        state: PublicationState,
        visibility: Visibility,
    ) {
        let artifacts = store.root().join("artifacts");
        for entry in std::fs::read_dir(artifacts).unwrap() {
            let path = entry.unwrap().path().join("artifact.json");
            let mut record: ArtifactRecord =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if record.descriptor.id == artifact_id {
                record.publication.state = state;
                record.publication.visibility = visibility;
                record.validate().unwrap();
                std::fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
                return;
            }
        }
        panic!("artifact head was not found");
    }

    #[tokio::test]
    async fn catalogs_include_only_public_published_generic_heads_and_versioned_plugins() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ArtifactStore::new(directory.path().join("artifacts")).unwrap());
        let mut public_ids = BTreeSet::new();
        let mut private_ids = BTreeSet::new();
        for kind in ["prompt", "agent", "hook"] {
            let private_source = directory.path().join(format!("private-{kind}.md"));
            std::fs::write(&private_source, kind).unwrap();
            let private = store
                .import_local(
                    ArtifactImportRequest::new(kind, "test", format!("private-{kind}")),
                    &private_source,
                )
                .unwrap();
            private_ids.insert(private.descriptor.id);

            let public_source = directory.path().join(format!("public-{kind}.md"));
            std::fs::write(&public_source, kind).unwrap();
            let public = store
                .import_local(
                    ArtifactImportRequest::new(kind, "test", format!("public-{kind}")),
                    &public_source,
                )
                .unwrap();
            set_publication(
                &store,
                &public.descriptor.id,
                PublicationState::Published,
                Visibility::Public,
            );
            public_ids.insert(public.descriptor.id);
        }
        for (name, state, visibility) in [
            ("public-draft", PublicationState::Draft, Visibility::Public),
            (
                "unlisted-published",
                PublicationState::Published,
                Visibility::Unlisted,
            ),
            (
                "public-withdrawn",
                PublicationState::Withdrawn,
                Visibility::Public,
            ),
            (
                "private-published",
                PublicationState::Published,
                Visibility::Private,
            ),
        ] {
            let source = directory.path().join(format!("{name}.md"));
            std::fs::write(&source, name).unwrap();
            let excluded = store
                .import_local(ArtifactImportRequest::new("prompt", "test", name), &source)
                .unwrap();
            set_publication(&store, &excluded.descriptor.id, state, visibility);
            private_ids.insert(excluded.descriptor.id);
        }
        let manager = crate::dispatch::gateway::config_store::test_gateway_manager(
            directory.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        );
        let mut provider = CanonicalExecutionCatalogProvider::new(store);
        provider.test_plugins = Some(vec![
            super::super::setup::claude_plugins::InstalledPlugin {
                id: "versioned@lab".into(),
                scope: "user".into(),
                version: Some("1.2.3".into()),
                enabled: true,
            },
            super::super::setup::claude_plugins::InstalledPlugin {
                id: "unversioned@lab".into(),
                scope: "user".into(),
                version: None,
                enabled: true,
            },
        ]);
        for principal in ["principal-1", "principal-2"] {
            let context = ExecutionLoadoutContext {
                principal: ExecutionPrincipal::new(principal).unwrap(),
                allowed_providers: None,
            };
            let members = provider.members(&manager, &context).await.unwrap();
            let generic_ids = members
                .iter()
                .filter(|member| {
                    member.provider == "labby"
                        && matches!(
                            member.family,
                            CapabilityFamily::Prompt
                                | CapabilityFamily::Agent
                                | CapabilityFamily::Hook
                        )
                })
                .map(|member| member.member_id.clone())
                .collect::<BTreeSet<_>>();
            assert_eq!(generic_ids, public_ids);
            assert!(generic_ids.is_disjoint(&private_ids));
            assert!(members.iter().any(|member| {
                member.provider == "claude-code"
                    && member.member_id == "versioned@lab"
                    && member.expected_revision == "1.2.3"
            }));
            assert!(
                members
                    .iter()
                    .all(|member| member.member_id != "unversioned@lab")
            );
        }
    }
}
