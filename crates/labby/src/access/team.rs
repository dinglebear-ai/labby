use labby_auth::VerifiedIdentity;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::domain::TeamRole;
use super::error::{AccessStoreError, AccessStoreResult};
use super::read::resolve_principal;
use super::store::map_sqlite_error;

const MAX_NAME: usize = 128;
const MAX_ID: usize = 128;

#[derive(Clone, Debug)]
pub(crate) struct CreateTeamInput {
    actor: VerifiedIdentity,
    team_id: String,
    name: String,
}

impl CreateTeamInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        name: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let name = name.into();
        if !valid_id(&team_id) || !valid_name(&name) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            name,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AddTeamMemberInput {
    actor: VerifiedIdentity,
    team_id: String,
    principal_id: String,
    role: TeamRole,
}

impl AddTeamMemberInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        principal_id: impl Into<String>,
        role: TeamRole,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let principal_id = principal_id.into();
        if !valid_id(&team_id) || !valid_id(&principal_id) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            principal_id,
            role,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TeamMembershipInput {
    actor: VerifiedIdentity,
    team_id: String,
    principal_id: String,
}

impl TeamMembershipInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        team_id: impl Into<String>,
        principal_id: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let team_id = team_id.into();
        let principal_id = principal_id.into();
        if !valid_id(&team_id) || !valid_id(&principal_id) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            team_id,
            principal_id,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlatformAdministratorInput {
    actor: VerifiedIdentity,
    principal_id: String,
}

