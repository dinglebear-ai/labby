//! Durable, secret-free Team credential bindings for Gateway loadouts.
//!
//! The table is part of the versioned access schema
//! (`migrations::GATEWAY_CREDENTIAL_SCHEMA`); this module never creates it.

use labby_runtime::gateway_authority::{TeamCredentialBinding, TeamCredentialStatus};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::{AccessStoreError, error::AccessStoreResult, store::map_sqlite_error};

#[derive(Clone, Debug)]
pub(crate) struct PutTeamCredentialBinding {
    pub binding_id: String,
    pub team_id: String,
    pub upstream_name: String,
    pub custodian_principal_id: String,
    pub rotated_at_millis: u64,
}

/// Create or rotate a binding. The generation advance is a single upsert
/// statement inside an immediate transaction, so two concurrent rotations
/// cannot both observe the same prior generation.
pub(crate) fn put(
    connection: &mut Connection,
    input: &PutTeamCredentialBinding,
) -> AccessStoreResult<TeamCredentialBinding> {
    let candidate = TeamCredentialBinding {
        binding_id: input.binding_id.clone(),
        team_id: input.team_id.clone(),
        upstream_name: input.upstream_name.clone(),
        custodian_principal_id: input.custodian_principal_id.clone(),
        generation: 1,
        rotated_at_millis: input.rotated_at_millis,
        status: TeamCredentialStatus::Active,
    };
    if !candidate.validate() {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    let rotated_at_sql = checked_i64(input.rotated_at_millis)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let binding = put_in_transaction(&tx, input, rotated_at_sql)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(binding)
}

pub(crate) fn put_authorized(
    connection: &mut Connection,
    request: super::AuthorityRequest,
    input: &PutTeamCredentialBinding,
) -> AccessStoreResult<TeamCredentialBinding> {
    let candidate = TeamCredentialBinding {
        binding_id: input.binding_id.clone(),
        team_id: input.team_id.clone(),
        upstream_name: input.upstream_name.clone(),
        custodian_principal_id: input.custodian_principal_id.clone(),
        generation: 1,
        rotated_at_millis: input.rotated_at_millis,
        status: TeamCredentialStatus::Active,
    };
    if !candidate.validate() {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    let rotated_at_sql = checked_i64(input.rotated_at_millis)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    super::authority::authorize_action_in_transaction(&tx, request)?;
    let binding = put_in_transaction(&tx, input, rotated_at_sql)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(binding)
}

fn put_in_transaction(
    tx: &Transaction<'_>,
    input: &PutTeamCredentialBinding,
    rotated_at_sql: i64,
) -> AccessStoreResult<TeamCredentialBinding> {
    tx.execute(
        "INSERT INTO gateway_team_credential_bindings VALUES
             (?1,?2,?3,?4,1,?5,'active',NULL)
             ON CONFLICT(team_id,upstream_name) DO UPDATE SET
               binding_id=excluded.binding_id,
               custodian_principal_id=excluded.custodian_principal_id,
               generation=gateway_team_credential_bindings.generation+1,
               rotated_at_millis=excluded.rotated_at_millis,
               status='active',revoked_at_millis=NULL",
        params![
            input.binding_id,
            input.team_id,
            input.upstream_name,
            input.custodian_principal_id,
            rotated_at_sql
        ],
    )
    .map_err(map_sqlite_error)?;
    get_in(tx, &input.team_id, &input.upstream_name)?
        .ok_or_else(|| AccessStoreError::Unavailable("credential binding write vanished".into()))
}

pub(crate) fn get(
    connection: &mut Connection,
    team_id: &str,
    upstream_name: &str,
) -> AccessStoreResult<Option<TeamCredentialBinding>> {
    get_in(connection, team_id, upstream_name)
}

fn get_in(
    connection: &Connection,
    team_id: &str,
    upstream_name: &str,
) -> AccessStoreResult<Option<TeamCredentialBinding>> {
    connection
        .query_row(
            "SELECT binding_id,team_id,upstream_name,custodian_principal_id,
                    generation,rotated_at_millis,status
             FROM gateway_team_credential_bindings
             WHERE team_id=?1 AND upstream_name=?2",
            params![team_id, upstream_name],
            decode,
        )
        .optional()
        .map_err(map_sqlite_error)
}

pub(crate) fn list(
    connection: &mut Connection,
    team_id: &str,
) -> AccessStoreResult<Vec<TeamCredentialBinding>> {
    let mut statement = connection
        .prepare(
            "SELECT binding_id,team_id,upstream_name,custodian_principal_id,
                    generation,rotated_at_millis,status
             FROM gateway_team_credential_bindings WHERE team_id=?1
             ORDER BY upstream_name",
        )
        .map_err(map_sqlite_error)?;
    statement
        .query_map([team_id], decode)
        .map_err(map_sqlite_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_sqlite_error)
}

/// Revoke an active binding. Revoking a binding that does not exist (or is
/// already revoked) is an error so callers never skip credential
/// invalidation silently on a typo or a stale team/upstream pair.
pub(crate) fn revoke(
    connection: &mut Connection,
    team_id: &str,
    upstream_name: &str,
    now_millis: u64,
) -> AccessStoreResult<TeamCredentialBinding> {
    let now_sql = checked_i64(now_millis)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    let binding = revoke_in_transaction(&tx, team_id, upstream_name, now_sql)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(binding)
}

pub(crate) fn revoke_authorized(
    connection: &mut Connection,
    request: super::AuthorityRequest,
    team_id: &str,
    upstream_name: &str,
    now_millis: u64,
) -> AccessStoreResult<TeamCredentialBinding> {
    let now_sql = checked_i64(now_millis)?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_error)?;
    super::authority::authorize_action_in_transaction(&tx, request)?;
    let binding = revoke_in_transaction(&tx, team_id, upstream_name, now_sql)?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(binding)
}

fn revoke_in_transaction(
    tx: &Transaction<'_>,
    team_id: &str,
    upstream_name: &str,
    now_sql: i64,
) -> AccessStoreResult<TeamCredentialBinding> {
    let changed = tx
        .execute(
            "UPDATE gateway_team_credential_bindings SET
               generation=generation+1,status='revoked',revoked_at_millis=?3,
               rotated_at_millis=?3
             WHERE team_id=?1 AND upstream_name=?2 AND status='active'",
            params![team_id, upstream_name, now_sql],
        )
        .map_err(map_sqlite_error)?;
    if changed != 1 {
        return Err(AccessStoreError::TeamCredentialBindingUnavailable);
    }
    get_in(tx, team_id, upstream_name)?.ok_or(AccessStoreError::TeamCredentialBindingUnavailable)
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamCredentialBinding> {
    Ok(TeamCredentialBinding {
        binding_id: row.get(0)?,
        team_id: row.get(1)?,
        upstream_name: row.get(2)?,
        custodian_principal_id: row.get(3)?,
        generation: checked_u64(row.get::<_, i64>(4)?)?,
        rotated_at_millis: checked_u64(row.get::<_, i64>(5)?)?,
        status: match row.get::<_, String>(6)?.as_str() {
            "active" => TeamCredentialStatus::Active,
            "revoked" => TeamCredentialStatus::Revoked,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
    })
}

fn checked_i64(value: u64) -> AccessStoreResult<i64> {
    i64::try_from(value).map_err(|_| AccessStoreError::MalformedVocabulary)
}

fn checked_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(super::super::migrations::GATEWAY_CREDENTIAL_SCHEMA)
            .unwrap();
        connection
    }

    #[test]
    fn rotation_and_revocation_advance_generation_without_secret_columns() {
        let mut connection = connection();
        let input = PutTeamCredentialBinding {
            binding_id: "binding-a-v1".into(),
            team_id: "alpha".into(),
            upstream_name: "shared".into(),
            custodian_principal_id: "owner-a".into(),
            rotated_at_millis: 1,
        };
        let first = put(&mut connection, &input).unwrap();
        let mut rotated = input;
        rotated.binding_id = "binding-a-v2".into();
        rotated.rotated_at_millis = 2;
        let second = put(&mut connection, &rotated).unwrap();
        assert_eq!(second.generation, first.generation + 1);
        let revoked = revoke(&mut connection, "alpha", "shared", 3).unwrap();
        assert_eq!(revoked.generation, second.generation + 1);
        assert!(!revoked.usable(second.generation));
        assert!(matches!(
            revoke(&mut connection, "alpha", "shared", 4),
            Err(AccessStoreError::TeamCredentialBindingUnavailable)
        ));
        assert!(matches!(
            revoke(&mut connection, "alpha", "missing", 4),
            Err(AccessStoreError::TeamCredentialBindingUnavailable)
        ));
        let columns: String = connection
            .query_row(
                "SELECT group_concat(name, ',') FROM pragma_table_info('gateway_team_credential_bindings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!columns.contains("secret"));
        assert!(!columns.contains("token"));
    }

    #[test]
    fn teams_sharing_an_upstream_cannot_observe_each_others_binding() {
        let mut connection = connection();
        for team in ["alpha", "beta"] {
            put(
                &mut connection,
                &PutTeamCredentialBinding {
                    binding_id: format!("binding-{team}"),
                    team_id: team.into(),
                    upstream_name: "shared".into(),
                    custodian_principal_id: format!("owner-{team}"),
                    rotated_at_millis: 1,
                },
            )
            .unwrap();
        }
        let alpha = list(&mut connection, "alpha").unwrap();
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha[0].binding_id, "binding-alpha");
    }
}