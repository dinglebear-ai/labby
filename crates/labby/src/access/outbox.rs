//! Authority projection outbox.
//!
//! v1 projection is **snapshot-only**: an outbox row is a durable "the
//! authority state of this Organization changed" marker written in the same
//! transaction as its audit event. The producer coalesces every pending row
//! for an Organization into one complete signed snapshot; it never replays
//! per-event payloads. Depot acknowledges a contiguous watermark and the last
//! accepted envelope digest, both persisted here so the chain survives
//! retention and restarts.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufWriter, Write as _};

use labby_primitives::digest::Sha256Digest;

use super::error::{AccessStoreError, AccessStoreResult};
use super::store::map_sqlite_error;

pub(crate) const OUTBOX_BATCH_LIMIT: usize = 256;
/// Delivery attempts before a row is parked as terminally `failed`. A failed
/// row no longer starves other Organizations; it is surfaced through health
/// and swept once a later snapshot acknowledgement covers its sequence.
pub(crate) const OUTBOX_MAX_ATTEMPTS: i64 = 8;
const SNAPSHOT_SPOOL_MAX_RECORDS: usize = 1_000_000;
const SNAPSHOT_SPOOL_MAX_BYTES: usize = 256 * 1024 * 1024;

/// One claimed outbox row. Payloads are deliberately absent: delivery is a
/// snapshot of current state, never a replay of the event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingProjection {
    pub(crate) organization_id: String,
    pub(crate) sequence: u64,
}

/// Persisted Depot acknowledgement for one Organization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityAcknowledgement {
    /// Highest contiguous sequence Depot has durably accepted (0 = never).
    pub(crate) sequence: u64,
    /// Digest of the last accepted envelope; the next envelope chains to it.
    pub(crate) digest: Option<String>,
}

/// Delivery posture of one Organization's outbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OrganizationDelivery {
    pub(crate) organization_id: String,
    /// Highest local sequence ever enqueued (0 = nothing yet).
    pub(crate) head: u64,
    pub(crate) acknowledged: u64,
    pub(crate) pending: usize,
    pub(crate) inflight: usize,
    pub(crate) failed: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct AuthoritySnapshotRecord {
    pub(crate) resource_type: String,
    pub(crate) resource_id: String,
    pub(crate) value: Value,
}

#[derive(Debug)]
pub(crate) struct AuthoritySnapshotCheckpoint {
    pub(crate) spool: tempfile::NamedTempFile,
    pub(crate) record_count: usize,
    /// Last local mutation included in the same SQLite view as the spool.
    pub(crate) outbox_cutoff: Option<u64>,
}

pub(super) fn snapshot(
    connection: &Connection,
    organization_id: &str,
) -> AccessStoreResult<Vec<AuthoritySnapshotRecord>> {
    let mut records = Vec::new();
    snapshot_into(connection, organization_id, |record| {
        records.push(record);
        Ok(())
    })?;
    Ok(records)
}