impl PlatformAdministratorInput {
    pub(crate) fn new(
        actor: VerifiedIdentity,
        principal_id: impl Into<String>,
    ) -> AccessStoreResult<Self> {
        let principal_id = principal_id.into();
        if !valid_id(&principal_id) {
            return Err(AccessStoreError::InvalidTeamInput);
        }
        Ok(Self {
            actor,
            principal_id,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamSnapshot {
    pub(crate) organization_id: String,
    pub(crate) team_id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) role: Option<TeamRole>,
    pub(crate) policy_epoch: u64,
    pub(crate) membership_epoch: u64,
    pub(crate) global_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TeamMembershipSnapshot {
    pub(crate) organization_id: String,
    pub(crate) team_id: String,
    pub(crate) principal_id: String,
    pub(crate) role: TeamRole,
    pub(crate) status: String,
    pub(crate) membership_epoch: u64,
}

pub(super) fn create_team(
    connection: &mut Connection,
    input: &CreateTeamInput,
) -> AccessStoreResult<TeamSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_platform_admin(&tx, &actor.id)?;
    let now = unix_now()?;
    tx.execute(
        "INSERT INTO groups(group_id,organization_id,kind,name,status,policy_epoch,membership_epoch,created_by,created_at,updated_at,deleted_at) VALUES(?1,?2,'team',?3,'active',1,1,?4,?5,?5,NULL)",
        params![input.team_id, actor.organization_id, input.name, actor.id, now],
    ).map_err(map_sqlite_error)?;
    tx.execute(
        "INSERT INTO team_memberships(membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,created_by,created_at,updated_at,revoked_at) VALUES(?1,?2,?3,?4,'owner','active',1,?4,?5,?5,NULL)",
        params![format!("team-owner-{}-{}", input.team_id, actor.id), actor.organization_id, input.team_id, actor.id, now],
    ).map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team.create",
        "team",
        &input.team_id,
        1,
        "platform_admin",
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(TeamSnapshot {
        organization_id: actor.organization_id,
        team_id: input.team_id.clone(),
        name: input.name.clone(),
        status: "active".into(),
        role: Some(TeamRole::Owner),
        policy_epoch: 1,
        membership_epoch: 1,
        global_revision: revision,
    })
}

pub(super) fn list_teams(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
) -> AccessStoreResult<Vec<TeamSnapshot>> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let actor = resolve_principal(&tx, identity)?;
    let platform = is_platform_admin(&tx, &actor.id)?;
    let revision = global_revision(&tx)?;
    let sql = if platform {
        "SELECT g.organization_id,g.group_id,g.name,g.status,NULL,g.policy_epoch,g.membership_epoch FROM groups g WHERE g.kind='team' AND g.status!='deleted' ORDER BY g.organization_id COLLATE BINARY,g.group_id COLLATE BINARY"
    } else {
        "SELECT g.organization_id,g.group_id,g.name,g.status,m.role,g.policy_epoch,g.membership_epoch FROM team_memberships m JOIN groups g ON g.organization_id=m.organization_id AND g.group_id=m.team_id WHERE m.organization_id=?1 AND m.principal_id=?2 AND m.status='active' AND g.kind='team' AND g.status!='deleted' ORDER BY g.group_id COLLATE BINARY"
    };
    let mut statement = tx.prepare(sql).map_err(map_sqlite_error)?;
    let mut snapshots = Vec::new();
    if platform {
        let rows = statement
            .query_map([], team_row)
            .map_err(map_sqlite_error)?;
        for row in rows {
            snapshots.push(snapshot(row.map_err(map_sqlite_error)?, revision)?);
        }
    } else {
        let rows = statement
            .query_map(params![actor.organization_id, actor.id], team_row)
            .map_err(map_sqlite_error)?;
        for row in rows {
            snapshots.push(snapshot(row.map_err(map_sqlite_error)?, revision)?);
        }
    }
    drop(statement);
    tx.commit().map_err(map_sqlite_error)?;
    Ok(snapshots)
}

fn team_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, String, Option<String>, i64, i64)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn snapshot(
    row: (String, String, String, String, Option<String>, i64, i64),
    revision: u64,
) -> AccessStoreResult<TeamSnapshot> {
    let role = match row.4.as_deref() {
        Some(value) => {
            Some(TeamRole::from_persisted(value).ok_or(AccessStoreError::MalformedVocabulary)?)
        }
        None => None,
    };
    Ok(TeamSnapshot {
        organization_id: row.0,
        team_id: row.1,
        name: row.2,
        status: row.3,
        role,
        policy_epoch: epoch(row.5)?,
        membership_epoch: epoch(row.6)?,
        global_revision: revision,
    })
}

pub(super) fn add_member(
    connection: &mut Connection,
    input: &AddTeamMemberInput,
) -> AccessStoreResult<TeamMembershipSnapshot> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, &input.team_id)?;
    require_principal_in_organization(&tx, &input.principal_id, &actor.organization_id)?;
    let now = unix_now()?;
    let existing: Option<String> = tx.query_row("SELECT status FROM team_memberships WHERE organization_id=?1 AND team_id=?2 AND principal_id=?3", params![actor.organization_id,input.team_id,input.principal_id], |row| row.get(0)).optional().map_err(map_sqlite_error)?;
    if existing.is_some() {
        return Err(AccessStoreError::TeamUnavailable);
    }
    tx.execute("INSERT INTO team_memberships(membership_id,organization_id,team_id,principal_id,role,status,membership_epoch,created_by,created_at,updated_at,revoked_at) VALUES(?1,?2,?3,?4,?5,'active',1,?6,?7,?7,NULL)", params![format!("team-member-{}-{}",input.team_id,input.principal_id),actor.organization_id,input.team_id,input.principal_id,input.role.as_persisted(),actor.id,now]).map_err(map_sqlite_error)?;
    let membership_epoch =
        advance_team_membership_epoch(&tx, &actor.organization_id, &input.team_id, now)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team_member.add",
        "team_membership",
        &input.principal_id,
        membership_epoch,
        "team_manage",
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(TeamMembershipSnapshot {
        organization_id: actor.organization_id,
        team_id: input.team_id.clone(),
        principal_id: input.principal_id.clone(),
        role: input.role,
        status: "active".into(),
        membership_epoch: 1,
    })
}

pub(super) fn suspend_team(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
) -> AccessStoreResult<()> {
    mutate_team_status(connection, identity, team_id, "suspended")
}

pub(super) fn activate_team(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
) -> AccessStoreResult<()> {
    mutate_team_status(connection, identity, team_id, "active")
}

