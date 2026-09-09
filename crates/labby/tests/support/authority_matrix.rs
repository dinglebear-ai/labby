use std::collections::BTreeSet;

use super::action_matrix::CatalogAction;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum OwnerKind {
    Installation,
    Team,
    Project,
    Personal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResourceFamily {
    Platform,
    Library,
    Project,
    Gateway,
    Stash,
    Agent,
    Task,
    DevContainer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationClass {
    Discover,
    Read,
    Operate,
    Administer,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityClassification {
    pub(crate) resource: ResourceFamily,
    pub(crate) operation: OperationClass,
    pub(crate) owners: &'static [OwnerKind],
    pub(crate) delegated: bool,
    pub(crate) final_boundary_reauthorization: bool,
}

const INSTALLATION: &[OwnerKind] = &[OwnerKind::Installation];
const USER_OWNED: &[OwnerKind] = &[OwnerKind::Team, OwnerKind::Project, OwnerKind::Personal];
const GATEWAY_OWNED: &[OwnerKind] = &[
    OwnerKind::Installation,
    OwnerKind::Team,
    OwnerKind::Project,
    OwnerKind::Personal,
];
/// Installation-scoped gateway administration (`platform.manage`) is owned by
/// the installation alone even though the gateway family also has user-owned
/// Loadout/protected-route resources.
const INSTALLATION_GATEWAY: &[OwnerKind] = &[OwnerKind::Installation];
/// Managed Projects are Team-owned in the v1 matrix (`resourceFamilies.project`).
const TEAM_OWNED: &[OwnerKind] = &[OwnerKind::Team];
const PROJECT_OWNED: &[OwnerKind] = &[OwnerKind::Project];

/// Identity of one registered action: the `service:action` pair that the
/// generated catalog, the intent fixtures, and the authority expectations
/// fixture all key on.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ActionRef {
    pub(crate) service: String,
    pub(crate) action: String,
}

impl ActionRef {
    pub(crate) fn new(service: &str, action: &str) -> Self {
        Self {
            service: service.to_owned(),
            action: action.to_owned(),
        }
    }

    pub(crate) fn key(&self) -> String {
        format!("{}:{}", self.service, self.action)
    }
}

impl From<&CatalogAction> for ActionRef {
    fn from(action: &CatalogAction) -> Self {
        Self::new(&action.service, &action.action)
    }
}

/// Operation class implied by the exact capability the shared evaluator
/// demands. `None` means the action has no capability-gated evaluator step
/// (builtin probes and caller-membership projections).
pub(crate) fn operation_for_capability(capability: Option<&str>) -> Option<OperationClass> {
    Some(match capability? {
        "platform.read" | "scope.read" => OperationClass::Read,
        "scope.use" | "scope.operate" | "scope.create" => OperationClass::Operate,
        "platform.manage" | "scope.manage" | "membership.manage" | "ownership.transfer"
        | "policy.explain" | "audit.read" => OperationClass::Administer,
        "scope.delete" => OperationClass::Delete,
        _ => return None,
    })
}

/// Classifies one registered action by the authority it must enforce.
///
/// The resource family and owner kinds are a per-`ActionRef` policy. The
/// operation class is derived from the catalog's `required_capability`
/// column (the evaluator vocabulary), never from `requires_admin` or
/// `destructive`: those are separate surface-policy axes and are compared
/// against the derived class by the completeness tests instead of feeding it.
///
/// Services that have not yet moved onto the capability evaluator
/// (`authorization_boundary` of `transport`/`transport_admin`) have no
/// capability; their class falls back to the transport ceiling
/// (`Administer` for `transport_admin`, `Read` otherwise) and is labelled as
/// such through `delegated`/`resource` rather than pretending to be exact.
pub(crate) fn classify_labby(action: &CatalogAction) -> Option<AuthorityClassification> {
    let action_ref = ActionRef::from(action);
    let (resource, owners, delegated) = match action_ref.service.as_str() {
        "access" if action_ref.action.starts_with("access.platform_admin.") => {
            (ResourceFamily::Platform, INSTALLATION, false)
        }
        "access" if action_ref.action == "access.team.create" => {
            (ResourceFamily::Platform, INSTALLATION, false)
        }
        "access" => (ResourceFamily::Project, USER_OWNED, false),
        "agents" => (ResourceFamily::Agent, USER_OWNED, false),
        "depot_publish" => (ResourceFamily::Library, PROJECT_OWNED, true),
        "artifacts" | "bundles" | "sources" | "uploads" => {
            (ResourceFamily::Library, USER_OWNED, true)
        }
        "gateway" if action.required_capability.as_deref() == Some("platform.manage") => {
            (ResourceFamily::Gateway, INSTALLATION_GATEWAY, false)
        }
        "gateway" => (ResourceFamily::Gateway, GATEWAY_OWNED, true),
        "browser" | "snippets" => (ResourceFamily::Gateway, USER_OWNED, true),
        "dev_containers" => (ResourceFamily::DevContainer, USER_OWNED, false),
        "jobs" => (ResourceFamily::Task, USER_OWNED, true),
        "projects" => (ResourceFamily::Project, TEAM_OWNED, false),
        "stash" => (ResourceFamily::Stash, USER_OWNED, false),
        "tasks" => (ResourceFamily::Task, USER_OWNED, false),
        "doctor" | "fs" | "lab_admin" | "server_logs" | "setup" => {
            (ResourceFamily::Platform, INSTALLATION, false)
        }
        _ => return None,
    };
    let operation = if action.builtin {
        OperationClass::Discover
    } else if let Some(operation) = operation_for_capability(action.required_capability.as_deref())
    {
        operation
    } else {
        match action.authorization_boundary.as_str() {
            // Caller-membership projections: visibility is filtered inside
            // the store, so the evaluator gate is a read.
            "caller_membership_projection" | "team_project_membership" => OperationClass::Read,
            // Transport-ceiling services (not yet on the capability evaluator).
            "project_artifact_publish" => OperationClass::Operate,
            "transport_admin" => OperationClass::Administer,
            "transport" => OperationClass::Read,
            _ => return None,
        }
    };
    Some(AuthorityClassification {
        resource,
        operation,
        owners,
        delegated,
        // Only builtin probes carry no reauthorization at the final boundary;
        // every other action reaches the evaluator (capability) or the
        // transport ceiling again inside dispatch.
        final_boundary_reauthorization: !action.builtin,
    })
}

/// Golden Depot control-plane operation registry. `DEPOT_OPERATIONS` is a
/// reviewed authority snapshot of the same operation set; the completeness
/// tests assert the two name sets are identical so the hand list cannot drift.
pub(crate) const DEPOT_OPERATIONS_FIXTURE: &str =
    include_str!("../../../../docs/contracts/fixtures/depot-control-plane/operations-v1.json");

pub(crate) fn depot_fixture_operation_names() -> BTreeSet<String> {
    let fixture: serde_json::Value =
        serde_json::from_str(DEPOT_OPERATIONS_FIXTURE).expect("depot operations fixture parses");
    fixture["operations"]
        .as_array()
        .expect("depot fixture has an operations array")
        .iter()
        .map(|operation| {
            operation["name"]
                .as_str()
                .expect("depot operation has a name")
                .to_owned()
        })
        .collect()
}

pub(crate) fn depot_snapshot_operation_names() -> BTreeSet<String> {
    DEPOT_OPERATIONS
        .iter()
        .map(|operation| operation.name.to_owned())
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DepotOperation {
    pub(crate) name: &'static str,
    pub(crate) resource: ResourceFamily,
    pub(crate) operation: OperationClass,
    pub(crate) owners: &'static [OwnerKind],
    pub(crate) delegated: bool,
}

macro_rules! depot {
    ($name:literal, $resource:ident, $operation:ident, $owners:expr, $delegated:expr) => {
        DepotOperation {
            name: $name,
            resource: ResourceFamily::$resource,
            operation: OperationClass::$operation,
            owners: $owners,
            delegated: $delegated,
        }
    };
}

/// Snapshot of Depot's cross-product operation registry. This is intentionally
/// explicit: adding a Depot operation requires choosing its resource family,
/// operation class, owner kinds, and delegated-call posture here as well as in
/// Depot's own registry completeness test.
pub(crate) const DEPOT_OPERATIONS: &[DepotOperation] = &[
    depot!("depot.acp_registry.list", Library, Read, USER_OWNED, true),
    depot!("depot.artifacts.exact", Library, Read, USER_OWNED, true),
    depot!("depot.artifacts.follow", Library, Operate, USER_OWNED, true),
    depot!("depot.artifacts.fork", Library, Operate, USER_OWNED, true),
    depot!("depot.artifacts.get", Library, Read, USER_OWNED, true),
    depot!(
        "depot.artifacts.intake_candidate",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.artifacts.list", Library, Read, USER_OWNED, true),
    depot!(
        "depot.artifacts.list_candidates",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.artifacts.set_license",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.artifacts.set_publication",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.add_skill",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.create",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.bundles.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.bundles.get", Library, Read, USER_OWNED, true),
    depot!("depot.bundles.list", Library, Read, USER_OWNED, true),
    depot!(
        "depot.bundles.publish",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.remove_skill",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.bundles.set_visibility",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.ingest.cancel", Task, Operate, USER_OWNED, true),
    depot!("depot.ingest.get", Task, Read, USER_OWNED, true),
    depot!("depot.ingest.list", Task, Read, USER_OWNED, true),
    depot!("depot.ingest.retry", Task, Operate, USER_OWNED, true),
    depot!("depot.ingest.start", Task, Operate, USER_OWNED, true),
    depot!(
        "depot.maintenance.cas_audit",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.audit",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.copy",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.cutover",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.end_retention",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.plan",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.rollback",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.status",
        Platform,
        Read,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.cas_migration.verify",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.gc",
        Platform,
        Delete,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.sidecars",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!(
        "depot.maintenance.upstream",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!("depot.mcp_registry.list", Library, Read, USER_OWNED, true),
    depot!("depot.skills.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.skills.get", Library, Read, USER_OWNED, true),
    depot!(
        "depot.skills.ingest_acp_registry",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_ard_catalog",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_marketplace",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_mcp",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_mcp_registry",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_repo",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_skills_sh",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.ingest_well_known",
        Library,
        Operate,
        USER_OWNED,
        true
    ),
    depot!("depot.skills.list", Library, Read, USER_OWNED, true),
    depot!("depot.skills.load", Library, Read, USER_OWNED, true),
    depot!("depot.skills.read", Library, Read, USER_OWNED, true),
    depot!("depot.skills.search", Library, Read, USER_OWNED, true),
    depot!("depot.skills.search_ard", Library, Read, USER_OWNED, true),
    depot!(
        "depot.skills.search_marketplace",
        Library,
        Read,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.skills.search_skills_sh",
        Library,
        Read,
        USER_OWNED,
        true
    ),
    depot!(
        "depot.sources.configure",
        Library,
        Administer,
        USER_OWNED,
        true
    ),
    depot!("depot.sources.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.sources.list", Library, Read, USER_OWNED, true),
    depot!("depot.sources.refresh", Library, Operate, USER_OWNED, true),
    depot!("depot.system.status", Platform, Read, INSTALLATION, false),
    depot!(
        "depot.tokens.create",
        Platform,
        Administer,
        INSTALLATION,
        false
    ),
    depot!("depot.tokens.list", Platform, Read, INSTALLATION, false),
    depot!("depot.tokens.revoke", Platform, Delete, INSTALLATION, false),
    depot!("depot.uploads.create", Library, Operate, USER_OWNED, true),
    depot!("depot.uploads.delete", Library, Delete, USER_OWNED, true),
    depot!("depot.uploads.get", Library, Read, USER_OWNED, true),
];

pub(crate) fn duplicate_depot_operations() -> BTreeSet<&'static str> {
    let mut seen = BTreeSet::new();
    DEPOT_OPERATIONS
        .iter()
        .filter_map(|operation| (!seen.insert(operation.name)).then_some(operation.name))
        .collect()
}
