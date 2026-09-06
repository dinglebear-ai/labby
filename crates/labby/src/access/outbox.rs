use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::{Value, json};

use super::error::{AccessStoreError, AccessStoreResult};
use super::store::map_sqlite_error;

pub(crate) const OUTBOX_BATCH_LIMIT: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingProjection {
    pub(crate) organization_id: String,
    pub(crate) sequence: u64,
    pub(crate) payload_json: String,
    pub(crate) previous_digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AuthoritySnapshotRecord {
    pub(crate) resource_type: String,
    pub(crate) resource_id: String,
    pub(crate) value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AuthoritySnapshotCheckpoint {
    pub(crate) records: Vec<AuthoritySnapshotRecord>,
    /// Last local mutation included in the same SQLite view as `records`.
    pub(crate) outbox_cutoff: Option<u64>,
}

pub(super) fn snapshot(
    connection: &Connection,
    organization_id: &str,
) -> AccessStoreResult<Vec<AuthoritySnapshotRecord>> {
    let mut records = Vec::new();
    let mut administrators = connection
        .prepare("SELECT p.principal_id,p.status,a.status,a.authority_epoch FROM platform_administrators a JOIN principals p ON p.principal_id=a.principal_id WHERE p.organization_id=?1 AND a.status!='revoked' ORDER BY p.principal_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    for row in administrators
        .query_map([organization_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(map_sqlite_error)?
    {
        let (principal_id, principal_status, status, authority_epoch) =
            row.map_err(map_sqlite_error)?;
        records.push(AuthoritySnapshotRecord {
            resource_type: "principal".into(),
            resource_id: principal_id,
            value: json!({"principal_status":principal_status,"status":status,"authority_epoch":authority_epoch}),
        });
    }
    drop(administrators);
    let mut teams = connection
        .prepare("SELECT group_id,status,policy_epoch,membership_epoch FROM groups WHERE organization_id=?1 AND kind='team' AND status!='deleted' ORDER BY group_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    for row in teams
        .query_map([organization_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(map_sqlite_error)?
    {
        let (team_id, status, policy_epoch, membership_epoch) = row.map_err(map_sqlite_error)?;
        records.push(AuthoritySnapshotRecord {
            resource_type: "team".into(),
            resource_id: team_id,
            value: json!({"status":status,"policy_epoch":policy_epoch,"membership_epoch":membership_epoch}),
        });
    }
    drop(teams);

    let mut memberships = connection
        .prepare("SELECT team_id,principal_id,role,status,membership_epoch FROM team_memberships WHERE organization_id=?1 AND status!='revoked' ORDER BY team_id COLLATE BINARY,principal_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    for row in memberships
        .query_map([organization_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(map_sqlite_error)?
    {
        let (team_id, principal_id, role, status, membership_epoch) =
            row.map_err(map_sqlite_error)?;
        records.push(AuthoritySnapshotRecord {
            resource_type: "team_membership".into(),
            resource_id: format!("{team_id}\u{0}{principal_id}"),
            value: json!({"team_id":team_id,"principal_id":principal_id,"role":role,"status":status,"membership_epoch":membership_epoch}),
        });
    }
    drop(memberships);

    let mut assignments = connection
        .prepare("SELECT team_id,project_id,role,status,assignment_epoch FROM team_project_assignments WHERE organization_id=?1 AND status='active' ORDER BY team_id COLLATE BINARY,project_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    for row in assignments
        .query_map([organization_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(map_sqlite_error)?
    {
        let (team_id, project_id, role, status, assignment_epoch) =
            row.map_err(map_sqlite_error)?;
        records.push(AuthoritySnapshotRecord {
            resource_type: "team_project".into(),
            resource_id: format!("{team_id}\u{0}{project_id}"),
            value: json!({"team_id":team_id,"project_id":project_id,"role":role,"status":status,"assignment_epoch":assignment_epoch}),
        });
    }
    Ok(records)
}

pub(super) fn snapshot_checkpoint(
    connection: &mut Connection,
    organization_id: &str,
) -> AccessStoreResult<AuthoritySnapshotCheckpoint> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let records = snapshot(&tx, organization_id)?;
    let cutoff = tx
        .query_row(
            "SELECT MAX(sequence) FROM authority_projection_outbox WHERE organization_id=?1",
            [organization_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(map_sqlite_error)?
        .map(|value| u64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary))
        .transpose()?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(AuthoritySnapshotCheckpoint {
        records,
        outbox_cutoff: cutoff,
    })
}

pub(super) fn organizations(connection: &mut Connection) -> AccessStoreResult<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT organization_id FROM organizations WHERE status='active' ORDER BY organization_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(map_sqlite_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)
}

pub(super) fn claim(
    connection: &mut Connection,
    now: i64,
    limit: usize,
) -> AccessStoreResult<Vec<PendingProjection>> {
    let limit = limit.clamp(1, OUTBOX_BATCH_LIMIT);
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let mut rows = Vec::new();
    {
        let mut statement = tx.prepare("SELECT o.organization_id,o.sequence,o.payload_json,(SELECT p.envelope_digest FROM authority_projection_outbox p WHERE p.organization_id=o.organization_id AND p.sequence<o.sequence AND p.status='sent' ORDER BY p.sequence DESC LIMIT 1) FROM authority_projection_outbox o WHERE o.status IN ('pending','inflight') AND o.next_attempt_at<=?1 ORDER BY o.organization_id COLLATE BINARY,o.sequence LIMIT ?2").map_err(map_sqlite_error)?;
        let selected = statement
            .query_map(params![now, i64::try_from(limit).unwrap_or(256)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(map_sqlite_error)?;
        for row in selected {
            let (organization_id, sequence, payload_json, previous_digest) =
                row.map_err(map_sqlite_error)?;
            rows.push(PendingProjection {
                organization_id,
                sequence: u64::try_from(sequence)
                    .map_err(|_| AccessStoreError::MalformedVocabulary)?,
                payload_json,
                previous_digest,
            });
        }
    }
    for row in &rows {
        tx.execute("UPDATE authority_projection_outbox SET status='inflight',attempt_count=attempt_count+1,next_attempt_at=?3 WHERE organization_id=?1 AND sequence=?2",params![row.organization_id,i64::try_from(row.sequence).map_err(|_|AccessStoreError::MalformedVocabulary)?,now.saturating_add(30)]).map_err(map_sqlite_error)?;
    }
    tx.commit().map_err(map_sqlite_error)?;
    Ok(rows)
}

pub(super) fn acknowledge(
    connection: &mut Connection,
    organization_id: &str,
    highest: u64,
    digest: &str,
    now: i64,
) -> AccessStoreResult<usize> {
    if organization_id.trim().is_empty()
        || !(digest.len() == 64 || (digest.len() == 71 && digest.starts_with("sha256:")))
    {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    let highest = i64::try_from(highest).map_err(|_| AccessStoreError::MalformedVocabulary)?;
    connection.execute("UPDATE authority_projection_outbox SET status='sent',sent_at=?3,envelope_digest=?4 WHERE organization_id=?1 AND sequence<=?2 AND status='inflight'",params![organization_id,highest,now,digest]).map_err(map_sqlite_error)
}

pub(super) fn release_failed(
    connection: &mut Connection,
    organization_id: &str,
    through: u64,
    now: i64,
) -> AccessStoreResult<usize> {
    let through = i64::try_from(through).map_err(|_| AccessStoreError::MalformedVocabulary)?;
    connection.execute("UPDATE authority_projection_outbox SET status='pending',next_attempt_at=?3+MIN(3600,30*(1<<MIN(attempt_count,7))) WHERE organization_id=?1 AND sequence<=?2 AND status='inflight'",params![organization_id,through,now]).map_err(map_sqlite_error)
}

pub(super) fn retain(connection: &mut Connection, older_than: i64) -> AccessStoreResult<usize> {
    connection
        .execute(
            "DELETE FROM authority_projection_outbox WHERE status='sent' AND sent_at<?1",
            [older_than],
        )
        .map_err(map_sqlite_error)
}

pub(super) fn supersede_with_snapshot(
    connection: &mut Connection,
    organization_id: &str,
    digest: &str,
    through: u64,
    now: i64,
) -> AccessStoreResult<usize> {
    connection
        .execute(
            "UPDATE authority_projection_outbox SET status='sent',sent_at=?3,envelope_digest=?4 WHERE organization_id=?1 AND sequence<=?2 AND status IN ('pending','inflight')",
            params![organization_id, i64::try_from(through).map_err(|_| AccessStoreError::MalformedVocabulary)?, now, digest],
        )
        .map_err(map_sqlite_error)
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use crate::access::{AccessStore, BootstrapOwnerInput};

    #[tokio::test]
    async fn snapshot_uses_typed_collision_safe_team_records() {
        let directory = crate::access::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(identity, "Local", "Default").unwrap())
            .await
            .unwrap();

        let records = store
            .authority_snapshot("bootstrap-local".into())
            .await
            .unwrap();
        assert!(records.iter().any(|record| {
            record.resource_type == "team"
                && record.resource_id == "bootstrap-initial-team"
                && record.value["status"] == "active"
        }));
        assert!(records.iter().any(|record| {
            record.resource_type == "team_membership"
                && record.resource_id == "bootstrap-initial-team\0bootstrap-owner"
                && record.value["role"] == "owner"
                && record.value["status"] == "active"
        }));
    }

    #[tokio::test]
    async fn audit_and_outbox_are_atomic_ordered_and_retryable() {
        let directory = crate::access::test_support::secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(identity, "Local", "Default").unwrap())
            .await
            .unwrap();
        let claimed = store
            .claim_authority_projection_batch(10, 256)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 3);
        assert!(
            claimed
                .windows(2)
                .all(|pair| pair[1].sequence == pair[0].sequence + 1)
        );
        assert!(
            claimed
                .iter()
                .all(|row| row.organization_id == "bootstrap-local")
        );
        store
            .release_failed_authority_projection(
                "bootstrap-local".into(),
                claimed.last().unwrap().sequence,
                10,
            )
            .await
            .unwrap();
        assert!(
            store
                .claim_authority_projection_batch(39, 256)
                .await
                .unwrap()
                .is_empty()
        );
        let retried = store
            .claim_authority_projection_batch(1000, 256)
            .await
            .unwrap();
        assert_eq!(retried.len(), claimed.len());
        store
            .acknowledge_authority_projection(
                "bootstrap-local".into(),
                retried.last().unwrap().sequence,
                "ab".repeat(32),
                1001,
            )
            .await
            .unwrap();
        assert_eq!(
            store.retain_authority_projection(1002).await.unwrap(),
            retried.len()
        );
    }
}
