use super::*;
use crate::gateway::manager::GatewayRuntimeHandle;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

fn manager() -> GatewayManager {
    let directory =
        std::env::temp_dir().join(format!("execution-loadout-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).expect("create isolated loadout test directory");
    let path = directory.join("labby.toml");
    GatewayManager::new(path, GatewayRuntimeHandle::default())
}

fn caller() -> PaletteCaller {
    PaletteCaller::admin(Some("principal-1"), Some("request-1"))
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

struct CountingCatalogProvider {
    calls: Arc<AtomicUsize>,
}

struct RecordingCatalogProvider {
    allowed: Arc<Mutex<Vec<Option<BTreeSet<String>>>>>,
    requested: Arc<Mutex<Vec<Option<Vec<CapabilityRef>>>>>,
    member: CapabilityRef,
}

impl ExecutionCapabilityCatalogProvider for RecordingCatalogProvider {
    fn snapshot<'a>(
        &'a self,
        _manager: &'a GatewayManager,
        principal: &'a str,
        _tenant: &'a str,
        allowed_upstreams: Option<&'a BTreeSet<String>>,
        requested_members: Option<&'a [CapabilityRef]>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<CapabilityCatalogSnapshot, ExecutionLoadoutError>>
                + Send
                + 'a,
        >,
    > {
        self.allowed
            .lock()
            .unwrap()
            .push(allowed_upstreams.cloned());
        self.requested
            .lock()
            .unwrap()
            .push(requested_members.map(<[CapabilityRef]>::to_vec));
        Box::pin(async move {
            Ok(CapabilityCatalogSnapshot {
                generation: "selected-generation".into(),
                principal: principal.into(),
                members: vec![self.member.clone()],
            })
        })
    }
}

impl ExecutionCapabilityCatalogProvider for CountingCatalogProvider {
    fn snapshot<'a>(
        &'a self,
        _manager: &'a GatewayManager,
        _principal: &'a str,
        _tenant: &'a str,
        _allowed_upstreams: Option<&'a BTreeSet<String>>,
        _requested_members: Option<&'a [CapabilityRef]>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<CapabilityCatalogSnapshot, ExecutionLoadoutError>>
                + Send
                + 'a,
        >,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Err(ExecutionLoadoutError::Storage {
                message: "discovery must not run".into(),
            })
        })
    }
}

#[tokio::test]
async fn empty_preview_and_activation_never_discover_capabilities() {
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = manager().with_execution_capability_provider(Arc::new(CountingCatalogProvider {
        calls: Arc::clone(&calls),
    }));
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "empty".into(),
                name: "Empty".into(),
                description: None,
                members: Vec::new(),
            },
        )
        .await
        .expect("create empty loadout");

    let preview = manager
        .execution_loadout_preview(&caller(), "empty", "runtime-1")
        .await
        .expect("empty preview must not wait for discovery");
    assert!(preview.resolved.is_empty());
    assert!(preview.effective.is_empty());

    let activation = manager
        .execution_loadout_activate(&caller(), "empty", 1, "runtime-1")
        .await
        .expect("empty activation must not wait for discovery");
    assert!(activation.preview.resolved.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let active_preview = manager
        .execution_loadout_preview(&caller(), "empty", "runtime-1")
        .await
        .expect("active preview");
    assert_eq!(active_preview.active_revision, 1);
    assert_eq!(
        serde_json::to_value(&active_preview).expect("serialize preview")["activeRevision"],
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn nonempty_preview_constrains_discovery_to_selected_providers() {
    let selected = member(CapabilityFamily::Prompt);
    let allowed = Arc::new(Mutex::new(Vec::new()));
    let requested = Arc::new(Mutex::new(Vec::new()));
    let manager =
        manager().with_execution_capability_provider(Arc::new(RecordingCatalogProvider {
            allowed: Arc::clone(&allowed),
            requested: Arc::clone(&requested),
            member: selected.clone(),
        }));
    manager
        .execution_loadout_create(
            &caller(),
            ExecutionLoadoutCreate {
                id: "selected".into(),
                name: "Selected".into(),
                description: None,
                members: vec![selected],
            },
        )
        .await
        .expect("create selected loadout");

    manager
        .execution_loadout_preview(&caller(), "selected", "runtime-1")
        .await
        .expect("resolve selected provider");

    assert_eq!(
        allowed.lock().unwrap().as_slice(),
        &[Some(BTreeSet::from(["provider-1".to_string()]))]
    );
    assert_eq!(
        requested.lock().unwrap().as_slice(),
        &[Some(vec![member(CapabilityFamily::Prompt)])]
    );
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

    let other = PaletteCaller::admin(Some("principal-2"), Some("request-2"));
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
