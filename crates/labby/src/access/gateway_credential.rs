//! Durable, secret-free Team credential bindings for Gateway loadouts.
//!
//! The table is part of the versioned access schema
//! (`migrations::GATEWAY_CREDENTIAL_SCHEMA`); this module never creates it.

use labby_runtime::gateway_authority::{TeamCredentialBinding, TeamCredentialStatus};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::Digest as _;

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
    let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
    let prior = get_in(&tx, &input.team_id, &input.upstream_name)?;
    let binding = put_in_transaction(&tx, input, rotated_at_sql)?;
    audit_binding_change(
        &tx,
        &lease,
        BIND_ACTION,
        if prior.is_some() {
            "team_credential_rebound"
        } else {
            "team_credential_bound"
        },
        prior.as_ref(),
        &binding,
        rotated_at_sql,
    )?;
    tx.commit().map_err(map_sqlite_error)?;
    Ok(binding)
}

const BIND_ACTION: &str = "access.gateway_credential.bind";
const REVOKE_ACTION: &str = "access.gateway_credential.revoke";

/// Append the durable audit row for an authorized binding change inside the
/// mutation's own transaction, so a binding can never advance without its
/// evidence. Binding ids are recorded only as fingerprints.
fn audit_binding_change(
    tx: &Transaction<'_>,
    lease: &labby_runtime::authority::AuthorityLease,
    action: &str,
    reason: &str,
    prior: Option<&TeamCredentialBinding>,
    current: &TeamCredentialBinding,
    now_millis: i64,
) -> AccessStoreResult<()> {
    let actor = lease.binding().principal_id();
    let organization: String = tx
        .query_row(
            "SELECT organization_id FROM principals WHERE principal_id=?1 AND status='active'",
            [actor],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite_error)?
        .ok_or(AccessStoreError::NotAuthorized)?;
    let fingerprint = |value: &str| hex::encode(sha2::Sha256::digest(value.as_bytes()));
    let metadata = serde_json::json!({
        "team_id": current.team_id,
        "upstream_name": current.upstream_name,
        "generation": current.generation,
        "prior_binding_fingerprint": prior.map(|binding| fingerprint(&binding.binding_id)),
        "binding_fingerprint": fingerprint(&current.binding_id),
        "prior_custodian_principal_id": prior.map(|binding| binding.custodian_principal_id.as_str()),
        "custodian_principal_id": current.custodian_principal_id,
    })
    .to_string();
    tx.execute(
        "INSERT INTO access_audit(event_id,occurred_at,correlation_id,actor_principal_id,
             organization_id,project_id,action,target_kind,target_fingerprint,decision,
             reason_code,policy_epoch,metadata_json)
         VALUES(?1,?2,NULL,?3,?4,NULL,?5,'team_gateway_credential',?6,'allow',?7,?8,?9)",
        params![
            format!("gateway-credential-{}", ulid::Ulid::new()),
            now_millis / 1000,
            actor,
            organization,
            action,
            fingerprint(&format!("{}\0{}", current.team_id, current.upstream_name)),
            reason,
            checked_i64(current.generation)?,
            metadata
        ],
    )
    .map_err(map_sqlite_error)?;
    Ok(())
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
    let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
    let prior = get_in(&tx, team_id, upstream_name)?;
    let binding = revoke_in_transaction(&tx, team_id, upstream_name, now_sql)?;
    audit_binding_change(
        &tx,
        &lease,
        REVOKE_ACTION,
        "team_credential_revoked",
        prior.as_ref(),
        &binding,
        now_sql,
    )?;
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

    fn owner() -> labby_auth::VerifiedIdentity {
        labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap()
    }

    fn team_request(action: &str) -> super::super::AuthorityRequest {
        use labby_primitives::access::{
            ActionRef, Capability, OwnerScope, ResourceFamily, ResourceId, ResourceRef, TeamId,
        };
        let action = ActionRef::new("access", action).unwrap();
        super::super::AuthorityRequest::new(
            owner(),
            super::super::ActionAuthoritySpec::SCHEMA_VERSION,
            action.clone(),
            ResourceRef::new(
                OwnerScope::Team(TeamId::new("bootstrap-initial-team").unwrap()),
                ResourceFamily::Platform,
                ResourceId::new("bootstrap-initial-team").unwrap(),
            ),
            super::super::AuthorityCeiling::trusted_local(),
            None,
            1_000,
            vec![labby_runtime::authority::AuthoritySafeBoundary::BeforeDispatch],
            vec![super::super::ActionAuthoritySpec::new(
                action,
                ResourceFamily::Platform,
                Capability::ScopeManage,
            )],
        )
    }

    fn binding(binding_id: &str, rotated_at_millis: u64) -> PutTeamCredentialBinding {
        PutTeamCredentialBinding {
            binding_id: binding_id.into(),
            team_id: "bootstrap-initial-team".into(),
            upstream_name: "github".into(),
            custodian_principal_id: super::super::bootstrap::PRINCIPAL_ID.into(),
            rotated_at_millis,
        }
    }

    async fn audit_rows(store: &super::super::AccessStore) -> Vec<(String, String, String)> {
        store
            .with_connection(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT action,reason_code,metadata_json FROM access_audit
                         WHERE target_kind='team_gateway_credential' ORDER BY rowid",
                    )
                    .map_err(map_sqlite_error)?;
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .map_err(map_sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap()
    }

    /// SEC-M3 / TST-M2: every authorized bind, rebind, and revoke writes an
    /// `access_audit` row in the same transaction; a failed audit write
    /// rolls the binding change back.
    #[tokio::test]
    async fn rebind_is_audited_atomically_and_audit_failure_rolls_back() {
        let directory = super::super::test_support::secure_tempdir();
        let store = super::super::AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        store
            .bootstrap_owner(
                super::super::BootstrapOwnerInput::new(owner(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        for (binding_id, at) in [("binding-1", 1_000), ("binding-2", 2_000)] {
            store
                .put_team_gateway_credential_binding_authorized(
                    team_request(BIND_ACTION),
                    binding(binding_id, at),
                )
                .await
                .unwrap();
        }
        let rows = audit_rows(&store).await;
        assert_eq!(
            rows.iter()
                .map(|(action, reason, _)| (action.as_str(), reason.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (BIND_ACTION, "team_credential_bound"),
                (BIND_ACTION, "team_credential_rebound"),
            ]
        );
        assert!(
            rows.iter()
                .all(|(_, _, metadata)| !metadata.contains("binding-1")
                    && !metadata.contains("binding-2")),
            "binding ids are recorded only as fingerprints"
        );
        assert!(rows[1].2.contains("\"prior_binding_fingerprint\":\""));

        store
            .with_connection(|connection| {
                connection
                    .execute_batch(
                        "CREATE TEMP TRIGGER fail_gateway_credential_audit BEFORE INSERT ON access_audit WHEN NEW.target_kind='team_gateway_credential' BEGIN SELECT RAISE(ABORT,'forced'); END;",
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert!(
            store
                .put_team_gateway_credential_binding_authorized(
                    team_request(BIND_ACTION),
                    binding("binding-3", 3_000),
                )
                .await
                .is_err()
        );
        let current = store
            .get_team_gateway_credential_binding("bootstrap-initial-team".into(), "github".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (current.binding_id.as_str(), current.generation),
            ("binding-2", 2),
            "a binding must not advance without its audit row"
        );
        assert!(
            store
                .revoke_team_gateway_credential_binding_authorized(
                    team_request(REVOKE_ACTION),
                    "bootstrap-initial-team".into(),
                    "github".into(),
                    4_000,
                )
                .await
                .is_err(),
            "revocation is audited under the same rule"
        );

        store
            .with_connection(|connection| {
                connection
                    .execute_batch("DROP TRIGGER fail_gateway_credential_audit")
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        store
            .revoke_team_gateway_credential_binding_authorized(
                team_request(REVOKE_ACTION),
                "bootstrap-initial-team".into(),
                "github".into(),
                4_000,
            )
            .await
            .unwrap();
        let rows = audit_rows(&store).await;
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[2].0.as_str(), rows[2].1.as_str()),
            (REVOKE_ACTION, "team_credential_revoked")
        );
    }
}