fn mutate_team_status(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
    status: &str,
) -> AccessStoreResult<()> {
    if !valid_id(team_id) {
        return Err(AccessStoreError::InvalidTeamInput);
    }
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, identity)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, team_id)?;
    let now = unix_now()?;
    let changed = tx.execute("UPDATE groups SET status=?1,policy_epoch=policy_epoch+1,updated_at=?2 WHERE organization_id=?3 AND group_id=?4 AND kind='team' AND status!=?1 AND status!='deleted'", params![status,now,actor.organization_id,team_id]).map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let policy_epoch: i64 = tx
        .query_row(
            "SELECT policy_epoch FROM groups WHERE organization_id=?1 AND group_id=?2",
            params![actor.organization_id, team_id],
            |r| r.get(0),
        )
        .map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        "access.team.status",
        "team",
        team_id,
        epoch(policy_epoch)?,
        status,
    )?;
    tx.commit().map_err(map_sqlite_error)
}

pub(super) fn set_member_role(
    connection: &mut Connection,
    input: &AddTeamMemberInput,
) -> AccessStoreResult<()> {
    mutate_member(
        connection,
        &input.actor,
        &input.team_id,
        &input.principal_id,
        Some(input.role),
        None,
        "access.team_member.role",
    )
}

pub(super) fn suspend_member(
    connection: &mut Connection,
    input: &TeamMembershipInput,
) -> AccessStoreResult<()> {
    mutate_member(
        connection,
        &input.actor,
        &input.team_id,
        &input.principal_id,
        None,
        Some("suspended"),
        "access.team_member.suspend",
    )
}

pub(super) fn remove_member(
    connection: &mut Connection,
    input: &TeamMembershipInput,
) -> AccessStoreResult<()> {
    mutate_member(
        connection,
        &input.actor,
        &input.team_id,
        &input.principal_id,
        None,
        Some("revoked"),
        "access.team_member.remove",
    )
}

fn mutate_member(
    connection: &mut Connection,
    identity: &VerifiedIdentity,
    team_id: &str,
    principal_id: &str,
    role: Option<TeamRole>,
    status: Option<&str>,
    action: &str,
) -> AccessStoreResult<()> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, identity)?;
    require_team_manager(&tx, &actor.id, &actor.organization_id, team_id)?;
    protect_last_owner(
        &tx,
        &actor.organization_id,
        team_id,
        principal_id,
        role,
        status,
    )?;
    let now = unix_now()?;
    let changed=match (role,status) {
        (Some(role),None)=>tx.execute("UPDATE team_memberships SET role=?1,membership_epoch=membership_epoch+1,updated_at=?2 WHERE organization_id=?3 AND team_id=?4 AND principal_id=?5 AND status!='revoked' AND role!=?1",params![role.as_persisted(),now,actor.organization_id,team_id,principal_id]),
        (None,Some("revoked"))=>tx.execute("UPDATE team_memberships SET status='revoked',membership_epoch=membership_epoch+1,updated_at=?1,revoked_at=?1 WHERE organization_id=?2 AND team_id=?3 AND principal_id=?4 AND status!='revoked'",params![now,actor.organization_id,team_id,principal_id]),
        (None,Some(value))=>tx.execute("UPDATE team_memberships SET status=?1,membership_epoch=membership_epoch+1,updated_at=?2,revoked_at=NULL WHERE organization_id=?3 AND team_id=?4 AND principal_id=?5 AND status!=?1 AND status!='revoked'",params![value,now,actor.organization_id,team_id,principal_id]),
        _=>return Err(AccessStoreError::InvalidTeamInput),
    }.map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let team_epoch = advance_team_membership_epoch(&tx, &actor.organization_id, team_id, now)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        action,
        "team_membership",
        principal_id,
        team_epoch,
        "team_manage",
    )?;
    tx.commit().map_err(map_sqlite_error)
}

