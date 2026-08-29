//! Production ExecutionLoadout catalog projection from canonical owning stores.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use labby_gateway::gateway::manager::GatewayManager;
use labby_gateway::gateway::{
    CapabilityCatalogSnapshot, CapabilityFamily, CapabilityRef, ExecutionCapabilityCatalogProvider,
    ExecutionLoadoutError,
};
use labby_runtime::artifacts::{ArtifactStore, SkillVisibility};
use sha2::{Digest, Sha256};

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
    fn snapshot<'a>(
        &'a self,
        manager: &'a GatewayManager,
        principal: &'a str,
        tenant: &'a str,
        allowed_upstreams: Option<&'a BTreeSet<String>>,
        requested_members: Option<&'a [CapabilityRef]>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<CapabilityCatalogSnapshot, ExecutionLoadoutError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let mut members = manager
                .canonical_upstream_execution_capabilities(allowed_upstreams, requested_members)
                .await?;
            let requested = |provider: &str, family: CapabilityFamily, member_id: &str| {
                requested_members.is_none_or(|items| {
                    items.iter().any(|item| {
                        item.provider == provider
                            && item.family == family
                            && item.member_id == member_id
                    })
                })
            };
            let wants_labby_artifacts = requested_members.is_none_or(|items| {
                items.iter().any(|item| {
                    item.provider == "labby"
                        && matches!(
                            item.family,
                            CapabilityFamily::Skill
                                | CapabilityFamily::Agent
                                | CapabilityFamily::Prompt
                                | CapabilityFamily::Resource
                                | CapabilityFamily::Plugin
                        )
                })
            });
            if wants_labby_artifacts {
                let library = self.artifacts.library_snapshot().map_err(catalog_error)?;
                for record in library.records.values().filter(|record| {
                    !record.archived
                        && record.active_revision_id.is_some()
                        && record.ownership.tenant_id.as_str() == tenant
                        && (record.visibility == SkillVisibility::Tenant
                            || record.ownership.owner_id.as_str() == principal)
                }) {
                    let artifact = self
                        .artifacts
                        .get(&record.artifact_id)
                        .map_err(catalog_error)?;
                    let family = match artifact.descriptor.kind.as_str() {
                        "skill" => CapabilityFamily::Skill,
                        "agent" => CapabilityFamily::Agent,
                        "prompt" => CapabilityFamily::Prompt,
                        "resource" => CapabilityFamily::Resource,
                        "plugin" => CapabilityFamily::Plugin,
                        _ => continue,
                    };
                    if requested("labby", family, &record.artifact_id) {
                        members.push(CapabilityRef {
                            provider: "labby".into(),
                            family,
                            member_id: record.artifact_id.clone(),
                            expected_revision: record
                                .active_revision_id
                                .clone()
                                .expect("filtered above"),
                        });
                    }
                }
            }

            let wants_mcp_apps = requested_members.is_none_or(|items| {
                items
                    .iter()
                    .any(|item| item.provider == "labby" && item.family == CapabilityFamily::McpApp)
            });
            if wants_mcp_apps {
                let config = manager.current_config().await;
                for (id, revision) in
                    crate::mcp::handlers_resources::execution_loadout_mcp_app_catalog(
                        config.code_mode.mcp_ui_enabled,
                        config.mcp_apps,
                    )
                {
                    if requested("labby", CapabilityFamily::McpApp, &id) {
                        members.push(CapabilityRef {
                            provider: "labby".into(),
                            family: CapabilityFamily::McpApp,
                            member_id: id,
                            expected_revision: revision,
                        });
                    }
                }
            }

            let wants_installed_plugins = requested_members.is_none_or(|items| {
                items.iter().any(|item| {
                    item.provider == "claude-code" && item.family == CapabilityFamily::Plugin
                })
            });
            if self.include_installed_plugins && wants_installed_plugins {
                #[cfg(test)]
                let plugins = if let Some(plugins) = &self.test_plugins {
                    plugins.clone()
                } else {
                    super::setup::claude_plugins::installed_plugins(false)
                        .await
                        .map_err(|error| ExecutionLoadoutError::Storage {
                            message: error.to_string(),
                        })?
                };
                #[cfg(not(test))]
                let plugins = super::setup::claude_plugins::installed_plugins(false)
                    .await
                    .map_err(|error| ExecutionLoadoutError::Storage {
                        message: error.to_string(),
                    })?;
                for plugin in plugins
                    .into_iter()
                    .filter(|plugin| plugin.enabled)
                    .filter_map(|plugin| plugin.version.map(|version| (plugin.id, version)))
                {
                    if requested("claude-code", CapabilityFamily::Plugin, &plugin.0) {
                        members.push(CapabilityRef {
                            provider: "claude-code".into(),
                            family: CapabilityFamily::Plugin,
                            member_id: plugin.0,
                            expected_revision: plugin.1,
                        });
                    }
                }
            }
            members.sort();
            members.dedup();
            let generation = generation(&members)?;
            Ok(CapabilityCatalogSnapshot {
                generation,
                principal: principal.into(),
                members,
            })
        })
    }
}

fn generation(members: &[CapabilityRef]) -> Result<String, ExecutionLoadoutError> {
    let bytes = serde_json::to_vec(members).map_err(|_| ExecutionLoadoutError::Storage {
        message: "canonical execution catalog is not serializable".into(),
    })?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

fn catalog_error(error: impl std::fmt::Display) -> ExecutionLoadoutError {
    ExecutionLoadoutError::Storage {
        message: error.to_string(),
    }
}

#[cfg(test)]
#[path = "execution_catalog_tests.rs"]
mod tests;
