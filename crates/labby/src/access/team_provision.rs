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
    provision_with_role(connection, identity, project_id, InitialRole::Member)
}

pub(super) fn provision_viewer(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
) -> AccessStoreResult<TeamMemberProvisionOutcome> {
    provision_with_role(connection, identity, project_id, InitialRole::Viewer)
}

const INITIAL_TEAM_ID: &str = "bootstrap-initial-team";

/// Access an allowlist entry grants at first sign-in. Product-owned admission
/// only; callers cannot request `owner`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllowlistRole {
    Member,
    Admin,
}

impl AllowlistRole {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "member" => Some(Self::Member),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Admin => "admin",
        }
    }
}

/// Who authorized an allowlist admission. Recorded on the
/// `access.allowlist.provision` audit row so a grant can be traced back to the
/// administrator who allowed the email, or to the deployment's configured
/// admin list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AllowlistAdmission {
    /// An allowlist row, identified by the fingerprint of the provider subject
    /// that added it. The raw subject is deliberately never stored here.
    AllowlistEntry { added_by_fingerprint: String },
    /// `LABBY_AUTH_ADMIN_EMAIL`; the deployment's configuration is the
    /// authority, so there is no administrator subject to record.
    ConfiguredAdminEmail,
}

impl AllowlistAdmission {
    fn audit_metadata(&self, role: AllowlistRole) -> serde_json::Value {
        match self {
            Self::AllowlistEntry {
                added_by_fingerprint,
            } => serde_json::json!({
                "role": role.as_str(),
                "admitted_via": "allowlist",
                "added_by_fp": added_by_fingerprint,
            }),
            Self::ConfiguredAdminEmail => serde_json::json!({
                "role": role.as_str(),
                "admitted_via": "configured_admin_email",
            }),
        }
    }
}

/// Admit an allowlisted identity: a Principal (if missing), an Initial Team
/// membership, a default-Project membership, and for `Admin` a platform
/// administrator grant — one transaction, idempotent. Either an existing active
/// default-Project membership or an existing Initial Team membership (in any
/// status) means the identity was already admitted; nothing is upgraded and
/// `AlreadyActive` is returned.
pub(super) fn provision_allowlisted(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    role: AllowlistRole,
    admitted_by: AllowlistAdmission,
) -> AccessStoreResult<TeamMemberProvisionOutcome> {
    let project_id = super::bootstrap::PROJECT_ID;
    let now = unix_now()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    // An identity that already holds an Initial Team membership was admitted
    // before. Admitting it again must never rewrite that membership's role or
    // grant platform administration, so the gate runs before any write.
    if existing_initial_team_member(&transaction, identity)? {
        transaction.commit().map_err(map_sqlite_error)?;
        return Ok(TeamMemberProvisionOutcome::AlreadyActive);
    }
    let (outcome, principal_id, organization_id, organization_epoch) = provision_in_transaction(
        &transaction,
        identity,
        project_id,
        match role {
            AllowlistRole::Member => InitialRole::Member,
            AllowlistRole::Admin => InitialRole::Admin,
        },
        now,
    )?;
    if outcome == TeamMemberProvisionOutcome::AlreadyActive {
        // The helper wrote nothing on this path, so committing only releases
        // the transaction.
        transaction.commit().map_err(map_sqlite_error)?;
        return Ok(outcome);
    }
    transaction
        .execute(
            "INSERT INTO team_memberships(membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,created_by,created_at,updated_at,revoked_at)
             VALUES(?1,?2,?3,?4,?5,'active',1,?4,?6,?6,NULL)",
            params![
                format!("team-member-{INITIAL_TEAM_ID}-{principal_id}"),
                organization_id,
                INITIAL_TEAM_ID,
                principal_id,
                role.as_str(),
                now
            ],
        )
        .map_err(map_sqlite_error)?;
    // Mirrors `team::advance_team_membership_epoch`: a missing or deleted team
    // must fail the whole admission rather than silently skip the epoch bump.
    let epoch_updates = transaction
        .execute(
            "UPDATE groups SET membership_epoch=membership_epoch+1,updated_at=?1
             WHERE organization_id=?2 AND group_id=?3 AND status!='deleted'",
            params![now, organization_id, INITIAL_TEAM_ID],
        )
        .map_err(map_sqlite_error)?;
    if epoch_updates != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    if role == AllowlistRole::Admin {
        transaction
            .execute(
                "INSERT INTO platform_administrators(principal_id,status,authority_epoch,granted_by,created_at,updated_at,revoked_at)
                 VALUES(?1,'active',1,?1,?2,?2,NULL)
                 ON CONFLICT(principal_id) DO UPDATE SET status='active',authority_epoch=platform_administrators.authority_epoch+1,updated_at=excluded.updated_at,revoked_at=NULL",
                params![principal_id, now],
            )
            .map_err(map_sqlite_error)?;
    }
    transaction
        .execute(
            "UPDATE access_metadata SET global_revision=global_revision+1,updated_at=?1 WHERE singleton=1",
            [now],
        )
        .map_err(map_sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json)
             VALUES(?1,?2,NULL,?3,?4,?5,'access.allowlist.provision','team_membership',?6,'allow','allowlist_admission',?7,?8)",
            params![
                format!("allowlist-provision-{}", identity.safe_fingerprint().replace(':', "-")),
                now,
                principal_id,
                organization_id,
                project_id,
                hex::encode(Sha256::digest(format!(
                    "team_membership\0{INITIAL_TEAM_ID}\0{principal_id}"
                ))),
                organization_epoch,
                admitted_by.audit_metadata(role).to_string()
            ],
        )
        .map_err(map_sqlite_error)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(TeamMemberProvisionOutcome::Created)
}

