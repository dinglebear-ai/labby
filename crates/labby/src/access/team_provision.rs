use labby_auth::{PrincipalLink, VerifiedIdentity};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use super::error::{AccessStoreError, AccessStoreResult};
use super::store::map_sqlite_error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TeamMemberProvisionOutcome {
    Created,
    AlreadyActive,
}

fn scoped_fingerprint(
    identity: &VerifiedIdentity,
    organization_id: &str,
    project_id: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"labby.team-membership.v1\0");
    digest.update(identity.safe_fingerprint());
    digest.update(b"\0");
    digest.update(organization_id);
    digest.update(b"\0");
    digest.update(project_id);
    hex::encode(digest.finalize())
}

pub(super) fn provision(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<TeamMemberProvisionOutcome> {
    let PrincipalLink::External { issuer, subject } = identity.principal_link() else {
        return Err(AccessStoreError::NotAuthorized);
    };
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let (organization_id, project_status, organization_status, organization_epoch, creator): (String, String, String, i64, String) =
        transaction
            .query_row(
                "SELECT p.organization_id,p.status,o.status,o.policy_epoch,m.principal_id
                 FROM projects p
                 JOIN organizations o ON o.organization_id=p.organization_id
                 JOIN project_memberships m ON m.organization_id=p.organization_id AND m.project_id=p.project_id
                 JOIN principals actor ON actor.organization_id=m.organization_id AND actor.principal_id=m.principal_id
                 WHERE p.project_id=?1 AND m.status='active' AND actor.status='active'
                   AND m.role IN ('owner','admin')
                 ORDER BY CASE m.role WHEN 'owner' THEN 0 ELSE 1 END,m.principal_id LIMIT 1",
                [project_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or(AccessStoreError::NotAuthorized)?;
    if project_status != "active" || organization_status != "active" {
        return Err(AccessStoreError::NotAuthorized);
    }

    let existing: Option<(String, String, String)> = transaction
        .query_row(
            "SELECT l.principal_id,l.status,p.status
             FROM principal_links l JOIN principals p ON p.principal_id=l.principal_id
             WHERE l.link_kind='external' AND l.issuer=?1 AND l.subject=?2",
            params![issuer, subject],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let identity_fingerprint = identity.safe_fingerprint().replace(':', "-");
    let principal_id = match existing {
        Some((principal_id, link_status, principal_status)) => {
            if link_status != "active" || principal_status != "active" {
                return Err(AccessStoreError::NotAuthorized);
            }
            let principal_org: String = transaction
                .query_row(
                    "SELECT organization_id FROM principals WHERE principal_id=?1",
                    [&principal_id],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            if principal_org != organization_id {
                return Err(AccessStoreError::NotAuthorized);
            }
            principal_id
        }
        None => {
            let principal_id = format!("team-member-{identity_fingerprint}");
            let link_id = format!("team-link-{identity_fingerprint}");
            transaction.execute(
                "INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,?2,'user','active',NULL,unixepoch(),unixepoch())",
                params![principal_id, organization_id],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'external',?3,?4,NULL,'active',1,1,unixepoch(),unixepoch())",
                params![link_id, principal_id, issuer, subject],
            ).map_err(map_sqlite_error)?;
            principal_id
        }
    };

    let membership: Option<(String, String)> = transaction
        .query_row(
            "SELECT role,status FROM project_memberships WHERE organization_id=?1 AND project_id=?2 AND principal_id=?3",
            params![organization_id, project_id, principal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    match membership {
        Some((role, status))
            if status == "active" && matches!(role.as_str(), "member" | "admin" | "owner") =>
        {
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(TeamMemberProvisionOutcome::AlreadyActive)
        }
        Some(_) => Err(AccessStoreError::NotAuthorized),
        None => {
            let scoped_fingerprint = scoped_fingerprint(identity, &organization_id, project_id);
            let membership_id = format!("team-membership-{scoped_fingerprint}");
            transaction.execute(
                "INSERT INTO project_memberships(membership_id,organization_id,project_id,principal_id,role,status,created_by,created_at,updated_at) VALUES(?1,?2,?3,?4,'member','active',?5,unixepoch(),unixepoch())",
                params![membership_id, organization_id, project_id, principal_id, creator],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE access_metadata SET global_revision=global_revision+1,updated_at=unixepoch() WHERE singleton=1",
                [],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json) VALUES(?1,unixepoch(),NULL,?2,?3,?4,'access.team_member.provision','project_membership',?5,'allow','verified_team_admission',?6,'{\"role\":\"member\"}')",
                params![format!("team-provision-{scoped_fingerprint}"), principal_id, organization_id, project_id, scoped_fingerprint, organization_epoch],
            ).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(TeamMemberProvisionOutcome::Created)
        }
    }
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;
    use crate::access::{AccessStore, BootstrapOwnerInput, Permission};

    fn identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, AccessStore) {
        let directory = crate::access::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner, "Team", "Team").unwrap())
            .await
            .unwrap();
        store
            .execute_test_statement(
                "INSERT INTO project_loadouts VALUES('bootstrap-local','bootstrap-default','team','bootstrap-owner',1,1);",
            )
            .await
            .unwrap();
        (directory, store)
    }

    #[tokio::test]
    async fn first_admission_is_member_and_repeat_is_idempotent_without_escalation() {
        let (_directory, store) = fixture().await;
        let employee = identity("employee");
        assert_eq!(
            store
                .provision_team_member(employee.clone(), "bootstrap-default".into())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::Created
        );
        assert_eq!(
            store
                .provision_team_member(employee.clone(), "bootstrap-default".into())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        assert!(
            store
                .authorize_skill_library(
                    employee.clone(),
                    "bootstrap-default".into(),
                    Permission::AssetUse,
                )
                .await
                .is_ok()
        );
        assert!(
            store
                .authorize_project(crate::access::AuthorizeProjectInput::new(
                    employee,
                    "bootstrap-default",
                    Permission::ProjectManage,
                ))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn disabled_membership_is_terminal_and_is_not_reactivated() {
        let (_directory, store) = fixture().await;
        let employee = identity("revoked");
        store
            .provision_team_member(employee.clone(), "bootstrap-default".into())
            .await
            .unwrap();
        store
            .execute_test_statement(
                "UPDATE project_memberships SET status='disabled' WHERE principal_id LIKE 'team-member-%';",
            )
            .await
            .unwrap();
        assert!(
            store
                .provision_team_member(employee, "bootstrap-default".into())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn identity_already_bound_to_another_tenant_is_rejected() {
        let (_directory, store) = fixture().await;
        store
            .execute_test_statement(
                "INSERT INTO organizations VALUES('other','Other','active',0,2,2);
                 INSERT INTO principals VALUES('other-user','other','user','active',NULL,2,2);
                 INSERT INTO principal_links VALUES('other-link','other-user','external','https://accounts.google.com','outsider',NULL,'active',1,1,2,2);",
            )
            .await
            .unwrap();
        assert!(
            store
                .provision_team_member(identity("outsider"), "bootstrap-default".into())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn same_identity_can_be_provisioned_into_two_projects_without_id_collision() {
        let (_directory, store) = fixture().await;
        store
            .execute_test_statement(
                "INSERT INTO projects VALUES('second-project','bootstrap-local','Second','active',0,2,2);
                 INSERT INTO project_memberships VALUES('second-owner','bootstrap-local','second-project','bootstrap-owner','owner','active','bootstrap-owner',2,2);
                 INSERT INTO project_loadouts VALUES('bootstrap-local','second-project','team','bootstrap-owner',2,2);",
            )
            .await
            .unwrap();
        let employee = identity("two-project-employee");
        for project in ["bootstrap-default", "second-project"] {
            assert_eq!(
                store
                    .provision_team_member(employee.clone(), project.into())
                    .await
                    .unwrap(),
                TeamMemberProvisionOutcome::Created
            );
            assert!(
                store
                    .authorize_skill_library(employee.clone(), project.into(), Permission::AssetUse)
                    .await
                    .is_ok()
            );
        }
    }
}
