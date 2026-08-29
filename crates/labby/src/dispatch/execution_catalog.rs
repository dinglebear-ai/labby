//! Production ExecutionLoadout catalog projection from canonical owning stores.

use std::collections::BTreeMap;
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
        allowed_upstreams: Option<&'a BTreeMap<String, ()>>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<CapabilityCatalogSnapshot, ExecutionLoadoutError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let mut members = manager
                .canonical_upstream_execution_capabilities(allowed_upstreams)
                .await?;
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
                members.push(CapabilityRef {
                    provider: "labby".into(),
                    family,
                    member_id: record.artifact_id.clone(),
                    expected_revision: record.active_revision_id.clone().expect("filtered above"),
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

            if self.include_installed_plugins {
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
                    members.push(CapabilityRef {
                        provider: "claude-code".into(),
                        family: CapabilityFamily::Plugin,
                        member_id: plugin.0,
                        expected_revision: plugin.1,
                    });
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
mod tests {
    use super::*;
    use labby_gateway::gateway::manager::GatewayRuntimeHandle;
    use labby_gateway::gateway::palette::PaletteCaller;
    use labby_gateway::gateway::{ExecutionLoadoutCreate, ResolutionStatus};
    use labby_gateway::upstream::pool::UpstreamPool;
    use labby_runtime::artifacts::{
        ArtifactImportRequest, LibraryActorId, LibraryAuthorization, LibraryGrant,
        LibraryIdempotency, LibraryMutation, LibraryOwnership, LibraryTenantId, LibraryTimestamp,
        SkillLibraryRecord,
    };
    use labby_runtime::gateway_config::{GatewayConfig, McpAppsConfig, UpstreamConfig};
    use rmcp::model::Resource;

    fn add_artifact(
        store: &ArtifactStore,
        source_root: &std::path::Path,
        version: u64,
        kind: &str,
        name: &str,
        tenant: &str,
        owner: &str,
        visibility: SkillVisibility,
    ) -> CapabilityRef {
        let source = source_root.join(format!("{kind}-{name}.md"));
        std::fs::write(&source, format!("{kind} {name}")).unwrap();
        let artifact = store
            .import_local(ArtifactImportRequest::new(kind, "test", name), &source)
            .unwrap();
        let ownership = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection(tenant).unwrap(),
            LibraryActorId::from_canonical_projection(owner).unwrap(),
        );
        let authorization = LibraryAuthorization::from_authorized_access_projection(
            ownership.tenant_id.clone(),
            ownership.owner_id.clone(),
            LibraryGrant::Owner,
        );
        let timestamp = LibraryTimestamp::parse("2026-08-29T00:00:00Z").unwrap();
        store
            .mutate_library(
                &authorization,
                &ownership,
                version,
                LibraryIdempotency {
                    key: format!("create-{kind}-{name}"),
                    request_digest: format!("sha256:{}", "0".repeat(64)),
                    terminal_audit: None,
                },
                LibraryMutation::Create {
                    record: SkillLibraryRecord {
                        artifact_id: artifact.descriptor.id.clone(),
                        name: name.into(),
                        ownership: ownership.clone(),
                        visibility,
                        archived: false,
                        active_revision_id: None,
                        latest_revision_id: artifact.current_revision_id.clone(),
                        latest_revision_files: Vec::new(),
                        provenance_provider: None,
                        materialized: true,
                        created_at: timestamp.clone(),
                        updated_at: timestamp.clone(),
                    },
                },
                timestamp.clone(),
            )
            .unwrap();
        store
            .mutate_library(
                &authorization,
                &ownership,
                version + 1,
                LibraryIdempotency {
                    key: format!("activate-{kind}-{name}"),
                    request_digest: format!("sha256:{}", "1".repeat(64)),
                    terminal_audit: None,
                },
                LibraryMutation::Activate {
                    artifact_id: artifact.descriptor.id.clone(),
                    revision_id: artifact.current_revision_id.clone(),
                    updated_at: timestamp.clone(),
                },
                timestamp,
            )
            .unwrap();
        CapabilityRef {
            provider: "labby".into(),
            family: match kind {
                "skill" => CapabilityFamily::Skill,
                "agent" => CapabilityFamily::Agent,
                "prompt" => CapabilityFamily::Prompt,
                "resource" => CapabilityFamily::Resource,
                "plugin" => CapabilityFamily::Plugin,
                _ => unreachable!(),
            },
            member_id: artifact.descriptor.id,
            expected_revision: artifact.current_revision_id,
        }
    }

    #[tokio::test]
    async fn production_provider_activates_authorized_mixed_families_and_rejects_missing_or_private()
     {
        let directory = tempfile::tempdir().unwrap();
        let canonical_root = std::fs::canonicalize(directory.path()).unwrap();
        let store = Arc::new(ArtifactStore::new(canonical_root.join("artifacts")).unwrap());
        let mut members = ["skill", "agent", "prompt", "resource", "plugin"]
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                add_artifact(
                    store.as_ref(),
                    &canonical_root,
                    (index * 2) as u64,
                    kind,
                    kind,
                    "tenant-1",
                    "principal-1",
                    SkillVisibility::Tenant,
                )
            })
            .collect::<Vec<_>>();
        let private = add_artifact(
            store.as_ref(),
            &canonical_root,
            (members.len() * 2) as u64,
            "skill",
            "private-other",
            "tenant-1",
            "principal-2",
            SkillVisibility::Private,
        );
        let same_tenant = members.clone();
        let cross_tenant = ["skill", "agent", "prompt", "resource", "plugin"]
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                add_artifact(
                    store.as_ref(),
                    &canonical_root,
                    12 + (index * 2) as u64,
                    kind,
                    &format!("cross-tenant-{kind}"),
                    "tenant-2",
                    "principal-2",
                    SkillVisibility::Tenant,
                )
            })
            .collect::<Vec<_>>();
        let catalog_store = Arc::clone(&store);
        let provider = Arc::new(CanonicalExecutionCatalogProvider {
            artifacts: store,
            include_installed_plugins: true,
            test_plugins: Some(vec![
                crate::dispatch::setup::claude_plugins::InstalledPlugin {
                    id: "plugin-1@lab".into(),
                    scope: "user".into(),
                    version: Some("1.0.0".into()),
                    enabled: true,
                },
                crate::dispatch::setup::claude_plugins::InstalledPlugin {
                    id: "unversioned@lab".into(),
                    scope: "user".into(),
                    version: None,
                    enabled: true,
                },
            ]),
        });
        let runtime = GatewayRuntimeHandle::default();
        let pool = Arc::new(UpstreamPool::new());
        pool.insert_resource_routes_for_tests(
            "server-1",
            vec![Resource::new("resource://one", "resource-one")],
        )
        .await;
        runtime.swap(Some(pool)).await;
        let manager = GatewayManager::new(canonical_root.join("config.toml"), runtime)
            .with_execution_capability_provider(provider.clone());
        manager
            .try_seed_config(GatewayConfig {
                mcp_apps: McpAppsConfig {
                    manager: true,
                    ..Default::default()
                },
                upstream: vec![
                    serde_json::from_value::<UpstreamConfig>(serde_json::json!({
                        "name": "server-1",
                        "url": "http://127.0.0.1:9/mcp"
                    }))
                    .unwrap(),
                ],
                ..Default::default()
            })
            .await
            .unwrap();
        let snapshot = provider
            .snapshot(&manager, "principal-1", "tenant-1", None)
            .await
            .unwrap();
        members = snapshot.members.clone();
        assert_eq!(
            members
                .iter()
                .map(|member| member.family)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            7,
            "snapshot members: {:?}",
            snapshot.members
        );
        assert!(snapshot.members.iter().any(|member| {
            member.provider == "claude-code"
                && member.member_id == "plugin-1@lab"
                && member.expected_revision == "1.0.0"
        }));
        assert!(
            snapshot
                .members
                .iter()
                .all(|member| member.member_id != "unversioned@lab")
        );
        for rejected in &cross_tenant {
            assert!(
                snapshot.members.iter().all(|member| member != rejected),
                "cross-tenant {:?} leaked into the catalog",
                rejected.family
            );
        }
        for expected in &same_tenant {
            assert!(
                snapshot.members.iter().any(|member| member == expected),
                "same-tenant {:?} was not published",
                expected.family
            );
        }

        let caller = PaletteCaller::admin(Some("principal-1"), Some("request-1"))
            .with_catalog_tenant("tenant-1");
        manager
            .execution_loadout_create(
                &caller,
                ExecutionLoadoutCreate {
                    id: "mixed-production".into(),
                    name: "Mixed production".into(),
                    description: None,
                    members: members.clone(),
                },
            )
            .await
            .unwrap();
        let activated = manager
            .execution_loadout_activate(&caller, "mixed-production", 1, "runtime-1")
            .await
            .unwrap();
        assert_eq!(activated.revision.members, members);

        let skill = members
            .iter()
            .find(|member| member.family == CapabilityFamily::Skill)
            .unwrap();
        let ownership = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("tenant-1").unwrap(),
            LibraryActorId::from_canonical_projection("principal-1").unwrap(),
        );
        let authorization = LibraryAuthorization::from_authorized_access_projection(
            ownership.tenant_id.clone(),
            ownership.owner_id.clone(),
            LibraryGrant::Owner,
        );
        let timestamp = LibraryTimestamp::parse("2026-08-29T00:01:00Z").unwrap();
        catalog_store
            .mutate_library(
                &authorization,
                &ownership,
                22,
                LibraryIdempotency {
                    key: "deactivate-skill".into(),
                    request_digest: format!("sha256:{}", "2".repeat(64)),
                    terminal_audit: None,
                },
                LibraryMutation::Deactivate {
                    artifact_id: skill.member_id.clone(),
                    updated_at: timestamp.clone(),
                },
                timestamp,
            )
            .unwrap();
        let invalidated = manager
            .execution_loadout_preview(&caller, "mixed-production", "runtime-1")
            .await
            .unwrap();
        assert!(invalidated.resolved.iter().any(|member| {
            member.capability.family == CapabilityFamily::Skill
                && member.status == ResolutionStatus::Missing
        }));

        for (id, rejected) in [
            (
                "missing",
                CapabilityRef {
                    member_id: "missing".into(),
                    ..members[0].clone()
                },
            ),
            ("private", private),
        ] {
            manager
                .execution_loadout_create(
                    &caller,
                    ExecutionLoadoutCreate {
                        id: id.into(),
                        name: id.into(),
                        description: None,
                        members: vec![rejected],
                    },
                )
                .await
                .unwrap();
            let error = manager
                .execution_loadout_activate(&caller, id, 1, "runtime-1")
                .await
                .expect_err("unpublished capability must be rejected");
            let text = error.to_string();
            assert!(text.contains("unresolved") || text.contains("not visible"));
        }

        let preview = manager
            .execution_loadout_preview(&caller, "private", "runtime-1")
            .await
            .unwrap();
        assert_eq!(preview.resolved[0].status, ResolutionStatus::Missing);

        for (index, rejected) in cross_tenant.into_iter().enumerate() {
            let id = format!("cross-tenant-{index}");
            manager
                .execution_loadout_create(
                    &caller,
                    ExecutionLoadoutCreate {
                        id: id.clone(),
                        name: id.clone(),
                        description: None,
                        members: vec![rejected],
                    },
                )
                .await
                .unwrap();
            let preview = manager
                .execution_loadout_preview(&caller, &id, "runtime-1")
                .await
                .unwrap();
            assert_eq!(preview.resolved[0].status, ResolutionStatus::Missing);
        }
    }
}