pub(super) fn grant_platform_admin(
    connection: &mut Connection,
    input: &PlatformAdministratorInput,
) -> AccessStoreResult<()> {
    mutate_platform_admin(connection, input, "active", "access.platform_admin.grant")
}
pub(super) fn revoke_platform_admin(
    connection: &mut Connection,
    input: &PlatformAdministratorInput,
) -> AccessStoreResult<()> {
    mutate_platform_admin(connection, input, "revoked", "access.platform_admin.revoke")
}

fn mutate_platform_admin(
    connection: &mut Connection,
    input: &PlatformAdministratorInput,
    status: &str,
    action: &str,
) -> AccessStoreResult<()> {
    let tx = immediate(connection)?;
    let actor = resolve_principal(&tx, &input.actor)?;
    require_platform_admin(&tx, &actor.id)?;
    require_principal_in_organization(&tx, &input.principal_id, &actor.organization_id)?;
    if status == "revoked" && input.principal_id == actor.id {
        return Err(AccessStoreError::NotAuthorized);
    }
    let now = unix_now()?;
    tx.execute("INSERT INTO platform_administrators(principal_id,status,authority_epoch,granted_by,created_at,updated_at,revoked_at) VALUES(?1,?2,1,?3,?4,?4,CASE WHEN ?2='revoked' THEN ?4 ELSE NULL END) ON CONFLICT(principal_id) DO UPDATE SET status=excluded.status,authority_epoch=platform_administrators.authority_epoch+1,updated_at=excluded.updated_at,revoked_at=excluded.revoked_at",params![input.principal_id,status,actor.id,now]).map_err(map_sqlite_error)?;
    let revision = advance_global_revision(&tx, now)?;
    audit(
        &tx,
        revision,
        now,
        &actor.id,
        &actor.organization_id,
        action,
        "principal",
        &input.principal_id,
        revision,
        status,
    )?;
    tx.commit().map_err(map_sqlite_error)
}

