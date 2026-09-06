use labby_auth::VerifiedIdentity;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::domain::{Permission, ProjectRole};
use super::error::{AccessStoreError, AccessStoreResult};
use super::read::{select_project_in_transaction, select_project_membership_in_transaction};
use super::store::map_sqlite_error;

/// Exact request facts for one Project-scoped authorization decision.
///
/// This deliberately does not implement `Debug`: verified identity material must not leak into
/// diagnostics.
pub(crate) struct AuthorizeProjectInput {
    identity: VerifiedIdentity,
    project_id: String,
    permission: Permission,
}

impl AuthorizeProjectInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        project_id: impl Into<String>,
        permission: Permission,
    ) -> Self {
        Self {
            identity,
            project_id: project_id.into(),
            permission,
        }
    }
}

/// Redacted facts from one project-level permission snapshot.
///
/// This is not a reusable dispatch grant: it binds no concrete gateway action, target, or catalog
/// generation. Final dispatch must reauthorize the exact operation at its in-process boundary.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ProjectPermissionSnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) loadout_name: String,
    pub(crate) permission: Permission,
    pub(crate) global_revision: u64,
}

/// One exact current membership snapshot for Labby-owned library policy.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct LibraryAccessSnapshot {
    pub(crate) principal_id: String,
    pub(crate) organization_id: String,
    pub(crate) project_id: String,
    pub(crate) role: ProjectRole,
    pub(crate) global_revision: u64,
    pub(crate) team_ids: Vec<String>,
    /// Teams where the principal holds the management capability bundle (Owner/Admin).
    pub(crate) team_management_ids: Vec<String>,
    pub(crate) is_platform_admin: bool,
}

pub(super) fn authorize(
    connection: &mut Connection,
    input: &AuthorizeProjectInput,
) -> AccessStoreResult<ProjectPermissionSnapshot> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let selected = select_project_in_transaction(&transaction, &input.identity, &input.project_id)
        .map_err(collapse_denial)?;
    if !selected.role.permissions().contains(&input.permission) {
        return Err(AccessStoreError::NotAuthorized);
    }

    let snapshot = ProjectPermissionSnapshot {
        principal_id: selected.principal_id,
        organization_id: selected.organization_id,
        project_id: selected.project_id,
        role: selected.role,
        loadout_name: selected.loadout_name,
        permission: input.permission,
        global_revision: selected.global_revision,
    };
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(snapshot)
}

pub(super) fn authorize_library(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
    permission: Permission,
) -> AccessStoreResult<LibraryAccessSnapshot> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let snapshot =
        authorize_library_in_transaction(&transaction, identity, project_id, permission)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(snapshot)
}

