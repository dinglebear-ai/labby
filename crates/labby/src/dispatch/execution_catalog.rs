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
                    && record.ownership.tenant_id.as_str() == context.tenant.as_str()
                    && (record.visibility == labby_runtime::artifacts::SkillVisibility::Tenant
                        || record.ownership.owner_id.as_str() == context.principal.as_str())
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
            for (id, revision) in crate::app_catalog::enabled_versions(
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

    use labby_gateway::gateway::manager::GatewayRuntimeHandle;
    use labby_gateway::gateway::{
        CapabilityCatalogSnapshot, ExecutionLoadoutCreate, ExecutionPrincipal, ExecutionTenant,
        ResolutionStatus,
    };
    use labby_runtime::artifacts::{
        ArtifactImportRequest, ArtifactRecord, LibraryActorId, LibraryAuthorization, LibraryGrant,
        LibraryIdempotency, LibraryMutation, LibraryOwnership, LibraryTenantId, LibraryTimestamp,
        SkillLibraryRecord, SkillVisibility,
    };

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

    fn add_skill(
        store: &ArtifactStore,
        root: &std::path::Path,
        version: u64,
        name: &str,
        tenant: &str,
        owner: &str,
        visibility: SkillVisibility,
        activate: bool,
    ) -> CapabilityRef {
        let source = root.join(format!("{name}.md"));
        std::fs::write(&source, name).unwrap();
        let artifact = store
            .import_local(ArtifactImportRequest::new("skill", "test", name), &source)
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
        let timestamp = LibraryTimestamp::parse("2026-09-05T00:00:00Z").unwrap();
        store
            .mutate_library(
                &authorization,
                &ownership,
                version,
                LibraryIdempotency {
                    key: format!("create-{name}"),
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
                        search_metadata: Vec::new(),
                        provenance_provider: None,
                        materialized: true,
                        created_at: timestamp.clone(),
                        updated_at: timestamp.clone(),
                    },
                },
                timestamp.clone(),
            )
            .unwrap();
        if activate {
            store
                .mutate_library(
                    &authorization,
                    &ownership,
                    version + 1,
                    LibraryIdempotency {
                        key: format!("activate-{name}"),
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
        }
        CapabilityRef {
            provider: "labby".into(),
            family: CapabilityFamily::Skill,
            member_id: artifact.descriptor.id,
            expected_revision: artifact.current_revision_id,
        }
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
                tenant: ExecutionTenant::new("tenant-1").unwrap(),
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

    #[tokio::test]
    async fn skill_catalogs_enforce_tenant_owner_and_active_lifecycle_authority() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ArtifactStore::new(directory.path().join("artifacts")).unwrap());
        let private_owner = add_skill(
            &store,
            directory.path(),
            0,
            "private-owner",
            "tenant-1",
            "principal-1",
            SkillVisibility::Private,
            true,
        );
        let tenant_visible = add_skill(
            &store,
            directory.path(),
            2,
            "tenant-visible",
            "tenant-1",
            "principal-2",
            SkillVisibility::Tenant,
            true,
        );
        let private_other = add_skill(
            &store,
            directory.path(),
            4,
            "private-other",
            "tenant-1",
            "principal-2",
            SkillVisibility::Private,
            true,
        );
        let cross_tenant = add_skill(
            &store,
            directory.path(),
            6,
            "cross-tenant",
            "tenant-2",
            "principal-2",
            SkillVisibility::Tenant,
            true,
        );
        let inactive = add_skill(
            &store,
            directory.path(),
            8,
            "inactive",
            "tenant-1",
            "principal-1",
            SkillVisibility::Tenant,
            false,
        );
        let archived = add_skill(
            &store,
            directory.path(),
            9,
            "archived",
            "tenant-1",
            "principal-1",
            SkillVisibility::Tenant,
            true,
        );
        let owner = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("tenant-1").unwrap(),
            LibraryActorId::from_canonical_projection("principal-1").unwrap(),
        );
        let authorization = LibraryAuthorization::from_authorized_access_projection(
            owner.tenant_id.clone(),
            owner.owner_id.clone(),
            LibraryGrant::Owner,
        );
        let timestamp = LibraryTimestamp::parse("2026-09-05T00:00:01Z").unwrap();
        store
            .mutate_library(
                &authorization,
                &owner,
                11,
                LibraryIdempotency {
                    key: "archive-archived".into(),
                    request_digest: format!("sha256:{}", "2".repeat(64)),
                    terminal_audit: None,
                },
                LibraryMutation::Archive {
                    artifact_id: archived.member_id.clone(),
                    updated_at: timestamp.clone(),
                },
                timestamp.clone(),
            )
            .unwrap();

        let manager = crate::dispatch::gateway::config_store::test_gateway_manager(
            directory.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        );
        let mut provider = CanonicalExecutionCatalogProvider::new(Arc::clone(&store));
        provider.include_installed_plugins = false;
        let context = ExecutionLoadoutContext {
            principal: ExecutionPrincipal::new("principal-1").unwrap(),
            tenant: ExecutionTenant::new("tenant-1").unwrap(),
            allowed_providers: Some(BTreeSet::new()),
        };
        let members = provider.members(&manager, &context).await.unwrap();
        assert!(members.contains(&private_owner));
        assert!(members.contains(&tenant_visible));
        assert!(!members.contains(&private_other));
        assert!(!members.contains(&cross_tenant));
        assert!(!members.contains(&inactive));
        assert!(!members.contains(&archived));

        manager
            .publish_execution_capability_snapshots(vec![CapabilityCatalogSnapshot {
                generation: "before-deactivation".into(),
                principal: context.principal.as_str().into(),
                members,
            }])
            .unwrap();
        manager
            .execution_loadout_create(
                &context,
                ExecutionLoadoutCreate {
                    id: "tenant-skill".into(),
                    name: "Tenant skill".into(),
                    description: None,
                    members: vec![tenant_visible.clone()],
                },
            )
            .await
            .unwrap();
        assert_eq!(
            manager
                .execution_loadout_preview(&context, "tenant-skill", "runtime")
                .await
                .unwrap()
                .resolved[0]
                .status,
            ResolutionStatus::Effective
        );

        let tenant_owner = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("tenant-1").unwrap(),
            LibraryActorId::from_canonical_projection("principal-2").unwrap(),
        );
        let tenant_authorization = LibraryAuthorization::from_authorized_access_projection(
            tenant_owner.tenant_id.clone(),
            tenant_owner.owner_id.clone(),
            LibraryGrant::Owner,
        );
        store
            .mutate_library(
                &tenant_authorization,
                &tenant_owner,
                12,
                LibraryIdempotency {
                    key: "deactivate-tenant-visible".into(),
                    request_digest: format!("sha256:{}", "3".repeat(64)),
                    terminal_audit: None,
                },
                LibraryMutation::Deactivate {
                    artifact_id: tenant_visible.member_id.clone(),
                    updated_at: timestamp.clone(),
                },
                timestamp,
            )
            .unwrap();
        let members = provider.members(&manager, &context).await.unwrap();
        assert!(!members.contains(&tenant_visible));
        manager
            .publish_execution_capability_snapshots(vec![CapabilityCatalogSnapshot {
                generation: "after-deactivation".into(),
                principal: context.principal.as_str().into(),
                members,
            }])
            .unwrap();
        assert_eq!(
            manager
                .execution_loadout_preview(&context, "tenant-skill", "runtime")
                .await
                .unwrap()
                .resolved[0]
                .status,
            ResolutionStatus::Missing
        );
    }
}