fn protect_last_owner(
    tx: &Transaction<'_>,
    organization_id: &str,
    team_id: &str,
    principal_id: &str,
    new_role: Option<TeamRole>,
    new_status: Option<&str>,
) -> AccessStoreResult<()> {
    let current:Option<(String,String)>=tx.query_row("SELECT role,status FROM team_memberships WHERE organization_id=?1 AND team_id=?2 AND principal_id=?3",params![organization_id,team_id,principal_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(map_sqlite_error)?;
    if current
        .as_ref()
        .is_some_and(|(role, status)| role == "owner" && status == "active")
        && (new_role.is_some_and(|role| role != TeamRole::Owner)
            || new_status.is_some_and(|status| status != "active"))
    {
        let owners:i64=tx.query_row("SELECT count(*) FROM team_memberships WHERE organization_id=?1 AND team_id=?2 AND role='owner' AND status='active'",params![organization_id,team_id],|r|r.get(0)).map_err(map_sqlite_error)?;
        if owners <= 1 {
            return Err(AccessStoreError::LastActiveTeamOwner);
        }
    }
    Ok(())
}

fn require_platform_admin(tx: &Transaction<'_>, principal_id: &str) -> AccessStoreResult<()> {
    if is_platform_admin(tx, principal_id)? {
        Ok(())
    } else {
        Err(AccessStoreError::NotAuthorized)
    }
}
fn is_platform_admin(tx: &Transaction<'_>, principal_id: &str) -> AccessStoreResult<bool> {
    tx.query_row("SELECT EXISTS(SELECT 1 FROM platform_administrators WHERE principal_id=?1 AND status='active')",[principal_id],|r|r.get(0)).map_err(map_sqlite_error)
}
fn require_team_manager(
    tx: &Transaction<'_>,
    principal_id: &str,
    organization_id: &str,
    team_id: &str,
) -> AccessStoreResult<()> {
    if is_platform_admin(tx, principal_id)? {
        return Ok(());
    }
    let allowed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM team_memberships m JOIN groups g ON g.organization_id=m.organization_id AND g.group_id=m.team_id WHERE m.organization_id=?1 AND m.team_id=?2 AND m.principal_id=?3 AND m.status='active' AND m.role IN ('owner','admin') AND g.status='active')",params![organization_id,team_id,principal_id],|r|r.get(0)).map_err(map_sqlite_error)?;
    if allowed {
        Ok(())
    } else {
        Err(AccessStoreError::NotAuthorized)
    }
}
fn require_principal_in_organization(
    tx: &Transaction<'_>,
    principal_id: &str,
    organization_id: &str,
) -> AccessStoreResult<()> {
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM principals WHERE principal_id=?1 AND organization_id=?2 AND status='active')",params![principal_id,organization_id],|r|r.get(0)).map_err(map_sqlite_error)?;
    if exists {
        Ok(())
    } else {
        Err(AccessStoreError::TeamUnavailable)
    }
}
fn advance_team_membership_epoch(
    tx: &Transaction<'_>,
    organization_id: &str,
    team_id: &str,
    now: i64,
) -> AccessStoreResult<u64> {
    let changed=tx.execute("UPDATE groups SET membership_epoch=membership_epoch+1,updated_at=?1 WHERE organization_id=?2 AND group_id=?3 AND status!='deleted'",params![now,organization_id,team_id]).map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamUnavailable);
    }
    let value: i64 = tx
        .query_row(
            "SELECT membership_epoch FROM groups WHERE organization_id=?1 AND group_id=?2",
            params![organization_id, team_id],
            |r| r.get(0),
        )
        .map_err(map_sqlite_error)?;
    epoch(value)
}
fn advance_global_revision(tx: &Transaction<'_>, now: i64) -> AccessStoreResult<u64> {
    tx.execute("UPDATE access_metadata SET global_revision=global_revision+1,updated_at=?1 WHERE singleton=1",[now]).map_err(map_sqlite_error)?;
    global_revision(tx)
}
fn global_revision(tx: &Transaction<'_>) -> AccessStoreResult<u64> {
    let value: i64 = tx
        .query_row(
            "SELECT global_revision FROM access_metadata WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(map_sqlite_error)?;
    epoch(value)
}
fn epoch(value: i64) -> AccessStoreResult<u64> {
    u64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary)
}
fn audit(
    tx: &Transaction<'_>,
    revision: u64,
    now: i64,
    actor: &str,
    organization: &str,
    action: &str,
    target_kind: &str,
    target: &str,
    policy_epoch: u64,
    reason: &str,
) -> AccessStoreResult<()> {
    tx.execute("INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,organization_id,project_id,action,target_kind,target_fingerprint,decision,reason_code,policy_epoch,metadata_json) VALUES(?1,?2,NULL,?3,?4,NULL,?5,?6,?7,'allow',?8,?9,'{}')",params![format!("team-authority-{revision}"),now,actor,organization,action,target_kind,target,reason,i64::try_from(policy_epoch).map_err(|_|AccessStoreError::MalformedVocabulary)?]).map_err(map_sqlite_error)?;
    Ok(())
}
fn immediate(connection: &mut Connection) -> AccessStoreResult<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)
}
fn unix_now() -> AccessStoreResult<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .map_err(|e| AccessStoreError::Unavailable(e.to_string()))
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NAME
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::*;
    use crate::access::{BootstrapOwnerInput, store::AccessStore};

    fn identity(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn store() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
        let directory = super::super::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = identity("owner");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        (directory, store, owner)
    }

    async fn seed_principal(store: &AccessStore, principal_id: &str, subject: &str) {
        let principal_id = principal_id.to_owned();
        let subject = subject.to_owned();
        store
            .with_connection(move |connection| {
                connection
                    .execute(
                        "INSERT INTO principals(principal_id,organization_id,kind,status,display_name,created_at,updated_at) VALUES(?1,'bootstrap-local','user','active',NULL,2,2)",
                        [&principal_id],
                    )
                    .map_err(map_sqlite_error)?;
                connection
                    .execute(
                        "INSERT INTO principal_links(link_id,principal_id,link_kind,issuer,subject,credential_id,status,verification_generation,link_generation,created_at,updated_at) VALUES(?1,?2,'external','https://accounts.google.com',?3,NULL,'active',1,1,2,2)",
                        params![format!("link-{principal_id}"), principal_id, subject],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn verified_bootstrap_binds_explicit_platform_admin_and_initial_team() {
        let (_directory, store, owner) = store().await;
        let teams = store.list_teams(owner).await.unwrap();
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].team_id, "bootstrap-initial-team");
        assert_eq!(teams[0].status, "active");
        assert_eq!(teams[0].policy_epoch, 1);
        assert_eq!(teams[0].membership_epoch, 1);

        let counts = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT (SELECT count(*) FROM platform_administrators WHERE status='active'),(SELECT count(*) FROM access_audit WHERE action IN ('access.platform_admin.bootstrap','access.team.bootstrap'))",
                        [],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(counts, (1, 2));
    }

    #[tokio::test]
    async fn platform_admin_creates_team_and_member_lists_only_its_teams() {
        let (_directory, store, owner) = store().await;
        let member = identity("member");
        seed_principal(&store, "member-principal", "member").await;

        let created = store
            .create_team(CreateTeamInput::new(owner.clone(), "team-a", "Team A").unwrap())
            .await
            .unwrap();
        assert_eq!(created.role, Some(TeamRole::Owner));

        store
            .add_team_member(
                AddTeamMemberInput::new(owner, "team-a", "member-principal", TeamRole::Member)
                    .unwrap(),
            )
            .await
            .unwrap();

        let visible = store.list_teams(member).await.unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].team_id, "team-a");
        assert_eq!(visible[0].role, Some(TeamRole::Member));
    }

    #[tokio::test]
    async fn regular_member_cannot_manage_team_but_team_admin_can() {
        let (_directory, store, owner) = store().await;
        let admin = identity("admin");
        let member = identity("member");
        seed_principal(&store, "admin-principal", "admin").await;
        seed_principal(&store, "member-principal", "member").await;
        store
            .create_team(CreateTeamInput::new(owner.clone(), "team-a", "Team A").unwrap())
            .await
            .unwrap();
        store
            .add_team_member(
                AddTeamMemberInput::new(
                    owner.clone(),
                    "team-a",
                    "admin-principal",
                    TeamRole::Admin,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .add_team_member(
                AddTeamMemberInput::new(owner, "team-a", "member-principal", TeamRole::Member)
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(matches!(
            store.suspend_team(member, "team-a".into()).await,
            Err(AccessStoreError::NotAuthorized)
        ));
        store.suspend_team(admin, "team-a".into()).await.unwrap();
    }

    #[tokio::test]
    async fn serialized_transaction_protects_last_active_owner() {
        let (_directory, store, owner) = store().await;
        let demote = AddTeamMemberInput::new(
            owner,
            "bootstrap-initial-team",
            "bootstrap-owner",
            TeamRole::Admin,
        )
        .unwrap();
        assert!(matches!(
            store.set_team_member_role(demote).await,
            Err(AccessStoreError::LastActiveTeamOwner)
        ));

        let state = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT role,status,membership_epoch FROM team_memberships WHERE membership_id='bootstrap-initial-team-owner'",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(state, ("owner".into(), "active".into(), 1));
    }

    #[tokio::test]
    async fn audit_failure_rolls_back_team_and_epoch_mutation() {
        let (_directory, store, owner) = store().await;
        store
            .with_connection(|connection| {
                connection.execute_batch("CREATE TEMP TRIGGER fail_team_audit BEFORE INSERT ON access_audit WHEN NEW.action='access.team.create' BEGIN SELECT RAISE(ABORT,'forced'); END;").map_err(map_sqlite_error)
            })
            .await
            .unwrap();

        assert!(
            store
                .create_team(CreateTeamInput::new(owner, "rolled-back", "Rolled Back").unwrap())
                .await
                .is_err()
        );
        let state = store
            .with_connection(|connection| {
                connection.query_row("SELECT (SELECT count(*) FROM groups WHERE group_id='rolled-back'),(SELECT global_revision FROM access_metadata WHERE singleton=1)",[],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?))).map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(state, (0, 1));
    }
}