/// What allowlist removal revoked for one identity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AllowlistRevocationOutcome {
    /// The Initial Team membership is now `revoked`.
    pub(crate) team_membership: bool,
    /// The default-Project membership is now `disabled`.
    pub(crate) project_membership: bool,
    /// The platform administrator grant is now `revoked`.
    pub(crate) platform_administrator: bool,
}

impl AllowlistRevocationOutcome {
    pub(crate) const fn revoked_anything(self) -> bool {
        self.team_membership || self.project_membership || self.platform_administrator
    }
}

/// Revoke what allowlist admission granted: the Initial Team membership, the
/// default-Project membership, and platform administrator status — one
/// transaction, idempotent, audited. The Principal row and its identity link
/// are kept so history stays attributable, and re-admission through the
/// allowlist stays blocked by the revoked Initial Team membership. Team
/// `owner` authority is never touched: allowlist admission cannot grant it, so
/// it was granted elsewhere and is removed only through `access` actions.
pub(super) fn revoke_allowlisted(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    revoked_by_fingerprint: &str,
) -> AccessStoreResult<AllowlistRevocationOutcome> {
    let PrincipalLink::External { issuer, subject } = identity.principal_link() else {
        return Err(AccessStoreError::NotAuthorized);
    };
    let project_id = super::bootstrap::PROJECT_ID;
    let now = unix_now()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    // Any link or Principal status qualifies: a suspended identity still names
    // the Principal whose allowlist grants must go.
    let principal: Option<(String, String)> = transaction
        .query_row(
            "SELECT p.principal_id,p.organization_id
             FROM principal_links l JOIN principals p ON p.principal_id=l.principal_id
             WHERE l.link_kind='external' AND l.issuer=?1 AND l.subject=?2",
            params![issuer, subject],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let Some((principal_id, organization_id)) = principal else {
        transaction.commit().map_err(map_sqlite_error)?;
        return Ok(AllowlistRevocationOutcome::default());
    };
    let team_owner: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM team_memberships
             WHERE organization_id=?1 AND team_id=?2 AND principal_id=?3 AND role='owner')",
            params![organization_id, INITIAL_TEAM_ID, principal_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if team_owner {
        transaction.commit().map_err(map_sqlite_error)?;
        return Ok(AllowlistRevocationOutcome::default());
    }
    let team_membership = transaction
        .execute(
            "UPDATE team_memberships SET status='revoked',membership_epoch=membership_epoch+1,updated_at=?1,revoked_at=?1
             WHERE organization_id=?2 AND team_id=?3 AND principal_id=?4 AND status!='revoked'",
            params![now, organization_id, INITIAL_TEAM_ID, principal_id],
        )
        .map_err(map_sqlite_error)?
        == 1;
    let project_membership = transaction
        .execute(
            "UPDATE project_memberships SET status='disabled',updated_at=?1
             WHERE organization_id=?2 AND project_id=?3 AND principal_id=?4
               AND status!='disabled' AND role!='owner'",
            params![now, organization_id, project_id, principal_id],
        )
        .map_err(map_sqlite_error)?
        == 1;
    let platform_administrator = transaction
        .execute(
            "UPDATE platform_administrators SET status='revoked',authority_epoch=authority_epoch+1,updated_at=?1,revoked_at=?1
             WHERE principal_id=?2 AND status!='revoked'",
            params![now, principal_id],
        )
        .map_err(map_sqlite_error)?
        == 1;
    let outcome = AllowlistRevocationOutcome {
        team_membership,
        project_membership,
        platform_administrator,
    };
    if !outcome.revoked_anything() {
        transaction.commit().map_err(map_sqlite_error)?;
        return Ok(outcome);
    }
    if team_membership {
        // Mirrors `team::advance_team_membership_epoch`: a missing or deleted
        // team fails the whole revocation rather than skipping the epoch bump.
        let epoch_updates = transaction
            .execute(
                "UPDATE groups SET membership_epoch=membership_epoch+1,updated_at=?1
                 WHERE organization_id=?2 AND group_id=?3 AND status!='deleted'",
                params![now, organization_id, INITIAL_TEAM_ID],
            )
            .map_err(map_sqlite_error)?;
        if epoch_updates != 1 {
            return Err(AccessStoreError::TeamUnavailable);
        }
    }
    let revision: i64 = transaction
        .query_row(
            "UPDATE access_metadata SET global_revision=global_revision+1,updated_at=?1
             WHERE singleton=1 RETURNING global_revision",
            [now],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let organization_epoch: i64 = transaction
        .query_row(
            "SELECT policy_epoch FROM organizations WHERE organization_id=?1",
            [&organization_id],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    let target_fingerprint = hex::encode(Sha256::digest(format!(
        "team_membership\0{INITIAL_TEAM_ID}\0{principal_id}"
    )));
    transaction
        .execute(
            "INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json)
             VALUES(?1,?2,NULL,?3,?4,?5,'access.allowlist.revoke','team_membership',?6,'allow','allowlist_removal',?7,?8)",
            params![
                format!("allowlist-revoke-{revision}-{}", &target_fingerprint[..16]),
                now,
                principal_id,
                organization_id,
                project_id,
                target_fingerprint,
                organization_epoch,
                serde_json::json!({
                    "revoked_by_fp": revoked_by_fingerprint,
                    "team_membership": team_membership,
                    "project_membership": project_membership,
                    "platform_administrator": platform_administrator,
                })
                .to_string()
            ],
        )
        .map_err(map_sqlite_error)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(outcome)
}

/// Only product-owned admission paths select an initial role; callers cannot
/// request administrative roles or replace an existing membership's role.
#[derive(Clone, Copy)]
enum InitialRole {
    Member,
    Viewer,
    Admin,
}

impl InitialRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Viewer => "viewer",
            Self::Admin => "admin",
        }
    }
}

