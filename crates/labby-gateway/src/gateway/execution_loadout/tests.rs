use super::*;
use crate::gateway::manager::GatewayRuntimeHandle;

fn manager() -> GatewayManager {
    let path =
        std::env::temp_dir().join(format!("execution-loadout-{}.toml", uuid::Uuid::new_v4()));
    GatewayManager::new(path, GatewayRuntimeHandle::default())
}

fn member(family: CapabilityFamily) -> CapabilityRef {
    CapabilityRef {
        provider: "provider-1".into(),
        family,
        member_id: format!("member-{family:?}"),
        expected_revision: "revision-1".into(),
    }
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
        .execution_loadout_create(ExecutionLoadoutCreate {
            id: "universal".into(),
            name: "Universal".into(),
            description: None,
            members: members.clone(),
        })
        .await
        .expect("create universal draft");
    members.sort();
    assert_eq!(created.members, members);

    let duplicate = vec![
        member(CapabilityFamily::Tool),
        member(CapabilityFamily::Tool),
    ];
    let error = manager
        .execution_loadout_create(ExecutionLoadoutCreate {
            id: "duplicate".into(),
            name: "Duplicate".into(),
            description: None,
            members: duplicate,
        })
        .await
        .expect_err("duplicates fail closed");
    assert!(matches!(error, ExecutionLoadoutError::Invalid { .. }));
}

#[tokio::test]
async fn stale_patch_returns_current_revision_and_mergeable_fields() {
    let manager = manager();
    manager
        .execution_loadout_create(ExecutionLoadoutCreate {
            id: "cas".into(),
            name: "CAS".into(),
            description: None,
            members: Vec::new(),
        })
        .await
        .expect("create");
    manager
        .execution_loadout_patch(
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
async fn preview_is_principal_runtime_bound_and_unsupported_is_explicit() {
    let manager = manager();
    manager
        .execution_loadout_create(ExecutionLoadoutCreate {
            id: "preview".into(),
            name: "Preview".into(),
            description: None,
            members: vec![member(CapabilityFamily::Agent)],
        })
        .await
        .expect("create");
    let caller = PaletteCaller::admin(Some("principal-1"), Some("request-1"));
    let preview = manager
        .execution_loadout_preview(&caller, "preview", "runtime-1")
        .await
        .expect("preview");
    assert_eq!(preview.principal, "principal-1");
    assert_eq!(preview.runtime_identity, "runtime-1");
    assert_eq!(preview.resolved[0].status, ResolutionStatus::Unsupported);
    assert!(preview.effective.is_empty());
}

#[tokio::test]
async fn activation_creates_immutable_revision_and_rollback_revises_draft() {
    let manager = manager();
    manager
        .execution_loadout_create(ExecutionLoadoutCreate {
            id: "lifecycle".into(),
            name: "Lifecycle".into(),
            description: None,
            members: Vec::new(),
        })
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
        .execution_loadout_rollback("lifecycle", revised.draft_revision, 1)
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
        .execution_loadout_create(ExecutionLoadoutCreate {
            id: "durable".into(),
            name: "Durable".into(),
            description: None,
            members: Vec::new(),
        })
        .await
        .expect("persist draft");
    drop(first);
    let restarted = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    assert_eq!(
        restarted
            .execution_loadout_get("durable")
            .await
            .expect("reload")
            .draft_revision,
        1
    );
    drop(std::fs::remove_file(
        path.with_extension("execution-loadouts.json"),
    ));
}
