use super::*;
use crate::gateway::manager::GatewayRuntimeHandle;

fn manager() -> GatewayManager {
    let path =
        std::env::temp_dir().join(format!("execution-loadout-{}.toml", uuid::Uuid::new_v4()));
    GatewayManager::new(path, GatewayRuntimeHandle::default())
}

fn caller() -> PaletteCaller {
    PaletteCaller::admin(Some("principal-1"), Some("request-1"))
}

fn member(family: CapabilityFamily) -> CapabilityRef {
    let mut capability = CapabilityRef {
        provider: "provider-1".into(),
        family,
        member_id: format!("member-{family:?}"),
        expected_revision: String::new(),
    };
    capability.expected_revision = capability_revision(&capability);
    capability
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
    let caller = PaletteCaller::admin(Some("principal-1"), Some("request-1"));
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
    manager
        .execution_loadout_create(
            &owner,
            ExecutionLoadoutCreate {
                id: "mixed".into(),
                name: "Mixed".into(),
                description: None,
                members: families.into_iter().map(member).collect(),
            },
        )
        .await
        .expect("create mixed loadout");
    let activation = manager
        .execution_loadout_activate(&owner, "mixed", 1, "axon-service")
        .await
        .expect("activate all advertised families");
    assert_eq!(activation.preview.effective.len(), 7);

    let other = PaletteCaller::admin(Some("principal-2"), Some("request-2"));
    assert!(matches!(
        manager.execution_loadout_get(&other, "mixed").await,
        Err(ExecutionLoadoutError::NotFound { .. })
    ));
    assert!(manager.execution_loadout_list(&other).await.is_empty());
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
    let caller = PaletteCaller::admin(Some("principal-1"), Some("request-1"));
    let activation = manager
        .execution_loadout_activate(&caller, "lifecycle", 1, "runtime-1")
        .await
        .expect("activate empty authorized selection");
    assert_eq!(activation.revision.revision, 1);
    assert_eq!(activation.loadout.desired_active_revision, Some(1));
    assert_eq!(activation.loadout.effective_runtime_revision, Some(1));
    assert!(!activation.loadout.restart_required);

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
    let path =
        std::env::temp_dir().join(format!("execution-loadout-{}.toml", uuid::Uuid::new_v4()));
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
    drop(std::fs::remove_file(
        path.with_extension("execution-loadouts.json"),
    ));
}