fn snapshot_into(
    connection: &Connection,
    organization_id: &str,
    mut emit: impl FnMut(AuthoritySnapshotRecord) -> AccessStoreResult<()>,
) -> AccessStoreResult<()> {
    let (organization_status, policy_epoch, authority_schema, global_revision) = connection
        .query_row(
            "SELECT o.status,o.policy_epoch,m.schema_version,m.global_revision FROM organizations o JOIN access_metadata m ON m.singleton=1 WHERE o.organization_id=?1",
            [organization_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
        )
        .map_err(map_sqlite_error)?;
    emit(AuthoritySnapshotRecord {
        resource_type: "organization".into(),
        resource_id: organization_id.to_owned(),
        value: json!({"status":organization_status,"policy_epoch":policy_epoch,"authority_schema":authority_schema,"global_revision":global_revision}),
    })?;
    let mut principals = connection
        .prepare("SELECT p.principal_id,p.status,e.epoch FROM principals p JOIN principal_epochs e ON e.principal_id=p.principal_id WHERE p.organization_id=?1 ORDER BY p.principal_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    for row in principals
        .query_map([organization_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(map_sqlite_error)?
    {
        let (principal_id, status, principal_epoch) = row.map_err(map_sqlite_error)?;
        emit(AuthoritySnapshotRecord {
            resource_type: "principal".into(),
            resource_id: principal_id,
            value: json!({"status":status,"principal_epoch":principal_epoch}),
        })?;
    }
    drop(principals);
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
        emit(AuthoritySnapshotRecord {
            resource_type: "platform_administrator".into(),
            resource_id: principal_id,
            value: json!({"principal_status":principal_status,"status":status,"authority_epoch":authority_epoch}),
        })?;
    }
    drop(administrators);
    let mut projects = connection
        .prepare("SELECT project_id,status,project_policy_epoch FROM projects WHERE organization_id=?1 ORDER BY project_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    for row in projects
        .query_map([organization_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(map_sqlite_error)?
    {
        let (project_id, status, policy_epoch) = row.map_err(map_sqlite_error)?;
        emit(AuthoritySnapshotRecord {
            resource_type: "project".into(),
            resource_id: project_id,
            value: json!({"status":status,"policy_epoch":policy_epoch}),
        })?;
    }
    drop(projects);
    let mut project_memberships = connection
        .prepare("SELECT m.project_id,m.principal_id,m.role,m.status,e.epoch FROM project_memberships m JOIN project_membership_epochs e ON e.membership_id=m.membership_id WHERE m.organization_id=?1 ORDER BY m.project_id COLLATE BINARY,m.principal_id COLLATE BINARY")
        .map_err(map_sqlite_error)?;
    for row in project_memberships
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
        let (project_id, principal_id, role, status, membership_epoch) =
            row.map_err(map_sqlite_error)?;
        emit(AuthoritySnapshotRecord {
            resource_type: "project_membership".into(),
            resource_id: format!("{project_id}\u{0}{principal_id}"),
            value: json!({"status":status,"project_id":project_id,"principal_id":principal_id,"role":role,"membership_epoch":membership_epoch}),
        })?;
    }
    drop(project_memberships);
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
        emit(AuthoritySnapshotRecord {
            resource_type: "team".into(),
            resource_id: team_id,
            value: json!({"status":status,"policy_epoch":policy_epoch,"membership_epoch":membership_epoch}),
        })?;
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
        emit(AuthoritySnapshotRecord {
            resource_type: "team_membership".into(),
            resource_id: format!("{team_id}\u{0}{principal_id}"),
            value: json!({"team_id":team_id,"principal_id":principal_id,"role":role,"status":status,"membership_epoch":membership_epoch}),
        })?;
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
        emit(AuthoritySnapshotRecord {
            resource_type: "team_project".into(),
            resource_id: format!("{team_id}\u{0}{project_id}"),
            value: json!({"team_id":team_id,"project_id":project_id,"role":role,"status":status,"assignment_epoch":assignment_epoch}),
        })?;
    }
    Ok(())
}

fn write_spooled_record(
    writer: &mut impl std::io::Write,
    record: &AuthoritySnapshotRecord,
    record_count: &mut usize,
    byte_count: &mut usize,
    max_records: usize,
    max_bytes: usize,
) -> AccessStoreResult<()> {
    if *record_count >= max_records {
        return Err(AccessStoreError::Unavailable(
            "authority snapshot exceeds record spool limit".into(),
        ));
    }
    let encoded = serde_json::to_vec(record)
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
    let next_bytes = byte_count.checked_add(encoded.len() + 1).ok_or_else(|| {
        AccessStoreError::Unavailable("authority snapshot spool size overflow".into())
    })?;
    if next_bytes > max_bytes {
        return Err(AccessStoreError::Unavailable(
            "authority snapshot exceeds byte spool limit".into(),
        ));
    }
    writer
        .write_all(&encoded)
        .and_then(|()| writer.write_all(b"\n"))
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
    *record_count += 1;
    *byte_count = next_bytes;
    Ok(())
}

pub(super) fn snapshot_checkpoint(
    connection: &mut Connection,
    organization_id: &str,
) -> AccessStoreResult<AuthoritySnapshotCheckpoint> {
    // A deferred read transaction gives the snapshot and cutoff one coherent
    // SQLite view without reserving the writer for the potentially large
    // snapshot materialization.
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let spool = tempfile::NamedTempFile::new()
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
    let mut writer = BufWriter::new(
        spool
            .reopen()
            .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?,
    );
    let mut record_count = 0usize;
    let mut byte_count = 0usize;
    snapshot_into(&tx, organization_id, |record| {
        write_spooled_record(
            &mut writer,
            &record,
            &mut record_count,
            &mut byte_count,
            SNAPSHOT_SPOOL_MAX_RECORDS,
            SNAPSHOT_SPOOL_MAX_BYTES,
        )
    })?;
    writer
        .flush()
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?;
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
        spool,
        record_count,
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

/// Claim deliverable rows (`pending`, or `inflight` whose claim lease lapsed)
/// and lease them for thirty seconds. Terminal `failed` rows are never claimed.
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
        let mut statement = tx.prepare("SELECT organization_id,sequence FROM authority_projection_outbox WHERE status IN ('pending','inflight') AND next_attempt_at<=?1 ORDER BY organization_id COLLATE BINARY,sequence LIMIT ?2").map_err(map_sqlite_error)?;
        let selected = statement
            .query_map(params![now, i64::try_from(limit).unwrap_or(256)], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(map_sqlite_error)?;
        for row in selected {
            let (organization_id, sequence) = row.map_err(map_sqlite_error)?;
            rows.push(PendingProjection {
                organization_id,
                sequence: u64::try_from(sequence)
                    .map_err(|_| AccessStoreError::MalformedVocabulary)?,
            });
        }
    }
    for row in &rows {
        tx.execute("UPDATE authority_projection_outbox SET status='inflight',attempt_count=attempt_count+1,next_attempt_at=?3 WHERE organization_id=?1 AND sequence=?2",params![row.organization_id,i64::try_from(row.sequence).map_err(|_|AccessStoreError::MalformedVocabulary)?,now.saturating_add(30)]).map_err(map_sqlite_error)?;
    }
    tx.commit().map_err(map_sqlite_error)?;
    Ok(rows)
}

/// The persisted Depot acknowledgement for one Organization.
pub(super) fn acknowledged(
    connection: &Connection,
    organization_id: &str,
) -> AccessStoreResult<AuthorityAcknowledgement> {
    let row = connection
        .query_row(
            "SELECT acknowledged_sequence,acknowledged_digest FROM authority_outbox_sequences WHERE organization_id=?1",
            [organization_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let Some((sequence, digest)) = row else {
        return Ok(AuthorityAcknowledgement {
            sequence: 0,
            digest: None,
        });
    };
    Ok(AuthorityAcknowledgement {
        sequence: u64::try_from(sequence).map_err(|_| AccessStoreError::MalformedVocabulary)?,
        digest,
    })
}

/// Record a Depot acknowledgement: persist the watermark and envelope digest,
/// and mark every outbox row the acknowledged snapshot covers as `sent`.
///
/// An acknowledgement below the persisted watermark is a chain regression
/// (a restored or rolled-back Depot) and is refused; the caller must resync
/// with a fresh snapshot rather than silently rewinding.
pub(super) fn acknowledge(
    connection: &mut Connection,
    organization_id: &str,
    highest: u64,
    digest: &str,
    now: i64,
) -> AccessStoreResult<usize> {
    if organization_id.trim().is_empty() || !Sha256Digest::is_canonical(digest) {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    let highest_sql = i64::try_from(highest).map_err(|_| AccessStoreError::MalformedVocabulary)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let current = acknowledged(&tx, organization_id)?;
    if highest < current.sequence {
        return Err(AccessStoreError::ProjectionWatermarkRegressed);
    }
    tx.execute(
        "INSERT INTO authority_outbox_sequences(organization_id,next_sequence,acknowledged_sequence,acknowledged_digest)
         VALUES(?1,?2+1,?2,?3)
         ON CONFLICT(organization_id) DO UPDATE SET
           acknowledged_sequence=excluded.acknowledged_sequence,
           acknowledged_digest=excluded.acknowledged_digest,
           next_sequence=MAX(authority_outbox_sequences.next_sequence,excluded.acknowledged_sequence+1)",
        params![organization_id, highest_sql, digest],
    )
    .map_err(map_sqlite_error)?;
    let changed = tx
        .execute(
            "UPDATE authority_projection_outbox SET status='sent',sent_at=?3,envelope_digest=?4 WHERE organization_id=?1 AND sequence<=?2 AND status IN ('pending','inflight','failed')",
            params![organization_id, highest_sql, now, digest],
        )
        .map_err(map_sqlite_error)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(changed)
}

/// Return a failed claim to the queue with exponential backoff, or park it as
/// terminally `failed` once [`OUTBOX_MAX_ATTEMPTS`] is exhausted.
pub(super) fn release_failed(
    connection: &mut Connection,
    organization_id: &str,
    through: u64,
    now: i64,
) -> AccessStoreResult<usize> {
    let through = i64::try_from(through).map_err(|_| AccessStoreError::MalformedVocabulary)?;
    connection.execute("UPDATE authority_projection_outbox SET status=CASE WHEN attempt_count>=?4 THEN 'failed' ELSE 'pending' END,next_attempt_at=?3+MIN(3600,30*(1<<MIN(attempt_count,7))) WHERE organization_id=?1 AND sequence<=?2 AND status='inflight'",params![organization_id,through,now,OUTBOX_MAX_ATTEMPTS]).map_err(map_sqlite_error)
}

/// Prune delivered rows that are both older than `older_than` and covered by
/// the persisted acknowledgement. The chain digest lives in
/// `authority_outbox_sequences`, so pruning never loses `previous_digest`.
pub(super) fn retain(connection: &mut Connection, older_than: i64) -> AccessStoreResult<usize> {
    connection
        .execute(
            "DELETE FROM authority_projection_outbox WHERE status='sent' AND sent_at<?1 AND sequence<=(SELECT acknowledged_sequence FROM authority_outbox_sequences s WHERE s.organization_id=authority_projection_outbox.organization_id)",
            [older_than],
        )
        .map_err(map_sqlite_error)
}

/// Delivery posture per Organization for readiness and lag reporting.
pub(super) fn delivery_status(
    connection: &mut Connection,
) -> AccessStoreResult<Vec<OrganizationDelivery>> {
    let mut statement = connection
        .prepare(
            "SELECT o.organization_id,
                    COALESCE(s.next_sequence-1,0),
                    COALESCE(s.acknowledged_sequence,0),
                    (SELECT count(*) FROM authority_projection_outbox x WHERE x.organization_id=o.organization_id AND x.status='pending'),
                    (SELECT count(*) FROM authority_projection_outbox x WHERE x.organization_id=o.organization_id AND x.status='inflight'),
                    (SELECT count(*) FROM authority_projection_outbox x WHERE x.organization_id=o.organization_id AND x.status='failed')
             FROM organizations o
             LEFT JOIN authority_outbox_sequences s ON s.organization_id=o.organization_id
             WHERE o.status='active'
             ORDER BY o.organization_id COLLATE BINARY",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(map_sqlite_error)?;
    let mut result = Vec::new();
    for row in rows {
        let (organization_id, head, acknowledged, pending, inflight, failed) =
            row.map_err(map_sqlite_error)?;
        let to_u64 =
            |value: i64| u64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary);
        let to_usize =
            |value: i64| usize::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary);
        result.push(OrganizationDelivery {
            organization_id,
            head: to_u64(head)?,
            acknowledged: to_u64(acknowledged)?,
            pending: to_usize(pending)?,
            inflight: to_usize(inflight)?,
            failed: to_usize(failed)?,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use labby_auth::{Authenticator, VerifiedIdentity};

    use super::{AuthoritySnapshotRecord, write_spooled_record};
    use crate::access::{AccessStore, AccessStoreError, BootstrapOwnerInput};

    #[test]
    fn snapshot_spool_limits_fail_closed_before_writing_past_the_ceiling() {
        let record = AuthoritySnapshotRecord {
            resource_type: "principal".into(),
            resource_id: "principal-1".into(),
            value: serde_json::json!({"status":"active"}),
        };
        let mut output = Vec::new();
        let mut records = 0;
        let mut bytes = 0;
        write_spooled_record(&mut output, &record, &mut records, &mut bytes, 1, 1024).unwrap();
        assert!(
            write_spooled_record(&mut output, &record, &mut records, &mut bytes, 1, 1024).is_err()
        );
        assert_eq!(records, 1);

        let mut output = Vec::new();
        let mut records = 0;
        let mut bytes = 0;
        assert!(
            write_spooled_record(&mut output, &record, &mut records, &mut bytes, 10, 1).is_err()
        );
        assert!(output.is_empty());
    }

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
            record.resource_type == "organization"
                && record.resource_id == "bootstrap-local"
                && record.value["status"] == "active"
                && record.value["authority_schema"].is_number()
                && record.value["global_revision"].is_number()
        }));
        assert!(records.iter().any(|record| {
            record.resource_type == "principal"
                && record.resource_id == "bootstrap-owner"
                && record.value["status"] == "active"
                && record.value["principal_epoch"] == 1
        }));
        assert!(records.iter().any(|record| {
            record.resource_type == "platform_administrator"
                && record.resource_id == "bootstrap-owner"
                && record.value["status"] == "active"
        }));
        assert!(records.iter().any(|record| {
            record.resource_type == "project"
                && record.resource_id == "bootstrap-default"
                && record.value["status"] == "active"
        }));
        assert!(records.iter().any(|record| {
            record.resource_type == "project_membership"
                && record.resource_id == "bootstrap-default\0bootstrap-owner"
                && record.value["status"] == "active"
                && record.value["membership_epoch"] == 1
        }));
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
        // A claimed-but-unacknowledged batch (producer crashed mid-send) is
        // not claimable until its lease lapses, then becomes claimable again.
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
        let digest = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(
            store
                .acknowledge_authority_projection(
                    "bootstrap-local".into(),
                    retried.last().unwrap().sequence,
                    digest.clone(),
                    1001,
                )
                .await
                .unwrap(),
            retried.len()
        );
        let acknowledged = store
            .acknowledged_authority_projection("bootstrap-local".into())
            .await
            .unwrap();
        assert_eq!(acknowledged.sequence, retried.last().unwrap().sequence);
        assert_eq!(acknowledged.digest.as_deref(), Some(digest.as_str()));
        // An acknowledgement below the persisted watermark is a regression.
        assert!(matches!(
            store
                .acknowledge_authority_projection(
                    "bootstrap-local".into(),
                    1,
                    format!("sha256:{}", "cd".repeat(32)),
                    1002,
                )
                .await,
            Err(AccessStoreError::ProjectionWatermarkRegressed)
        ));
        // Retention is keyed on the acknowledged watermark, and the chain
        // digest survives pruning because it is persisted separately.
        assert_eq!(
            store.retain_authority_projection(1002).await.unwrap(),
            retried.len()
        );
        assert_eq!(
            store
                .acknowledged_authority_projection("bootstrap-local".into())
                .await
                .unwrap()
                .digest
                .as_deref(),
            Some(digest.as_str())
        );
        let status = store.authority_delivery_status().await.unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].acknowledged, acknowledged.sequence);
        assert_eq!(status[0].head, acknowledged.sequence);
        assert_eq!(
            (status[0].pending, status[0].inflight, status[0].failed),
            (0, 0, 0)
        );
    }

    #[tokio::test]
    async fn exhausted_attempts_park_rows_as_failed_without_blocking_claims() {
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
        let mut now = 0;
        for _ in 0..super::OUTBOX_MAX_ATTEMPTS {
            let claimed = store
                .claim_authority_projection_batch(now, 256)
                .await
                .unwrap();
            assert!(
                !claimed.is_empty(),
                "rows stay claimable until the attempt cap"
            );
            store
                .release_failed_authority_projection(
                    "bootstrap-local".into(),
                    claimed.last().unwrap().sequence,
                    now,
                )
                .await
                .unwrap();
            now += 10_000;
        }
        assert!(
            store
                .claim_authority_projection_batch(now, 256)
                .await
                .unwrap()
                .is_empty(),
            "failed rows are terminal"
        );
        let status = store.authority_delivery_status().await.unwrap();
        assert_eq!(status[0].failed, 3);
        assert_eq!(status[0].pending + status[0].inflight, 0);
        // A later snapshot acknowledgement sweeps the failed rows.
        store
            .acknowledge_authority_projection(
                "bootstrap-local".into(),
                status[0].head,
                format!("sha256:{}", "ef".repeat(32)),
                now,
            )
            .await
            .unwrap();
        let status = store.authority_delivery_status().await.unwrap();
        assert_eq!(status[0].failed, 0);
        assert_eq!(status[0].acknowledged, status[0].head);
    }
}
