use std::collections::BTreeSet;

use labby_auth::{VerifiedIdentity, auth_context::AuthContext};
use labby_primitives::access::{
    ActionRef, Capability, CapabilitySchemaVersion, OwnerScope, PrincipalId, ResourceFamily,
    ResourceId, ResourceRef, RoleTemplate,
};
use labby_runtime::authority::{
    AUTHORITY_EPOCH_VECTOR_VERSION, AuthorityBinding, AuthorityEpochVector,
    AuthorityEpochVectorInput, AuthorityLease, AuthoritySafeBoundary, TeamMembershipEpoch,
};

use super::domain::{ProjectRole, TeamRole};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::error::{AccessStoreError, AccessStoreResult};
use super::read::resolve_principal;
use super::store::{AccessStore, map_sqlite_error};

const ACTION_SCHEMA_VERSION: u16 = 1;
const LEASE_LIFETIME_MILLIS: u64 = 30_000;

/// Trusted action registry entry supplied by the product dispatcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActionAuthoritySpec {
    action: ActionRef,
    resource_family: ResourceFamily,
    capability: Capability,
}

impl ActionAuthoritySpec {
    pub(crate) const SCHEMA_VERSION: u16 = ACTION_SCHEMA_VERSION;

    pub(crate) fn new(
        action: ActionRef,
        resource_family: ResourceFamily,
        capability: Capability,
    ) -> Self {
        Self {
            action,
            resource_family,
            capability,
        }
    }
}

/// Transport authority is a ceiling on durable authority, never a grant.
///
/// The ceiling has exactly two levels. `lab` (and the read-only `lab:read`
/// subset) admits every non-platform capability, so the durable role decides
/// what a Team owner, admin, or member may do; `lab:admin` additionally admits
/// the platform capabilities. An action whose required capability is
/// platform-level must therefore be `requires_admin` on every surface; the
/// two axes are one fact expressed twice.
#[derive(Clone, Debug)]
pub(crate) struct AuthorityCeiling {
    capabilities: BTreeSet<Capability>,
}

impl AuthorityCeiling {
    pub(crate) fn from_auth_context(context: &AuthContext) -> Self {
        let has = |name: &str| context.scopes.iter().any(|scope| scope == name);
        let capabilities = if has("lab:admin") {
            Capability::ALL.iter().copied().collect()
        } else if has("lab") {
            Self::scoped_capabilities().collect()
        } else if has("lab:read") {
            BTreeSet::from([Capability::ScopeRead, Capability::PolicyExplain])
        } else {
            BTreeSet::new()
        };
        Self { capabilities }
    }

    /// Every capability that is not installation authority.
    fn scoped_capabilities() -> impl Iterator<Item = Capability> {
        Capability::ALL
            .iter()
            .copied()
            .filter(|capability| !capability.is_platform())
    }

    /// Explicit ceiling for trusted local stdio, where there is no request AuthContext.
    pub(crate) fn trusted_local() -> Self {
        Self {
            capabilities: Capability::ALL.iter().copied().collect(),
        }
    }

    fn allows(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Whether this ceiling admits any platform (installation) capability.
    pub(crate) fn admits_platform(&self) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.is_platform())
    }
}

/// Exact action/resource request. The registry is trusted product metadata, not caller input.
#[derive(Clone)]
pub(crate) struct AuthorityRequest {
    identity: VerifiedIdentity,
    action_schema_version: u16,
    action: ActionRef,
    resource: ResourceRef,
    ceiling: AuthorityCeiling,
    intent_id: Option<ResourceId>,
    issued_at_millis: u64,
    safe_boundaries: Vec<AuthoritySafeBoundary>,
    registry: Vec<ActionAuthoritySpec>,
}

impl AuthorityRequest {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        identity: VerifiedIdentity,
        action_schema_version: u16,
        action: ActionRef,
        resource: ResourceRef,
        ceiling: AuthorityCeiling,
        intent_id: Option<ResourceId>,
        issued_at_millis: u64,
        safe_boundaries: Vec<AuthoritySafeBoundary>,
        registry: Vec<ActionAuthoritySpec>,
    ) -> Self {
        Self {
            identity,
            action_schema_version,
            action,
            resource,
            ceiling,
            intent_id,
            issued_at_millis,
            safe_boundaries,
            registry,
        }
    }

    pub(crate) fn identity(&self) -> &VerifiedIdentity {
        &self.identity
    }

    pub(crate) fn for_resource(&self, resource: ResourceRef) -> Self {
        let mut request = self.clone();
        request.resource = resource;
        request
    }
}

