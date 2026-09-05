use super::*;
use crate::gateway::manager::GatewayRuntimeHandle;

fn manager() -> GatewayManager {
    let directory =
        std::env::temp_dir().join(format!("execution-loadout-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).expect("create isolated loadout test directory");
    let path = directory.join("labby.toml");
    GatewayManager::new(path, GatewayRuntimeHandle::default())
}

fn caller() -> ExecutionLoadoutContext {
    ExecutionLoadoutContext {
        principal: ExecutionPrincipal::new("principal-1").unwrap(),
        allowed_providers: None,
    }
}

fn member(family: CapabilityFamily) -> CapabilityRef {
    CapabilityRef {
        provider: "provider-1".into(),
        family,
        member_id: format!("member-{family:?}"),
        expected_revision: format!("revision-{family:?}"),
    }
}

fn publish(manager: &GatewayManager, principal: &str, members: Vec<CapabilityRef>) {
    manager
        .publish_execution_capability_snapshots(vec![CapabilityCatalogSnapshot {
            generation: "catalog-generation-1".into(),
            principal: principal.into(),
            members,
        }])
        .expect("publish authoritative catalog");
}

#[tokio::test]
async fn normalizes_every_family_deterministically_and_rejects_duplicate_refs() {
    let manager = manager();
    let families = [
        CapabilityFamily::Tool,
        CapabilityFamily::Prompt,
        CapabilityFamily::Resource,
        CapabilityFamily::Skill,
        CapabilityFamily::Agent,
        CapabilityFamily::Hook,
        CapabilityFamily::McpApp,
        CapabilityFamily::McpServer,
        CapabilityFamily::Plugin,
    ];
    let mut members = families.into_iter().rev().map(member).collect::<Vec<_>>();
    let created = manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "universal".into(),
                name: "Universal".into(),
                description: None,
                members: members.clone(),
            },
        )
        .await
        .expect("create universal draft");
    members.sort();
    assert_eq!(created.members, members);

    let duplicate = vec![
        member(CapabilityFamily::Tool),
        member(CapabilityFamily::Tool),
    ];
    let error = manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "duplicate".into(),
                name: "Duplicate".into(),
                description: None,
                members: duplicate,
            },
        )
        .await
        .expect_err("duplicates fail closed");
    assert!(matches!(error, ExecutionLoadoutError::Invalid { .. }));
}

struct ContextCatalogProvider;

impl ExecutionCapabilityCatalogProvider for ContextCatalogProvider {
    fn members<'a>(
        &'a self,
        _manager: &'a GatewayManager,
        context: &'a ExecutionLoadoutContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CapabilityRef>, ExecutionLoadoutError>> + Send + 'a>>
    {
        Box::pin(async move {
            Ok(vec![CapabilityRef {
                provider: "labby".into(),
                family: CapabilityFamily::Prompt,
                member_id: context.principal.as_str().to_owned(),
                expected_revision: "revision-1".into(),
            }])
        })
    }
}

#[tokio::test]
async fn provider_refresh_publishes_explicit_principals_in_one_generation() {
    let mut manager = manager();
    manager.execution_capability_provider = Some(Arc::new(ContextCatalogProvider));
    let contexts = [
        ExecutionLoadoutContext {
            principal: ExecutionPrincipal::new("principal-2").unwrap(),
            allowed_providers: None,
        },
        caller(),
    ];

    manager
        .refresh_execution_capability_snapshots(&contexts)
        .await
        .unwrap();

    let published = manager.execution_capabilities.load_full();
    assert_eq!(published.snapshots.len(), 2);
    let generations = published
        .snapshots
        .values()
        .map(|snapshot| snapshot.generation.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(generations.len(), 1);
    assert_eq!(
        published
            .snapshots
            .get(&ExecutionPrincipal::new("principal-1").unwrap())
            .unwrap()
            .members[0]
            .member_id,
        "principal-1"
    );
}

#[tokio::test]
async fn stale_patch_returns_current_revision_and_mergeable_fields() {
    let manager = manager();
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "cas".into(),
                name: "CAS".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .expect("create");
    manager
        .execution_loadout_patch(
            &caller(),
            "cas",
            ExecutionLoadoutPatch {
                expected_draft_revision: 1,
                name: Some("First".into()),
                description: None,
                members: None,
            },
        )
        .await
        .expect("first writer");
    let error = manager
        .execution_loadout_patch(
            &caller(),
            "cas",
            ExecutionLoadoutPatch {
                expected_draft_revision: 1,
                name: Some("Second".into()),
                description: None,
                members: None,
            },
        )
        .await
        .expect_err("stale writer rejected");
    assert!(matches!(
        error,
        ExecutionLoadoutError::StaleRevision {
            expected: 1,
            current: 2,
            ..
        }
    ));
}

#[tokio::test]
async fn preview_is_principal_runtime_bound_and_all_families_are_effective() {
    let manager = manager();
    publish(
        &manager,
        "principal-1",
        vec![member(CapabilityFamily::Agent)],
    );
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "preview".into(),
                name: "Preview".into(),
                description: None,
                members: vec![member(CapabilityFamily::Agent)],
            },
        )
        .await
        .expect("create");
    let caller = caller();
    let preview = manager
        .execution_loadout_preview(&caller, "preview", "runtime-1")
        .await
        .expect("preview");
    assert_eq!(preview.principal, "principal-1");
    assert_eq!(preview.runtime_identity, "runtime-1");
    assert_eq!(preview.resolved[0].status, ResolutionStatus::Effective);
    assert_eq!(preview.effective.len(), 1);
}