fn unix_now() -> AccessStoreResult<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .map_err(|_| AccessStoreError::MalformedVocabulary)
}

/// True when the identity already resolves to an active Principal that holds an
/// Initial Team membership in any status.
fn existing_initial_team_member(
    transaction: &rusqlite::Transaction<'_>,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<bool> {
    let PrincipalLink::External { issuer, subject } = identity.principal_link() else {
        return Err(AccessStoreError::NotAuthorized);
    };
    let principal_id: Option<String> = transaction
        .query_row(
            "SELECT l.principal_id
             FROM principal_links l JOIN principals p ON p.principal_id=l.principal_id
             WHERE l.link_kind='external' AND l.issuer=?1 AND l.subject=?2
               AND l.status='active' AND p.status='active'",
            params![issuer, subject],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let Some(principal_id) = principal_id else {
        return Ok(false);
    };
    let existing: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM team_memberships WHERE team_id=?1 AND principal_id=?2",
            params![INITIAL_TEAM_ID, principal_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    Ok(existing.is_some())
}

fn provision_with_role(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    project_id: &str,
    initial_role: InitialRole,
) -> AccessStoreResult<TeamMemberProvisionOutcome> {
    let now = unix_now()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let (outcome, _, _, _) =
        provision_in_transaction(&transaction, identity, project_id, initial_role, now)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(outcome)
}

/// Creates the Principal (if missing) and the default-Project membership on the
/// caller's transaction, so admission paths that add more state stay atomic.
/// Returns the outcome plus the resolved principal, organization, and the
/// organization's policy epoch.
fn provision_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    identity: &VerifiedIdentity,
    project_id: &str,
    initial_role: InitialRole,
    now: i64,
) -> AccessStoreResult<(TeamMemberProvisionOutcome, String, String, i64)> {
    let PrincipalLink::External { issuer, subject } = identity.principal_link() else {
        return Err(AccessStoreError::NotAuthorized);
    };
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
                "INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,?2,'user','active',NULL,?3,?3)",
                params![principal_id, organization_id, now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'external',?3,?4,NULL,'active',1,1,?5,?5)",
                params![link_id, principal_id, issuer, subject, now],
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
            if status == "active"
                && matches!(role.as_str(), "viewer" | "member" | "admin" | "owner") =>
        {
            Ok((
                TeamMemberProvisionOutcome::AlreadyActive,
                principal_id,
                organization_id,
                organization_epoch,
            ))
        }
        Some(_) => Err(AccessStoreError::NotAuthorized),
        None => {
            let role = initial_role.as_str();
            let scoped_fingerprint = scoped_fingerprint(identity, &organization_id, project_id);
            let membership_id = format!("team-membership-{scoped_fingerprint}");
            transaction.execute(
                "INSERT INTO project_memberships(membership_id,organization_id,project_id,principal_id,role,status,created_by,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,'active',?6,?7,?7)",
                params![membership_id, organization_id, project_id, principal_id, role, creator, now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "UPDATE access_metadata SET global_revision=global_revision+1,updated_at=?1 WHERE singleton=1",
                [now],
            ).map_err(map_sqlite_error)?;
            transaction.execute(
                "INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json) VALUES(?1,?2,NULL,?3,?4,?5,'access.team_member.provision','project_membership',?6,'allow','verified_team_admission',?7,?8)",
                params![format!("team-provision-{scoped_fingerprint}"), now, principal_id, organization_id, project_id, scoped_fingerprint, organization_epoch, serde_json::json!({"role": role}).to_string()],
            ).map_err(map_sqlite_error)?;
            Ok((
                TeamMemberProvisionOutcome::Created,
                principal_id,
                organization_id,
                organization_epoch,
            ))
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

    fn allowlist_entry() -> AllowlistAdmission {
        AllowlistAdmission::AllowlistEntry {
            added_by_fingerprint: "fp-of-adding-admin".to_owned(),
        }
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

    async fn viewer_state(store: &AccessStore) -> (String, i64, i64, String, i64) {
        store.with_connection(|connection| {
            connection.query_row(
                "SELECT m.role,m.updated_at,a.global_revision,e.metadata_json,
                        (SELECT count(*) FROM access_audit WHERE action='access.team_member.provision')
                 FROM project_memberships m CROSS JOIN access_metadata a
                 JOIN access_audit e ON e.actor_principal_id=m.principal_id AND e.action='access.team_member.provision'
                 WHERE m.principal_id LIKE 'team-member-%'",
                [],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
            ).map_err(map_sqlite_error)
        }).await.unwrap()
    }

    async fn membership_rows(
        store: &AccessStore,
    ) -> (Vec<(String, String)>, Vec<(String, String)>, i64) {
        store
            .with_connection(|connection| {
                let mut teams = connection
                    .prepare("SELECT team_id, role FROM team_memberships WHERE principal_id LIKE 'team-member-%' ORDER BY team_id")
                    .map_err(map_sqlite_error)?;
                let teams = teams
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(map_sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(map_sqlite_error)?;
                let mut projects = connection
                    .prepare("SELECT project_id, role FROM project_memberships WHERE principal_id LIKE 'team-member-%' ORDER BY project_id")
                    .map_err(map_sqlite_error)?;
                let projects = projects
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(map_sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(map_sqlite_error)?;
                let admins: i64 = connection
                    .query_row(
                        "SELECT count(*) FROM platform_administrators WHERE status='active' AND principal_id LIKE 'team-member-%'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(map_sqlite_error)?;
                Ok((teams, projects, admins))
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn allowlisted_member_gets_team_and_project_membership_once() {
        let (_directory, store) = fixture().await;
        let eli = identity("eli");
        assert_eq!(
            store
                .provision_allowlisted(eli.clone(), AllowlistRole::Member, allowlist_entry())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::Created
        );
        let (teams, projects, admins) = membership_rows(&store).await;
        assert_eq!(
            teams,
            vec![("bootstrap-initial-team".to_owned(), "member".to_owned())]
        );
        assert_eq!(
            projects,
            vec![("bootstrap-default".to_owned(), "member".to_owned())]
        );
        assert_eq!(admins, 0);
        // Repeat sign-in is a no-op and never upgrades the role.
        assert_eq!(
            store
                .provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        assert_eq!(membership_rows(&store).await, (teams, projects, 0));
        let snapshot = store.session_authority(eli).await.unwrap();
        assert!(!snapshot.platform_administrator);
    }

    #[tokio::test]
    async fn allowlisted_admin_is_team_admin_and_platform_admin() {
        let (_directory, store) = fixture().await;
        let eli = identity("eli-admin");
        assert_eq!(
            store
                .provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::Created
        );
        let (teams, projects, admins) = membership_rows(&store).await;
        assert_eq!(
            teams,
            vec![("bootstrap-initial-team".to_owned(), "admin".to_owned())]
        );
        assert_eq!(
            projects,
            vec![("bootstrap-default".to_owned(), "admin".to_owned())]
        );
        assert_eq!(admins, 1);
        // The audit row records who authorized the grant, not only the role.
        let metadata: String = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT metadata_json FROM access_audit WHERE action='access.allowlist.provision'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&metadata).unwrap(),
            serde_json::json!({
                "role": "admin",
                "admitted_via": "allowlist",
                "added_by_fp": "fp-of-adding-admin",
            })
        );
        let snapshot = store.session_authority(eli).await.unwrap();
        assert!(snapshot.platform_administrator);
    }

    async fn audit_count(store: &AccessStore, action: &'static str) -> i64 {
        store
            .with_connection(move |connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM access_audit WHERE action=?1",
                        [action],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap()
    }

    /// Removing the allowlist entry revokes exactly what admission granted, in
    /// one audited step; repeating it changes nothing, and the revoked Initial
    /// Team membership keeps a later allowlist entry from re-admitting the
    /// identity or restoring platform administration.
    #[tokio::test]
    async fn allowlist_revocation_revokes_admin_grants_once_and_blocks_readmission() {
        let (_directory, store) = fixture().await;
        let eli = identity("eli-revoked");
        store
            .provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry())
            .await
            .unwrap();
        let outcome = store
            .revoke_allowlisted(eli.clone(), "fp-of-removing-admin".into())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            AllowlistRevocationOutcome {
                team_membership: true,
                project_membership: true,
                platform_administrator: true,
            }
        );
        let (teams, projects, admins) = membership_rows(&store).await;
        assert_eq!(
            teams,
            vec![("bootstrap-initial-team".to_owned(), "admin".to_owned())],
            "the membership row is kept, only its status changes"
        );
        assert_eq!(
            projects,
            vec![("bootstrap-default".to_owned(), "admin".to_owned())]
        );
        assert_eq!(admins, 0);
        let snapshot = store.session_authority(eli.clone()).await.unwrap();
        assert!(!snapshot.platform_administrator);
        assert!(snapshot.teams.is_empty());
        assert!(snapshot.projects.is_empty());
        let metadata: String = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT metadata_json FROM access_audit WHERE action='access.allowlist.revoke'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&metadata).unwrap(),
            serde_json::json!({
                "revoked_by_fp": "fp-of-removing-admin",
                "team_membership": true,
                "project_membership": true,
                "platform_administrator": true,
            })
        );

        // Idempotent: nothing left to revoke, no second audit row.
        assert_eq!(
            store
                .revoke_allowlisted(eli.clone(), "fp-of-removing-admin".into())
                .await
                .unwrap(),
            AllowlistRevocationOutcome::default()
        );
        assert_eq!(audit_count(&store, "access.allowlist.revoke").await, 1);

        // Re-adding the email later never re-provisions through the allowlist.
        assert_eq!(
            store
                .provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        assert_eq!(membership_rows(&store).await.2, 0);
        assert!(
            !store
                .session_authority(eli)
                .await
                .unwrap()
                .platform_administrator
        );
        assert_eq!(audit_count(&store, "access.allowlist.provision").await, 1);
    }

    /// Allowlist removal cannot reach Team owner authority (admission never
    /// grants it) and is a no-op for an identity with no Principal.
    #[tokio::test]
    async fn allowlist_revocation_never_touches_team_owner_or_unknown_identity() {
        let (_directory, store) = fixture().await;
        let owner = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        assert_eq!(
            store
                .revoke_allowlisted(owner.clone(), "fp".into())
                .await
                .unwrap(),
            AllowlistRevocationOutcome::default()
        );
        let snapshot = store.session_authority(owner).await.unwrap();
        assert!(snapshot.platform_administrator);
        assert!(!snapshot.teams.is_empty());
        assert_eq!(
            store
                .revoke_allowlisted(identity("never-signed-in"), "fp".into())
                .await
                .unwrap(),
            AllowlistRevocationOutcome::default()
        );
        assert_eq!(audit_count(&store, "access.allowlist.revoke").await, 0);
    }

    /// Two session reads racing the same first sign-in must produce one
    /// Principal, one Initial Team membership, one platform grant, and one
    /// audit row (the writer semaphore serializes callers in-process; this
    /// covers two store handles, as two processes would open).
    #[tokio::test]
    async fn concurrent_allowlisted_admission_creates_one_membership_and_audit_record() {
        let (directory, store) = fixture().await;
        let second = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let eli = identity("concurrent-allowlisted");
        let (first, second) = tokio::join!(
            store.provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry()),
            second.provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry()),
        );
        let outcomes = [first.unwrap(), second.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == TeamMemberProvisionOutcome::Created)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == TeamMemberProvisionOutcome::AlreadyActive)
                .count(),
            1
        );
        let (teams, projects, admins) = membership_rows(&store).await;
        assert_eq!(teams.len(), 1);
        assert_eq!(projects.len(), 1);
        assert_eq!(admins, 1);
        assert_eq!(audit_count(&store, "access.allowlist.provision").await, 1);
        let principals: i64 = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM principals WHERE principal_id LIKE 'team-member-%'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(principals, 1);
    }

    /// A `revoked` Initial Team membership — however it was revoked — blocks
    /// re-admission through the allowlist, and a later `admin` entry cannot
    /// use that path to reactivate the membership or grant platform
    /// administration. The gate deliberately has no status filter.
    #[tokio::test]
    async fn revoked_initial_team_membership_blocks_allowlist_readmission() {
        let (_directory, store) = fixture().await;
        let eli = identity("eli-revoked-by-admin");
        store
            .provision_allowlisted(eli.clone(), AllowlistRole::Member, allowlist_entry())
            .await
            .unwrap();
        store
            .execute_test_statement(
                "UPDATE team_memberships SET status='revoked',revoked_at=unixepoch()
                 WHERE team_id='bootstrap-initial-team' AND principal_id LIKE 'team-member-%';",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        let status: String = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT status FROM team_memberships WHERE team_id='bootstrap-initial-team' AND principal_id LIKE 'team-member-%'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(status, "revoked");
        assert_eq!(membership_rows(&store).await.2, 0);
        let snapshot = store.session_authority(eli).await.unwrap();
        assert!(!snapshot.platform_administrator);
        assert!(snapshot.teams.is_empty());
        assert_eq!(audit_count(&store, "access.allowlist.provision").await, 1);
    }

    /// An identity that already holds an Initial Team membership was admitted
    /// before: a later allowlist entry must not rewrite that role or grant
    /// platform administration. The default-Project membership is removed so
    /// the Initial Team membership is the only gate under test.
    #[tokio::test]
    async fn existing_team_member_is_not_upgraded_by_allowlist_admin() {
        let (_directory, store) = fixture().await;
        let eli = identity("eli-existing");
        store
            .provision_team_member(eli.clone(), "bootstrap-default".into())
            .await
            .unwrap();
        store
            .execute_test_statement(
                "INSERT INTO team_memberships(membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,created_by,created_at,updated_at,revoked_at)
                 SELECT 'seeded-team-member','bootstrap-local','bootstrap-initial-team',principal_id,'member','active',1,'bootstrap-owner',2,2,NULL
                 FROM principals WHERE principal_id LIKE 'team-member-%';
                 DELETE FROM project_memberships WHERE principal_id LIKE 'team-member-%';",
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .provision_allowlisted(eli.clone(), AllowlistRole::Admin, allowlist_entry())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        let (teams, _, admins) = membership_rows(&store).await;
        assert_eq!(
            teams,
            vec![("bootstrap-initial-team".to_owned(), "member".to_owned())]
        );
        assert_eq!(admins, 0);
        let snapshot = store.session_authority(eli).await.unwrap();
        assert!(!snapshot.platform_administrator);
    }

    /// A failure after the Principal and Project membership are written must
    /// roll the whole admission back, never leave a half-admitted identity.
    #[tokio::test]
    async fn failed_allowlisted_admission_leaves_no_partial_principal() {
        let (_directory, store) = fixture().await;
        store
            .execute_test_statement(
                "UPDATE groups SET status='deleted',deleted_at=unixepoch() WHERE group_id='bootstrap-initial-team';",
            )
            .await
            .unwrap();
        assert!(
            store
                .provision_allowlisted(
                    identity("rollback"),
                    AllowlistRole::Admin,
                    allowlist_entry()
                )
                .await
                .is_err()
        );
        let principals: i64 = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM principals WHERE principal_id LIKE 'team-member-%'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(principals, 0);
        assert_eq!(membership_rows(&store).await, (vec![], vec![], 0));
    }

    #[tokio::test]
    async fn viewer_first_and_repeat_admission_preserve_epochs_and_audit_role() {
        let (_directory, store) = fixture().await;
        let employee = identity("viewer");
        assert_eq!(
            store
                .provision_team_viewer(employee.clone(), "bootstrap-default".into())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::Created
        );
        let first = viewer_state(&store).await;
        assert_eq!(first.0, "viewer");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&first.3).unwrap(),
            serde_json::json!({"role":"viewer"})
        );
        assert_eq!(first.4, 1);
        assert_eq!(
            store
                .provision_team_viewer(employee.clone(), "bootstrap-default".into())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        // The older MCP member admission must not upgrade an existing Viewer.
        assert_eq!(
            store
                .provision_team_member(employee.clone(), "bootstrap-default".into())
                .await
                .unwrap(),
            TeamMemberProvisionOutcome::AlreadyActive
        );
        assert_eq!(viewer_state(&store).await, first);
        for permission in [Permission::AssetDiscover, Permission::ArtifactPublish] {
            assert!(
                store
                    .authorize_project(crate::access::AuthorizeProjectInput::new(
                        employee.clone(),
                        "bootstrap-default",
                        permission
                    ))
                    .await
                    .is_ok()
            );
        }
        for permission in [Permission::AssetUse, Permission::ProjectManage] {
            assert!(
                store
                    .authorize_project(crate::access::AuthorizeProjectInput::new(
                        employee.clone(),
                        "bootstrap-default",
                        permission
                    ))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn concurrent_viewer_admission_creates_one_membership_and_audit_record() {
        let (directory, store) = fixture().await;
        let second = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let employee = identity("concurrent-viewer");
        let (first, second) = tokio::join!(
            store.provision_team_viewer(employee.clone(), "bootstrap-default".into()),
            second.provision_team_viewer(employee, "bootstrap-default".into()),
        );
        let outcomes = [first.unwrap(), second.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == TeamMemberProvisionOutcome::Created)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == TeamMemberProvisionOutcome::AlreadyActive)
                .count(),
            1
        );
        assert_eq!(viewer_state(&store).await.4, 1);
    }

    #[tokio::test]
    async fn viewer_admission_preserves_every_existing_active_role_without_mutation() {
        let (_directory, store) = fixture().await;
        let employee = identity("existing-role");
        store
            .provision_team_viewer(employee.clone(), "bootstrap-default".into())
            .await
            .unwrap();
        for statement in [
            "UPDATE project_memberships SET role='owner' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE project_memberships SET role='admin' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE project_memberships SET role='member' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE project_memberships SET role='viewer' WHERE principal_id LIKE 'team-member-%'",
        ] {
            store.execute_test_statement(statement).await.unwrap();
            let before = viewer_state(&store).await;
            assert_eq!(
                store
                    .provision_team_viewer(employee.clone(), "bootstrap-default".into())
                    .await
                    .unwrap(),
                TeamMemberProvisionOutcome::AlreadyActive
            );
            assert_eq!(viewer_state(&store).await, before);
        }
    }

    #[tokio::test]
    async fn viewer_admission_never_reactivates_disabled_or_revoked_authority() {
        for statement in [
            "UPDATE project_memberships SET status='disabled' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE project_memberships SET status='suspended' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE principals SET status='disabled' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE principals SET status='suspended' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE principal_links SET status='revoked' WHERE principal_id LIKE 'team-member-%'",
            "UPDATE projects SET status='suspended'",
            "UPDATE organizations SET status='disabled'",
        ] {
            let (_directory, store) = fixture().await;
            let employee = identity("revoked-viewer");
            store
                .provision_team_viewer(employee.clone(), "bootstrap-default".into())
                .await
                .unwrap();
            store.execute_test_statement(statement).await.unwrap();
            let before = viewer_state(&store).await;
            assert!(
                store
                    .provision_team_viewer(employee, "bootstrap-default".into())
                    .await
                    .is_err()
            );
            assert_eq!(viewer_state(&store).await, before);
        }
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