pub(super) fn authorize_library_in_transaction(
    transaction: &Transaction<'_>,
    identity: &VerifiedIdentity,
    project_id: &str,
    permission: Permission,
) -> AccessStoreResult<LibraryAccessSnapshot> {
    let selected = select_project_membership_in_transaction(&transaction, identity, project_id)
        .map_err(collapse_denial)?;
    if !selected.role.permissions().contains(&permission) {
        return Err(AccessStoreError::NotAuthorized);
    }
    let (team_ids, team_management_ids) = {
        let mut statement = transaction
            .prepare(
                "SELECT tm.team_id, tm.role
                 FROM team_memberships tm
                 JOIN team_project_assignments assignment
                   ON assignment.organization_id=tm.organization_id
                  AND assignment.team_id=tm.team_id
                 WHERE tm.organization_id=?1 AND tm.principal_id=?2
                   AND tm.status='active' AND assignment.status='active'
                   AND assignment.project_id=?3
                 ORDER BY tm.team_id",
            )
            .map_err(map_sqlite_error)?;
        statement
            .query_map(
                params![
                    selected.organization_id,
                    selected.principal_id,
                    selected.project_id
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(map_sqlite_error)?
            .collect::<Result<Vec<(String, String)>, _>>()
            .map_err(map_sqlite_error)?
            .into_iter()
            .fold(
                (Vec::new(), Vec::new()),
                |(mut all, mut management), (id, role)| {
                    all.push(id.clone());
                    if matches!(role.as_str(), "owner" | "admin") {
                        management.push(id);
                    }
                    (all, management)
                },
            )
    };
    let is_platform_admin = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM platform_administrators
             WHERE principal_id=?1 AND status='active')",
            params![selected.principal_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let snapshot = LibraryAccessSnapshot {
        principal_id: selected.principal_id,
        organization_id: selected.organization_id,
        project_id: selected.project_id,
        role: selected.role,
        global_revision: selected.global_revision,
        team_ids,
        team_management_ids,
        is_platform_admin,
    };
    Ok(snapshot)
}

/// Authorizes management of a Project without consulting its Loadout mapping.
///
/// This narrow preflight exists for the operation that creates that mapping. It deliberately
/// returns no reusable grant; the mutation reauthorizes in its own immediate transaction.
pub(super) fn authorize_management_without_loadout(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    super::loadout::resolve_project_manager(&transaction, identity, project_id)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(())
}

fn collapse_denial(error: AccessStoreError) -> AccessStoreError {
    match error {
        AccessStoreError::IdentityUnavailable | AccessStoreError::ProjectAccessUnavailable => {
            AccessStoreError::NotAuthorized
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;
    use crate::access::{AccessStore, BootstrapOwnerInput};

    fn secure_tempdir() -> tempfile::TempDir {
        super::super::test_support::secure_tempdir()
    }

    fn identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
        let directory = secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = identity("owner-subject");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        store
            .execute_test_statement(
                "INSERT INTO projects VALUES
                   ('admin-project','bootstrap-local','Admin','active',0,2,2),
                   ('member-project','bootstrap-local','Member','active',0,2,2),
                   ('viewer-project','bootstrap-local','Viewer','active',0,2,2),
                   ('unmapped-project','bootstrap-local','Unmapped','active',0,2,2);
                 INSERT INTO project_memberships VALUES
                   ('admin-membership','bootstrap-local','admin-project','bootstrap-owner','admin','active','bootstrap-owner',2,2),
                   ('member-membership','bootstrap-local','member-project','bootstrap-owner','member','active','bootstrap-owner',2,2),
                   ('viewer-membership','bootstrap-local','viewer-project','bootstrap-owner','viewer','active','bootstrap-owner',2,2),
                   ('unmapped-membership','bootstrap-local','unmapped-project','bootstrap-owner','owner','active','bootstrap-owner',2,2);
                 INSERT INTO project_loadouts VALUES
                   ('bootstrap-local','bootstrap-default','production','bootstrap-owner',2,2),
                   ('bootstrap-local','admin-project','production','bootstrap-owner',2,2),
                   ('bootstrap-local','member-project','production','bootstrap-owner',2,2),
                   ('bootstrap-local','viewer-project','production','bootstrap-owner',2,2);
                 INSERT INTO organizations VALUES('other-org','Other','active',0,2,2);
                 INSERT INTO projects VALUES('other-project','other-org','Other','active',0,2,2);",
            )
            .await
            .unwrap();
        (directory, store, owner)
    }

    async fn decision(
        store: &AccessStore,
        identity: VerifiedIdentity,
        project_id: &str,
        permission: Permission,
    ) -> AccessStoreResult<ProjectPermissionSnapshot> {
        store
            .authorize_project(AuthorizeProjectInput::new(identity, project_id, permission))
            .await
    }

    #[tokio::test]
    async fn role_permission_matrix_uses_the_canonical_role_permissions() {
        let (_directory, store, owner) = fixture().await;
        let cases = [
            ("bootstrap-default", ProjectRole::Owner),
            ("admin-project", ProjectRole::Admin),
            ("member-project", ProjectRole::Member),
            ("viewer-project", ProjectRole::Viewer),
        ];
        let permissions = [
            Permission::ProjectRead,
            Permission::ProjectManage,
            Permission::AssetDiscover,
            Permission::AssetUse,
        ];

        for (project_id, role) in cases {
            for permission in permissions {
                let result = decision(&store, owner.clone(), project_id, permission).await;
                assert_eq!(
                    result.is_ok(),
                    role.permissions().contains(&permission),
                    "{role:?} {permission:?}"
                );
                if let Err(error) = result {
                    assert!(matches!(error, AccessStoreError::NotAuthorized));
                }
            }
        }
    }

    #[tokio::test]
    async fn allowed_snapshot_contains_only_exact_redacted_facts_and_revision() {
        let (_directory, store, owner) = fixture().await;
        let snapshot = decision(&store, owner, "member-project", Permission::AssetUse)
            .await
            .unwrap();

        assert_eq!(
            snapshot,
            ProjectPermissionSnapshot {
                principal_id: "bootstrap-owner".into(),
                organization_id: "bootstrap-local".into(),
                project_id: "member-project".into(),
                role: ProjectRole::Member,
                loadout_name: "production".into(),
                permission: Permission::AssetUse,
                global_revision: 1,
            }
        );
    }

    #[tokio::test]
    async fn ordinary_denials_are_indistinguishable_and_cross_org_is_denied() {
        let (_directory, store, owner) = fixture().await;
        let denials = [
            decision(
                &store,
                identity("unknown"),
                "member-project",
                Permission::ProjectRead,
            )
            .await,
            decision(
                &store,
                owner.clone(),
                "missing-project",
                Permission::ProjectRead,
            )
            .await,
            decision(
                &store,
                owner.clone(),
                "unmapped-project",
                Permission::ProjectRead,
            )
            .await,
            decision(
                &store,
                owner.clone(),
                "viewer-project",
                Permission::AssetUse,
            )
            .await,
            decision(&store, owner, "other-project", Permission::ProjectRead).await,
        ];
        for denial in denials {
            assert!(matches!(denial, Err(AccessStoreError::NotAuthorized)));
        }
    }

    #[tokio::test]
    async fn each_call_observes_revocation_and_authorization_never_writes_or_audits() {
        let (_directory, store, owner) = fixture().await;
        let before = store.loadout_state_for_test().await.unwrap();
        decision(
            &store,
            owner.clone(),
            "member-project",
            Permission::AssetUse,
        )
        .await
        .unwrap();
        let after_allow = store.loadout_state_for_test().await.unwrap();
        assert_eq!(after_allow, before);

        store
            .execute_test_statement(
                "UPDATE project_memberships SET status='disabled'
                 WHERE membership_id='member-membership';",
            )
            .await
            .unwrap();
        let denial = decision(&store, owner, "member-project", Permission::AssetUse).await;
        assert!(matches!(denial, Err(AccessStoreError::NotAuthorized)));
        assert_eq!(store.loadout_state_for_test().await.unwrap(), before);
    }

    #[tokio::test]
    async fn malformed_persisted_vocabulary_remains_typed() {
        let (_directory, store, owner) = fixture().await;
        store
            .execute_test_statement(
                "UPDATE project_loadouts SET loadout_name='bad
name'
                 WHERE project_id='member-project';",
            )
            .await
            .unwrap();
        let result = decision(&store, owner, "member-project", Permission::ProjectRead).await;
        assert!(matches!(result, Err(AccessStoreError::MalformedVocabulary)));
    }
}