#[tokio::test]
async fn mixed_family_activation_is_atomic_and_cross_principal_access_is_private() {
    let manager = manager();
    let owner = caller();
    let families = [
        CapabilityFamily::Prompt,
        CapabilityFamily::Resource,
        CapabilityFamily::Skill,
        CapabilityFamily::Agent,
        CapabilityFamily::McpApp,
        CapabilityFamily::McpServer,
        CapabilityFamily::Plugin,
    ];
    let members = families.into_iter().map(member).collect::<Vec<_>>();
    publish(&manager, "principal-1", members.clone());
    manager
        .execution_loadout_create(
            &owner,
            ExecutionLoadoutCreate {
                id: "mixed".into(),
                name: "Mixed".into(),
                description: None,
                members,
            },
        )
        .await
        .expect("create mixed loadout");
    let activation = manager
        .execution_loadout_activate(&owner, "mixed", 1, "axon-service")
        .await
        .expect("activate all advertised families");
    assert_eq!(activation.preview.effective.len(), 7);

    let other = ExecutionLoadoutContext {
        principal: ExecutionPrincipal::new("principal-2").unwrap(),
        allowed_providers: None,
    };
    assert!(matches!(
        manager.execution_loadout_get(&other, "mixed").await,
        Err(ExecutionLoadoutError::NotFound { .. })
    ));
    assert!(manager.execution_loadout_list(&other).await.is_empty());
}

#[tokio::test]
async fn non_tool_activation_rejects_missing_stale_and_unpublished_principal_members() {
    let manager = manager();
    let owner = caller();
    let authoritative = member(CapabilityFamily::Skill);
    publish(&manager, "principal-1", vec![authoritative.clone()]);
    for (id, selected) in [
        (
            "missing",
            CapabilityRef {
                member_id: "forged-skill".into(),
                ..authoritative.clone()
            },
        ),
        (
            "stale",
            CapabilityRef {
                expected_revision: "attacker-self-attested-revision".into(),
                ..authoritative.clone()
            },
        ),
    ] {
        manager
            .execution_loadout_create(
                &owner,
                ExecutionLoadoutCreate {
                    id: id.into(),
                    name: id.into(),
                    description: None,
                    members: vec![selected],
                },
            )
            .await
            .unwrap();
        assert!(
            manager
                .execution_loadout_activate(&owner, id, 1, "runtime")
                .await
                .is_err()
        );
    }

    manager
        .publish_execution_capability_snapshots(Vec::new())
        .unwrap();
    manager
        .execution_loadout_create(
            &owner,
            ExecutionLoadoutCreate {
                id: "unpublished".into(),
                name: "unpublished".into(),
                description: None,
                members: vec![authoritative],
            },
        )
        .await
        .unwrap();
    assert!(
        manager
            .execution_loadout_activate(&owner, "unpublished", 1, "runtime")
            .await
            .is_err()
    );
    assert!(
        manager
            .publish_execution_capability_snapshots(vec![
                CapabilityCatalogSnapshot {
                    generation: "same".into(),
                    principal: "principal-1".into(),
                    members: Vec::new(),
                },
                CapabilityCatalogSnapshot {
                    generation: "same".into(),
                    principal: "principal-1".into(),
                    members: Vec::new(),
                },
            ])
            .is_err()
    );
}