struct ResolvedAuthority {
    principal_id: String,
    capability: Capability,
    epochs: AuthorityEpochVector,
}

/// Resolve current durable facts and construct the runtime lease inside one
/// read transaction on the store's serialized connection; the connection is
/// released when the closure returns. Callers must still validate the lease
/// at each declared safe boundary.
pub(crate) async fn authorize_action(
    store: &AccessStore,
    request: AuthorityRequest,
) -> AccessStoreResult<AuthorityLease> {
    store
        .with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(map_sqlite_error)?;
            let lease = authorize_action_in_transaction(&transaction, request)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(lease)
        })
        .await
}

pub(crate) fn authorize_action_in_transaction(
    transaction: &Transaction<'_>,
    request: AuthorityRequest,
) -> AccessStoreResult<AuthorityLease> {
    let AuthorityRequest {
        identity,
        action_schema_version,
        action,
        resource,
        ceiling,
        intent_id,
        issued_at_millis,
        safe_boundaries,
        registry,
    } = request;
    if action_schema_version != ACTION_SCHEMA_VERSION {
        return Err(AccessStoreError::NotAuthorized);
    }
    let capability = registry
        .iter()
        .find(|spec| {
            spec.action.service() == action.service()
                && spec.action.action() == action.action()
                && spec.resource_family == resource.family()
        })
        .map(|spec| spec.capability)
        .ok_or(AccessStoreError::NotAuthorized)?;
    if !ceiling.allows(capability) {
        return Err(AccessStoreError::NotAuthorized);
    }

    let owner = resource.owner().clone();
    let resolved = resolve_authority(transaction, &identity, &owner, capability)?;

    // Contract violations here mean the durable vocabulary or the trusted
    // registry produced something the runtime types refuse (an invalid
    // method token, an unencodable epoch vector, an unbounded lease). That is
    // malformed store/registry state, not an outage, so it is typed as such
    // instead of being flattened into a string.
    let binding = AuthorityBinding::new(
        PrincipalId::new(resolved.principal_id)
            .map_err(|_| AccessStoreError::MalformedVocabulary)?,
        resource.owner().clone(),
        resolved.capability,
        format!("{}.{}", action.service(), action.action()),
        resource.id().clone(),
        intent_id,
    )
    .map_err(|_| AccessStoreError::MalformedVocabulary)?;
    AuthorityLease::new(
        binding,
        &resolved.epochs,
        issued_at_millis,
        issued_at_millis
            .checked_add(LEASE_LIFETIME_MILLIS)
            .ok_or(AccessStoreError::MalformedVocabulary)?,
        safe_boundaries,
    )
    .map_err(|_| AccessStoreError::MalformedVocabulary)
}

pub(crate) async fn refresh_authority_epochs(
    store: &AccessStore,
    identity: VerifiedIdentity,
    owner: OwnerScope,
    capability: Capability,
) -> AccessStoreResult<AuthorityEpochVector> {
    store
        .with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(map_sqlite_error)?;
            let resolved = resolve_authority(&transaction, &identity, &owner, capability)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(resolved.epochs)
        })
        .await
}

/// Resolve the caller's personal owner scope from verified durable identity facts. This avoids
/// accepting a principal identifier from an untrusted adapter payload.
pub(crate) async fn resolve_personal_owner(
    store: &AccessStore,
    identity: VerifiedIdentity,
) -> AccessStoreResult<OwnerScope> {
    let principal_id = store
        .with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(map_sqlite_error)?;
            let principal = resolve_principal(&transaction, &identity).map_err(collapse_denial)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(principal.id)
        })
        .await?;
    Ok(OwnerScope::Personal(
        PrincipalId::new(principal_id).map_err(|_| AccessStoreError::MalformedVocabulary)?,
    ))
}

