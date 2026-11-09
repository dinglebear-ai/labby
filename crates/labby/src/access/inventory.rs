//! Read-only Access inventories with a shared absolute execution budget.

use std::io::{self, Write};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::error::AccessStoreError;
use super::store::{AccessStore, map_sqlite_error};
use super::{AuthorityRequest, ProjectRole, TeamRole};

pub(crate) const PAGE_BYTES: usize = 61_440;
const READ_CEILING: Duration = Duration::from_millis(1_500);

#[derive(Clone, Copy, Debug)]
pub(crate) struct AccessReadDeadline(tokio::time::Instant);

impl AccessReadDeadline {
    pub(crate) fn server() -> Self {
        Self::after(READ_CEILING)
    }

    pub(crate) fn after(budget: Duration) -> Self {
        Self(tokio::time::Instant::now() + budget.min(READ_CEILING))
    }

    pub(crate) fn instant(self) -> tokio::time::Instant {
        self.0
    }

    pub(crate) fn remaining(self) -> Duration {
        self.0
            .saturating_duration_since(tokio::time::Instant::now())
    }

    pub(crate) fn check(self, cancellation: &CancellationToken) -> InventoryResult<()> {
        if self.remaining().is_zero() {
            Err(InventoryError::Timeout)
        } else if cancellation.is_cancelled() {
            Err(InventoryError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum InventoryError {
    #[error("access inventory admission expired")]
    QueueSaturated,
    #[error("access inventory deadline expired")]
    Timeout,
    #[error("access inventory cancelled")]
    Cancelled,
    #[error("access inventory worker unavailable")]
    WorkerUnavailable,
    #[error("access inventory cleanup failed")]
    CleanupFailed,
    #[error("access inventory revision changed")]
    RevisionConflict,
    #[error("access inventory page exceeds its byte budget")]
    TooLarge { terminal: bool },
    #[error("access inventory store operation failed")]
    Store(#[from] AccessStoreError),
}

pub(crate) type InventoryResult<T> = Result<T, InventoryError>;

#[derive(Clone, Copy, Debug)]
pub(crate) enum TeamInventoryKind {
    Members,
    Assignments,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InventoryPage {
    pub(crate) limit: u32,
    pub(crate) offset: i64,
    pub(crate) snapshot_revision: Option<i64>,
}

pub(crate) struct TeamInventoryInput {
    pub(crate) request: AuthorityRequest,
    pub(crate) team_id: String,
    pub(crate) kind: TeamInventoryKind,
    pub(crate) page: InventoryPage,
    pub(crate) deadline: AccessReadDeadline,
}

impl AccessStore {
    pub(crate) async fn list_team_inventory(
        &self,
        input: TeamInventoryInput,
    ) -> InventoryResult<Value> {
        self.with_inventory_connection(input.deadline, move |connection, cancellation| {
            list_team_inventory(connection, input, cancellation)
        })
        .await
    }
}

fn list_team_inventory(
    connection: &mut Connection,
    input: TeamInventoryInput,
    cancellation: &CancellationToken,
) -> InventoryResult<Value> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(map_sqlite_error)?;
    let actor = super::read::resolve_principal(&transaction, input.request.identity())?;
    // Platform guards can short-circuit. Establish the actual target and its
    // organization before consulting them, within this same read snapshot.
    let target = transaction
        .query_row(
            "SELECT status,policy_epoch,membership_epoch FROM groups
             WHERE group_id=?1 AND organization_id=?2 AND kind='team'
               AND status IN ('active','suspended')",
            params![input.team_id, actor.organization_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or(AccessStoreError::NotAuthorized)?;
    super::authority::authorize_action_in_transaction(&transaction, input.request)?;
    super::team::require_team_member(
        &transaction,
        &actor.id,
        &actor.organization_id,
        &input.team_id,
    )?;
    let revision: i64 = transaction
        .query_row(
            "SELECT global_revision FROM access_metadata WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    if input
        .page
        .snapshot_revision
        .is_some_and(|expected| expected != revision)
    {
        return Err(InventoryError::RevisionConflict);
    }
    let sql = match input.kind {
        TeamInventoryKind::Members => {
            "SELECT principal_id,role,status,membership_epoch FROM team_memberships
             WHERE organization_id=?1 AND team_id=?2
             ORDER BY principal_id COLLATE BINARY LIMIT ?3 OFFSET ?4"
        }
        TeamInventoryKind::Assignments => {
            "SELECT a.project_id,a.role,a.status,a.assignment_epoch,p.status,p.project_policy_epoch
             FROM team_project_assignments a JOIN projects p
               ON p.organization_id=a.organization_id AND p.project_id=a.project_id
             WHERE a.organization_id=?1 AND a.team_id=?2
             ORDER BY a.project_id COLLATE BINARY LIMIT ?3 OFFSET ?4"
        }
    };
    let mut statement = transaction.prepare(sql).map_err(map_sqlite_error)?;
    let mut rows = statement
        .query(params![
            actor.organization_id,
            input.team_id,
            i64::from(input.page.limit) + 1,
            input.page.offset
        ])
        .map_err(map_sqlite_error)?;
    let mut values = Vec::new();
    let mut encoded = PageSize::default();
    let mut has_more = false;
    while let Some(row) = rows.next().map_err(map_sqlite_error)? {
        input.deadline.check(cancellation)?;
        if values.len() == input.page.limit as usize {
            has_more = true;
            break;
        }
        let value = project_row(row, input.kind, input.page.limit)?;
        serde_json::to_writer(&mut encoded, &value).map_err(|_| overflow(input.page.limit))?;
        values.push(value);
    }
    drop(rows);
    drop(statement);
    input.deadline.check(cancellation)?;
    let next_offset = if has_more {
        Some(
            input
                .page
                .offset
                .checked_add(i64::from(input.page.limit))
                .ok_or(AccessStoreError::MalformedVocabulary)?,
        )
    } else {
        None
    };
    let mut result = json!({
        "team_id": input.team_id,
        "team_status": target.0,
        "global_revision": nonnegative(revision)?,
        "next_offset": next_offset,
    });
    match input.kind {
        TeamInventoryKind::Members => {
            result["team_policy_epoch"] = nonnegative(target.1)?.into();
            result["team_membership_epoch"] = nonnegative(target.2)?.into();
            result["members"] = values.into();
        }
        TeamInventoryKind::Assignments => result["assignments"] = values.into(),
    }
    serde_json::to_writer(PageSize::default(), &result).map_err(|_| overflow(input.page.limit))?;
    input.deadline.check(cancellation)?;
    transaction.commit().map_err(map_sqlite_error)?;
    Ok(result)
}

fn project_row(row: &Row<'_>, kind: TeamInventoryKind, limit: u32) -> InventoryResult<Value> {
    let id = bounded_text(row, 0, limit)?;
    let role = bounded_text(row, 1, limit)?;
    let status = bounded_text(row, 2, limit)?;
    if !matches!(status.as_str(), "active" | "suspended" | "revoked") {
        return Err(AccessStoreError::MalformedVocabulary.into());
    }
    let epoch = nonnegative(row.get(3).map_err(map_sqlite_error)?)?;
    match kind {
        TeamInventoryKind::Members => {
            TeamRole::from_persisted(&role).ok_or(AccessStoreError::MalformedVocabulary)?;
            Ok(json!({"principal_id":id,"role":role,"status":status,"membership_epoch":epoch}))
        }
        TeamInventoryKind::Assignments => {
            ProjectRole::from_persisted(&role).ok_or(AccessStoreError::MalformedVocabulary)?;
            let project_status = bounded_text(row, 4, limit)?;
            if !matches!(project_status.as_str(), "active" | "suspended" | "disabled") {
                return Err(AccessStoreError::MalformedVocabulary.into());
            }
            let project_epoch = nonnegative(row.get(5).map_err(map_sqlite_error)?)?;
            Ok(
                json!({"project_id":id,"role":role,"status":status,"assignment_epoch":epoch,
                "project_status":project_status,"project_policy_epoch":project_epoch}),
            )
        }
    }
}

fn nonnegative(value: i64) -> Result<u64, AccessStoreError> {
    u64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary)
}

fn bounded_text(row: &Row<'_>, column: usize, limit: u32) -> InventoryResult<String> {
    let value = row.get_ref(column).map_err(map_sqlite_error)?;
    let bytes = value
        .as_str()
        .map_err(|_| AccessStoreError::MalformedVocabulary)?;
    if bytes.len() > PAGE_BYTES {
        return Err(overflow(limit));
    }
    Ok(bytes.to_owned())
}

fn overflow(limit: u32) -> InventoryError {
    InventoryError::TooLarge {
        terminal: limit == 1,
    }
}

#[derive(Default)]
struct PageSize(usize);

impl Write for PageSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > PAGE_BYTES.saturating_sub(self.0) {
            return Err(io::Error::other("access inventory byte budget exceeded"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