#[tokio::test]
async fn activation_creates_immutable_revision_and_rollback_revises_draft() {
    let manager = manager();
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "lifecycle".into(),
                name: "Lifecycle".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .expect("create");
    let caller = caller();
    let activation = manager
        .execution_loadout_activate(&caller, "lifecycle", 1, "runtime-1")
        .await
        .expect("activate empty authorized selection");
    assert_eq!(activation.revision.revision, 1);
    assert_eq!(activation.preview.active_revision, 0);
    assert_eq!(activation.loadout.desired_active_revision, Some(1));
    assert_eq!(activation.loadout.effective_runtime_revision, Some(1));
    assert!(!activation.loadout.restart_required);
    let active_preview = manager
        .execution_loadout_preview(&caller, "lifecycle", "runtime-1")
        .await
        .expect("preview active revision");
    assert_eq!(active_preview.active_revision, 1);

    let revised = manager
        .execution_loadout_patch(
            &caller,
            "lifecycle",
            ExecutionLoadoutPatch {
                expected_draft_revision: 1,
                name: None,
                description: None,
                members: Some(vec![member(CapabilityFamily::Plugin)]),
            },
        )
        .await
        .expect("revise draft");
    let rolled_back = manager
        .execution_loadout_rollback(&caller, "lifecycle", revised.draft_revision, 1)
        .await
        .expect("rollback from immutable revision");
    assert!(rolled_back.members.is_empty());
    assert_eq!(rolled_back.draft_revision, 3);
}

#[tokio::test]
async fn revisions_survive_manager_restart_in_separate_atomic_store() {
    let directory = tempfile::tempdir().expect("temporary loadout store");
    let path = directory.path().join("labby.toml");
    let first = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    first
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "durable".into(),
                name: "Durable".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .expect("persist draft");
    drop(first);
    let restarted = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    assert_eq!(
        restarted
            .execution_loadout_get(&caller(), "durable")
            .await
            .expect("reload")
            .draft_revision,
        1
    );
}

#[tokio::test]
async fn persistence_failures_never_publish_candidate_state() {
    let manager = manager();
    manager.fail_next_execution_loadout_persist();
    assert!(
        manager
            .execution_loadout_create(
                &caller(),
                ExecutionLoadoutCreate {
                    id: "durable-first".into(),
                    name: "Draft".into(),
                    description: None,
                    members: Vec::new(),
                }
            )
            .await
            .is_err()
    );
    assert!(
        manager
            .execution_loadout_get(&caller(), "durable-first")
            .await
            .is_err()
    );

    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "durable-first".into(),
                name: "Draft".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .unwrap();
    manager.fail_next_execution_loadout_persist();
    assert!(
        manager
            .execution_loadout_patch(
                &caller(),
                "durable-first",
                ExecutionLoadoutPatch {
                    expected_draft_revision: 1,
                    name: Some("Changed".into()),
                    description: None,
                    members: None,
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        manager
            .execution_loadout_get(&caller(), "durable-first")
            .await
            .unwrap()
            .name,
        "Draft"
    );

    manager.fail_next_execution_loadout_persist();
    assert!(
        manager
            .execution_loadout_activate(&caller(), "durable-first", 1, "runtime")
            .await
            .is_err()
    );
    assert_eq!(
        manager
            .execution_loadout_get(&caller(), "durable-first")
            .await
            .unwrap()
            .effective_runtime_revision,
        None
    );

    manager
        .execution_loadout_activate(&caller(), "durable-first", 1, "runtime")
        .await
        .unwrap();
    manager
        .execution_loadout_patch(
            &caller(),
            "durable-first",
            ExecutionLoadoutPatch {
                expected_draft_revision: 1,
                name: None,
                description: None,
                members: Some(vec![member(CapabilityFamily::Plugin)]),
            },
        )
        .await
        .unwrap();
    manager.fail_next_execution_loadout_persist();
    assert!(
        manager
            .execution_loadout_rollback(&caller(), "durable-first", 2, 1)
            .await
            .is_err()
    );
    assert_eq!(
        manager
            .execution_loadout_get(&caller(), "durable-first")
            .await
            .unwrap()
            .members
            .len(),
        1
    );
}

#[tokio::test]
async fn post_commit_parent_sync_failure_keeps_memory_and_disk_coherent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("labby.toml");
    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager.fail_next_execution_loadout_parent_sync();
    let error = manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "published".into(),
                name: "Published".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ExecutionLoadoutError::Durability { .. }));
    assert_eq!(
        manager
            .execution_loadout_get(&caller(), "published")
            .await
            .unwrap()
            .name,
        "Published"
    );
    let restarted = GatewayManager::new(path, GatewayRuntimeHandle::default());
    assert_eq!(
        restarted
            .execution_loadout_get(&caller(), "published")
            .await
            .unwrap()
            .name,
        "Published"
    );
}