fn resolve_authority(
    transaction: &Transaction<'_>,
    identity: &VerifiedIdentity,
    owner: &OwnerScope,
    capability: Capability,
) -> AccessStoreResult<ResolvedAuthority> {
    let principal = resolve_principal(transaction, identity).map_err(collapse_denial)?;
    let is_platform_admin: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM platform_administrators
             WHERE principal_id=?1 AND status='active')",
            [&principal.id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;

    let (
        role,
        organization_id,
        team_epochs,
        team_policy_epoch,
        project_membership_epoch,
        project_policy_epoch,
    ): (
        RoleTemplate,
        String,
        Vec<TeamMembershipEpoch>,
        Option<u64>,
        Option<u64>,
        Option<u64>,
    ) = match owner {
        OwnerScope::Installation(_) => {
            if !is_platform_admin {
                return Err(AccessStoreError::NotAuthorized);
            }
            (
                RoleTemplate::PlatformAdmin,
                principal.organization_id.clone(),
                Vec::new(),
                None,
                None,
                None,
            )
        }
        OwnerScope::Personal(owner_id) => {
            if !is_platform_admin && owner_id.as_str() != principal.id {
                return Err(AccessStoreError::NotAuthorized);
            }
            let organization_id = owner_organization(transaction, owner_id.as_str())?;
            let role = if is_platform_admin {
                RoleTemplate::PlatformAdmin
            } else {
                RoleTemplate::PersonalUser
            };
            (role, organization_id, Vec::new(), None, None, None)
        }
        OwnerScope::Team(team_id) => {
            let row = transaction.query_row(
                    "SELECT g.organization_id,g.status,g.policy_epoch,m.role,m.status,m.membership_epoch
                     FROM groups g LEFT JOIN team_memberships m
                       ON m.organization_id=g.organization_id AND m.team_id=g.group_id AND m.principal_id=?1
                     WHERE g.group_id=?2 AND g.kind='team'",
                    params![principal.id, team_id.as_str()],
                    |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,Option<i64>>(5)?)),
                ).optional().map_err(map_sqlite_error)?.ok_or(AccessStoreError::NotAuthorized)?;
            // Platform administrators retain recovery authority over suspended teams so they can
            // reactivate them. Ordinary membership authority remains fail-closed while suspended.
            if row.1 != "active" && !(is_platform_admin && row.1 == "suspended") {
                return Err(AccessStoreError::NotAuthorized);
            }
            let (role, membership_epoch) = if is_platform_admin {
                (RoleTemplate::PlatformAdmin, row.5)
            } else if row.4.as_deref() == Some("active") {
                (team_role(row.3.as_deref())?, row.5)
            } else {
                return Err(AccessStoreError::NotAuthorized);
            };
            let team_epochs = membership_epoch
                .map(|epoch| {
                    epoch_value(epoch).map(|epoch| TeamMembershipEpoch {
                        team_id: team_id.as_str().to_owned(),
                        epoch,
                    })
                })
                .transpose()?
                .into_iter()
                .collect();
            (
                role,
                row.0,
                team_epochs,
                Some(epoch_value(row.2)?),
                None,
                None,
            )
        }
        OwnerScope::Project(project_id) => {
            let project = transaction
                .query_row(
                    "SELECT organization_id,status,project_policy_epoch FROM projects WHERE project_id=?1",
                    [project_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or(AccessStoreError::NotAuthorized)?;
            if project.1 != "active" {
                return Err(AccessStoreError::NotAuthorized);
            }
            let (role, team_epochs, membership_epoch) = if is_platform_admin {
                (RoleTemplate::PlatformAdmin, Vec::new(), None)
            } else {
                let effective = effective_project_role(
                    transaction,
                    &principal.id,
                    &project.0,
                    project_id.as_str(),
                )?
                .ok_or(AccessStoreError::NotAuthorized)?;
                (
                    effective.role.role_template(),
                    effective.team_epochs,
                    effective.membership_epoch,
                )
            };
            (
                role,
                project.0,
                team_epochs,
                None,
                membership_epoch,
                Some(epoch_value(project.2)?),
            )
        }
    };

    if !role
        .capabilities(CapabilitySchemaVersion::V1)
        .is_some_and(|capabilities| capabilities.contains(&capability))
    {
        return Err(AccessStoreError::NotAuthorized);
    }
    let (schema_generation, installation_epoch) = transaction
        .query_row(
            "SELECT schema_version,global_revision FROM access_metadata WHERE singleton=1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(map_sqlite_error)?;
    let organization_epoch: i64 = transaction
        .query_row(
            "SELECT policy_epoch FROM organizations WHERE organization_id=?1 AND status='active'",
            [&organization_id],
            |row| row.get(0),
        )
        .map_err(|error| collapse_denial(map_sqlite_error(error)))?;
    // Monotonic Principal epoch maintained by the schema triggers; it moves on
    // every status/kind/organization change and on every credential-link
    // change, so a revoked credential or suspended Principal invalidates
    // outstanding leases.
    let principal_epoch: i64 = transaction
        .query_row(
            "SELECT e.epoch FROM principal_epochs e JOIN principals p ON p.principal_id=e.principal_id WHERE p.principal_id=?1 AND p.status='active'",
            [&principal.id],
            |row| row.get(0),
        )
        .map_err(|error| collapse_denial(map_sqlite_error(error)))?;
    let epochs = AuthorityEpochVector::new(AuthorityEpochVectorInput {
        version: AUTHORITY_EPOCH_VECTOR_VERSION,
        authority_schema_generation: epoch_value(schema_generation)?,
        installation_epoch: epoch_value(installation_epoch)?,
        organization_epoch: epoch_value(organization_epoch)?,
        principal_epoch: epoch_value(principal_epoch)?,
        team_membership_epochs: team_epochs,
        team_policy_epoch,
        project_membership_epoch,
        project_policy_epoch,
        resource_policy_epoch: None,
        gateway_catalog_generation: None,
        depot_projection_watermark: None,
        // The persisted link generation advances when the credential behind
        // this identity is revoked or replaced.
        credential_generation: Some(principal.link_generation),
        // Browser sessions are revoked at the transport boundary by labby-auth;
        // the vector carries the persisted verification generation of the
        // identity link, not a per-session counter.
        session_generation: principal.verification_generation,
    })
    .map_err(|_| AccessStoreError::MalformedVocabulary)?;
    Ok(ResolvedAuthority {
        principal_id: principal.id,
        capability,
        epochs,
    })
}

struct EffectiveProjectRole {
    role: ProjectRole,
    /// Every active Team assignment that contributed to the decision.
    team_epochs: Vec<TeamMembershipEpoch>,
    /// Direct membership epoch when a direct membership contributed.
    membership_epoch: Option<u64>,
}

/// The effective Project role is the maximum of the active direct membership
/// role and every active Team-derived assignment role, exactly as the read
/// surfaces report it. Every contributing membership's epoch joins the vector
/// so a downgrade on any path invalidates the lease.
fn effective_project_role(
    transaction: &Transaction<'_>,
    principal_id: &str,
    organization_id: &str,
    project_id: &str,
) -> AccessStoreResult<Option<EffectiveProjectRole>> {
    let direct = transaction
        .query_row(
            "SELECT m.role,e.epoch FROM project_memberships m
             JOIN project_membership_epochs e ON e.membership_id=m.membership_id
             WHERE m.organization_id=?1 AND m.project_id=?2 AND m.principal_id=?3 AND m.status='active'",
            params![organization_id, project_id, principal_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let (mut best, membership_epoch): (Option<ProjectRole>, Option<u64>) = match direct {
        Some((role, epoch)) => (
            Some(ProjectRole::from_persisted(&role).ok_or(AccessStoreError::MalformedVocabulary)?),
            Some(epoch_value(epoch)?),
        ),
        None => (None, None),
    };
    let mut statement = transaction
        .prepare(
            "SELECT tm.team_id,tm.membership_epoch,a.role
             FROM team_memberships tm
             JOIN groups g ON g.organization_id=tm.organization_id AND g.group_id=tm.team_id
             JOIN team_project_assignments a
               ON a.organization_id=tm.organization_id AND a.team_id=tm.team_id
             WHERE tm.organization_id=?1 AND tm.principal_id=?2 AND a.project_id=?3
               AND tm.status='active' AND g.kind='team' AND g.status='active' AND a.status='active'
             ORDER BY tm.team_id COLLATE BINARY",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![organization_id, principal_id, project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut team_epochs = Vec::new();
    for row in rows {
        let (team_id, epoch, role) = row.map_err(map_sqlite_error)?;
        let role =
            ProjectRole::from_persisted(&role).ok_or(AccessStoreError::MalformedVocabulary)?;
        best = Some(best.map_or(role, |current| current.max_with(role)));
        team_epochs.push(TeamMembershipEpoch {
            team_id,
            epoch: epoch_value(epoch)?,
        });
    }
    Ok(best.map(|role| EffectiveProjectRole {
        role,
        team_epochs,
        membership_epoch,
    }))
}

fn owner_organization(
    transaction: &Transaction<'_>,
    principal_id: &str,
) -> AccessStoreResult<String> {
    transaction
        .query_row(
            "SELECT organization_id FROM principals WHERE principal_id=?1 AND status='active'",
            [principal_id],
            |row| row.get(0),
        )
        .map_err(|error| collapse_denial(map_sqlite_error(error)))
}

fn team_role(role: Option<&str>) -> AccessStoreResult<RoleTemplate> {
    role.and_then(TeamRole::from_persisted)
        .map(TeamRole::role_template)
        .ok_or(AccessStoreError::MalformedVocabulary)
}

fn epoch_value(value: i64) -> AccessStoreResult<u64> {
    u64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary)
}

/// Identity/project resolution failures are denials; every other store
/// failure (locked, corrupt, unavailable) keeps its typed cause.
pub(super) fn collapse_denial(error: AccessStoreError) -> AccessStoreError {
    match error {
        AccessStoreError::IdentityUnavailable | AccessStoreError::ProjectAccessUnavailable => {
            AccessStoreError::NotAuthorized
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::Authenticator;
    use labby_primitives::access::{ActionRef, ProjectId, ResourceFamily, TeamId};
    use labby_primitives::agent::{
        AgentDefinition, AgentRevision, AgentState, RunningRevocationPolicy,
    };
    use labby_runtime::authority::{AuthorityLeaseError, AuthoritySafeBoundary};

    use super::*;
    use crate::access::BootstrapOwnerInput;

    fn identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    fn action(name: &str) -> ActionRef {
        ActionRef::new("tasks", name).unwrap()
    }

    fn team_resource() -> ResourceRef {
        ResourceRef::new(
            OwnerScope::Team(TeamId::new("bootstrap-initial-team").unwrap()),
            ResourceFamily::Task,
            ResourceId::new("task-1").unwrap(),
        )
    }

    fn agent_definition() -> AgentDefinition {
        let digest = format!("sha256:{}", "a".repeat(64));
        AgentDefinition {
            id: "agent-1".into(),
            owner: OwnerScope::Team(TeamId::new("bootstrap-initial-team").unwrap()),
            revision: AgentRevision {
                version: 1,
                content_digest: digest.clone(),
                repository_digest: digest.clone(),
                image_digest: digest.clone(),
                harness_digest: digest.clone(),
                loadout_digest: digest,
                catalog_generation: "catalog-1".into(),
                credential_references: vec![],
            },
            state: AgentState::Active,
            required_capabilities: vec![],
            authority_epoch: 1,
            publication_epoch: 1,
            revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
        }
    }

    fn agent_request(identity: VerifiedIdentity) -> AuthorityRequest {
        let action = ActionRef::new("agents", "agents.create").unwrap();
        AuthorityRequest::new(
            identity,
            ActionAuthoritySpec::SCHEMA_VERSION,
            action.clone(),
            ResourceRef::new(
                OwnerScope::Team(TeamId::new("bootstrap-initial-team").unwrap()),
                ResourceFamily::Agent,
                ResourceId::new("agent-1").unwrap(),
            ),
            AuthorityCeiling::trusted_local(),
            None,
            1_000,
            vec![AuthoritySafeBoundary::BeforeCommit],
            vec![ActionAuthoritySpec::new(
                action,
                ResourceFamily::Agent,
                Capability::ScopeRead,
            )],
        )
    }

    fn request(
        identity: VerifiedIdentity,
        action_name: &str,
        version: u16,
        capability: Capability,
        ceiling: AuthorityCeiling,
    ) -> AuthorityRequest {
        AuthorityRequest::new(
            identity,
            version,
            action(action_name),
            team_resource(),
            ceiling,
            None,
            1_000,
            vec![AuthoritySafeBoundary::BeforeDispatch],
            vec![ActionAuthoritySpec::new(
                action("read"),
                ResourceFamily::Task,
                capability,
            )],
        )
    }

    async fn fixture() -> (
        tempfile::TempDir,
        AccessStore,
        VerifiedIdentity,
        VerifiedIdentity,
    ) {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = identity("owner-subject");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        store.execute_test_statement(
            "INSERT INTO principals VALUES('member-1','bootstrap-local','user','active','Member',10,10);
             INSERT INTO principal_links VALUES('member-link','member-1','external','https://accounts.google.com','member-subject',NULL,'active',1,1,10,10);
             INSERT INTO team_memberships VALUES('member-membership','bootstrap-local','bootstrap-initial-team','member-1','member','active',2,'bootstrap-owner',10,10,NULL);"
        ).await.unwrap();
        (directory, store, owner, identity("member-subject"))
    }

    #[tokio::test]
    async fn durable_role_and_transport_ceiling_are_both_required() {
        let (_directory, store, owner, member) = fixture().await;
        let member_lease = authorize_action(
            &store,
            request(
                member.clone(),
                "read",
                ActionAuthoritySpec::SCHEMA_VERSION,
                Capability::ScopeRead,
                AuthorityCeiling::trusted_local(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(member_lease.binding().principal_id(), "member-1");
        assert_eq!(member_lease.binding().capability(), &Capability::ScopeRead);

        let denied_by_role = authorize_action(
            &store,
            request(
                member,
                "read",
                ActionAuthoritySpec::SCHEMA_VERSION,
                Capability::ScopeManage,
                AuthorityCeiling::trusted_local(),
            ),
        )
        .await;
        assert!(matches!(
            denied_by_role,
            Err(AccessStoreError::NotAuthorized)
        ));

        let read_only_context = AuthContext {
            sub: "owner-subject".into(),
            actor_key: None,
            scopes: vec!["lab:read".into()],
            issuer: "browser-session".into(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        let denied_by_ceiling = authorize_action(
            &store,
            request(
                owner,
                "read",
                ActionAuthoritySpec::SCHEMA_VERSION,
                Capability::ScopeManage,
                AuthorityCeiling::from_auth_context(&read_only_context),
            ),
        )
        .await;
        assert!(matches!(
            denied_by_ceiling,
            Err(AccessStoreError::NotAuthorized)
        ));
    }

    #[tokio::test]
    async fn unknown_action_and_schema_version_fail_before_authority_resolution() {
        let (_directory, store, _owner, member) = fixture().await;
        let unknown_action = authorize_action(
            &store,
            request(
                member.clone(),
                "missing",
                ActionAuthoritySpec::SCHEMA_VERSION,
                Capability::ScopeRead,
                AuthorityCeiling::trusted_local(),
            ),
        )
        .await;
        assert!(matches!(
            unknown_action,
            Err(AccessStoreError::NotAuthorized)
        ));

        let unknown_version = authorize_action(
            &store,
            request(
                member,
                "read",
                ActionAuthoritySpec::SCHEMA_VERSION + 1,
                Capability::ScopeRead,
                AuthorityCeiling::trusted_local(),
            ),
        )
        .await;
        assert!(matches!(
            unknown_version,
            Err(AccessStoreError::NotAuthorized)
        ));
    }

    #[tokio::test]
    async fn personal_scope_is_self_only_for_regular_users() {
        let (_directory, store, owner, member) = fixture().await;
        let personal_resource = ResourceRef::new(
            OwnerScope::Personal(PrincipalId::new("member-1").unwrap()),
            ResourceFamily::Library,
            ResourceId::new("library-1").unwrap(),
        );
        let make_request = |identity, resource| {
            AuthorityRequest::new(
                identity,
                ActionAuthoritySpec::SCHEMA_VERSION,
                action("read"),
                resource,
                AuthorityCeiling::trusted_local(),
                None,
                1_000,
                vec![AuthoritySafeBoundary::BeforeDispatch],
                vec![ActionAuthoritySpec::new(
                    action("read"),
                    ResourceFamily::Library,
                    Capability::ScopeRead,
                )],
            )
        };
        assert!(
            authorize_action(&store, make_request(member, personal_resource.clone()))
                .await
                .is_ok()
        );

        // The platform administrator may cross owner scopes; an ordinary user cannot. Removing
        // the durable platform role proves the latter without changing transport authority.
        store.execute_test_statement("UPDATE platform_administrators SET status='revoked',revoked_at=11 WHERE principal_id='bootstrap-owner';").await.unwrap();
        let denial = authorize_action(&store, make_request(owner, personal_resource)).await;
        assert!(matches!(denial, Err(AccessStoreError::NotAuthorized)));
    }

    #[tokio::test]
    async fn revoked_team_membership_cannot_be_raced_with_agent_mutation() {
        let (_directory, store, _owner, member) = fixture().await;
        assert!(
            authorize_action(&store, agent_request(member.clone()))
                .await
                .is_ok()
        );

        store
            .execute_test_statement(
                "UPDATE team_memberships SET status='revoked',membership_epoch=membership_epoch+1,updated_at=11,revoked_at=11 WHERE membership_id='member-membership';",
            )
            .await
            .unwrap();

        assert!(matches!(
            store
                .authorize_and_put_agent_definition(
                    agent_request(member),
                    agent_definition(),
                    "member-1".into(),
                    12,
                )
                .await,
            Err(AccessStoreError::NotAuthorized)
        ));
        assert!(
            store
                .get_agent_definition("agent-1".into())
                .await
                .unwrap()
                .is_none()
        );
    }

    fn project_request(identity: VerifiedIdentity, capability: Capability) -> AuthorityRequest {
        let action = ActionRef::new("projects", "projects.update").unwrap();
        AuthorityRequest::new(
            identity,
            ActionAuthoritySpec::SCHEMA_VERSION,
            action.clone(),
            ResourceRef::new(
                OwnerScope::Project(ProjectId::new("bootstrap-default").unwrap()),
                ResourceFamily::Project,
                ResourceId::new("bootstrap-default").unwrap(),
            ),
            AuthorityCeiling::trusted_local(),
            None,
            1_000,
            vec![AuthoritySafeBoundary::BeforeDispatch],
            vec![ActionAuthoritySpec::new(
                action,
                ResourceFamily::Project,
                capability,
            )],
        )
    }

    #[tokio::test]
    async fn team_derived_project_roles_authorize_and_bind_their_epochs() {
        let (_directory, store, _owner, member) = fixture().await;
        // The member has no direct Project membership: denied.
        assert!(matches!(
            authorize_action(
                &store,
                project_request(member.clone(), Capability::ScopeRead)
            )
            .await,
            Err(AccessStoreError::NotAuthorized)
        ));
        // Assign the Team to the Project as admin: the member now holds the
        // Team-derived admin role and may manage.
        store
            .execute_test_statement(
                "INSERT INTO team_project_assignments VALUES('assign-1','bootstrap-local','bootstrap-initial-team','bootstrap-default','admin','active',1,'bootstrap-owner',20,20,NULL);",
            )
            .await
            .unwrap();
        let lease = authorize_action(
            &store,
            project_request(member.clone(), Capability::ScopeManage),
        )
        .await
        .unwrap();
        assert_eq!(lease.binding().principal_id(), "member-1");
        let epochs = refresh_authority_epochs(
            &store,
            member.clone(),
            OwnerScope::Project(ProjectId::new("bootstrap-default").unwrap()),
            Capability::ScopeManage,
        )
        .await
        .unwrap();
        assert!(
            lease
                .validate_at(AuthoritySafeBoundary::BeforeDispatch, 1_500, &epochs)
                .is_ok()
        );
        // Downgrading the Team's assignment changes the contributing epochs
        // and both re-authorization and the outstanding lease observe it.
        store
            .execute_test_statement(
                "UPDATE team_project_assignments SET role='viewer',assignment_epoch=assignment_epoch+1 WHERE assignment_id='assign-1'; UPDATE team_memberships SET membership_epoch=membership_epoch+1 WHERE membership_id='member-membership';",
            )
            .await
            .unwrap();
        assert!(matches!(
            authorize_action(
                &store,
                project_request(member.clone(), Capability::ScopeManage)
            )
            .await,
            Err(AccessStoreError::NotAuthorized)
        ));
        let downgraded = refresh_authority_epochs(
            &store,
            member,
            OwnerScope::Project(ProjectId::new("bootstrap-default").unwrap()),
            Capability::ScopeRead,
        )
        .await
        .unwrap();
        assert_eq!(
            lease.validate_at(AuthoritySafeBoundary::BeforeDispatch, 1_500, &downgraded),
            Err(AuthorityLeaseError::AuthorityChanged)
        );
    }

    #[tokio::test]
    async fn direct_project_role_downgrade_and_credential_revocation_invalidate_leases() {
        let (_directory, store, _owner, member) = fixture().await;
        store
            .execute_test_statement(
                "INSERT INTO project_memberships VALUES('member-project','bootstrap-local','bootstrap-default','member-1','admin','active','bootstrap-owner',20,20);",
            )
            .await
            .unwrap();
        let project = OwnerScope::Project(ProjectId::new("bootstrap-default").unwrap());
        let lease = authorize_action(
            &store,
            project_request(member.clone(), Capability::ScopeManage),
        )
        .await
        .unwrap();
        let current = refresh_authority_epochs(
            &store,
            member.clone(),
            project.clone(),
            Capability::ScopeRead,
        )
        .await
        .unwrap();
        assert_eq!(current.project_membership_epoch(), Some(1));
        assert!(
            lease
                .validate_at(AuthoritySafeBoundary::BeforeDispatch, 1_500, &current)
                .is_ok()
        );
        // A role downgrade through any statement bumps the membership epoch.
        store
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE membership_id='member-project';",
            )
            .await
            .unwrap();
        let downgraded = refresh_authority_epochs(
            &store,
            member.clone(),
            project.clone(),
            Capability::ScopeRead,
        )
        .await
        .unwrap();
        assert_eq!(downgraded.project_membership_epoch(), Some(2));
        assert_eq!(
            lease.validate_at(AuthoritySafeBoundary::BeforeDispatch, 1_500, &downgraded),
            Err(AuthorityLeaseError::AuthorityChanged)
        );
        // Revoking the credential link behind the identity moves both the
        // principal epoch and the credential generation; the identity itself
        // no longer resolves.
        store
            .execute_test_statement(
                "UPDATE principal_links SET status='revoked',link_generation=link_generation+1 WHERE link_id='member-link';",
            )
            .await
            .unwrap();
        assert!(matches!(
            refresh_authority_epochs(&store, member, project, Capability::ScopeRead).await,
            Err(AccessStoreError::NotAuthorized)
        ));
        let epoch: i64 = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT epoch FROM principal_epochs WHERE principal_id='member-1'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(epoch, 2);
    }

    #[tokio::test]
    async fn lab_scope_admits_team_management_and_only_lab_admin_admits_platform() {
        let (_directory, store, owner, _member) = fixture().await;
        let context = |scopes: &[&str]| AuthContext {
            sub: "owner-subject".into(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            issuer: "browser-session".into(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        // A Team owner with the ordinary `lab` scope manages its Team: the
        // durable role decides, the transport only caps platform authority.
        let lab = AuthorityCeiling::from_auth_context(&context(&["lab"]));
        assert!(lab.allows(Capability::ScopeManage));
        assert!(lab.allows(Capability::MembershipManage));
        assert!(lab.allows(Capability::OwnershipTransfer));
        assert!(!lab.admits_platform());
        assert!(
            authorize_action(
                &store,
                request(
                    owner.clone(),
                    "read",
                    ActionAuthoritySpec::SCHEMA_VERSION,
                    Capability::MembershipManage,
                    lab,
                ),
            )
            .await
            .is_ok()
        );
        let read_only = AuthorityCeiling::from_auth_context(&context(&["lab:read"]));
        assert!(read_only.allows(Capability::ScopeRead));
        assert!(!read_only.allows(Capability::ScopeOperate));
        let admin = AuthorityCeiling::from_auth_context(&context(&["lab", "lab:admin"]));
        assert!(admin.admits_platform());
        assert!(admin.allows(Capability::PlatformManage));
        for capability in Capability::ALL {
            assert_eq!(
                AuthorityCeiling::from_auth_context(&context(&["lab"])).allows(*capability),
                !capability.is_platform(),
                "{}",
                capability.as_wire()
            );
        }
    }
}