#[tokio::test]
async fn activation_retry_and_concurrent_callers_share_one_revision() {
    let manager = manager();
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "activate-once".into(),
                name: "Once".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .unwrap();
    let first_caller = caller();
    let second_caller = caller();
    let (first, second) = tokio::join!(
        manager.execution_loadout_activate(&first_caller, "activate-once", 1, "runtime"),
        manager.execution_loadout_activate(&second_caller, "activate-once", 1, "runtime"),
    );
    assert_eq!(first.unwrap().revision.revision, 1);
    assert_eq!(second.unwrap().revision.revision, 1);
    let retry = manager
        .execution_loadout_activate(&caller(), "activate-once", 1, "runtime")
        .await
        .unwrap();
    assert_eq!(retry.revision.revision, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revocation_publication_waits_for_the_durable_activation_commit() {
    let manager = manager();
    let selected = member(CapabilityFamily::Agent);
    publish(&manager, "principal-1", vec![selected.clone()]);
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "publication-race".into(),
                name: "Race".into(),
                description: None,
                members: vec![selected],
            },
        )
        .await
        .unwrap();
    let entered = Arc::new(std::sync::Barrier::new(2));
    let resume = Arc::new(std::sync::Barrier::new(2));
    manager.set_execution_loadout_activation_hook(Arc::clone(&entered), Arc::clone(&resume));
    let activating = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .execution_loadout_activate(&caller(), "publication-race", 1, "runtime")
                .await
        })
    };
    entered.wait();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let revoking = {
        let manager = manager.clone();
        tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            manager.publish_execution_capability_snapshots(vec![CapabilityCatalogSnapshot {
                generation: "catalog-generation-2".into(),
                principal: "principal-1".into(),
                members: Vec::new(),
            }])
        })
    };
    started_rx.recv().unwrap();
    resume.wait();
    assert_eq!(activating.await.unwrap().unwrap().revision.revision, 1);
    revoking.await.unwrap().unwrap();
    assert!(
        manager
            .execution_loadout_preview(&caller(), "publication-race", "runtime")
            .await
            .unwrap()
            .effective
            .is_empty()
    );
}

#[tokio::test]
async fn preview_rejects_a_runtime_other_than_the_active_binding() {
    let manager = manager();
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "bound".into(),
                name: "Bound".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .unwrap();
    manager
        .execution_loadout_activate(&caller(), "bound", 1, "runtime-a")
        .await
        .unwrap();
    assert!(
        manager
            .execution_loadout_preview(&caller(), "bound", "runtime-b")
            .await
            .is_err()
    );
}

#[test]
fn rejects_ambiguous_identity_and_mixed_publication_generations() {
    assert!(ExecutionPrincipal::new("shared").is_err());
    assert!(ExecutionPrincipal::new("bad\0principal").is_err());
    assert!(RecordKey::new(&ExecutionPrincipal::new("principal").unwrap(), "bad\0id").is_err());
    let manager = manager();
    assert!(
        manager
            .publish_execution_capability_snapshots(vec![
                CapabilityCatalogSnapshot {
                    generation: "one".into(),
                    principal: "principal-1".into(),
                    members: Vec::new()
                },
                CapabilityCatalogSnapshot {
                    generation: "two".into(),
                    principal: "principal-2".into(),
                    members: Vec::new()
                },
            ])
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_managers_serialize_against_the_durable_store() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("labby.toml");
    let first = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    let second = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    let first_caller = caller();
    let second_caller = caller();
    let (a, b) = tokio::join!(
        first.execution_loadout_create(
            &first_caller,
            ExecutionLoadoutCreate {
                id: "a".into(),
                name: "A".into(),
                description: None,
                members: Vec::new(),
            }
        ),
        second.execution_loadout_create(
            &second_caller,
            ExecutionLoadoutCreate {
                id: "b".into(),
                name: "B".into(),
                description: None,
                members: Vec::new(),
            }
        ),
    );
    a.unwrap();
    b.unwrap();
    let restarted = GatewayManager::new(path, GatewayRuntimeHandle::default());
    assert_eq!(restarted.execution_loadout_list(&caller()).await.len(), 2);
}
