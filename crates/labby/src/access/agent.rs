//! Durable Agent definition records and their admitted-session ledger.
//!
//! The tables are part of the versioned access schema
//! (`migrations::AGENT_TASK_SCHEMA`); this module never creates them.

use labby_primitives::access::OwnerScope;
use labby_primitives::agent::{AgentDefinition, AgentState};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

use super::error::{AccessStoreError, AccessStoreResult};

pub(crate) struct AgentDefinitionStore {
    connection: Connection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentSessionRecord {
    pub session_id: String,
    pub agent_id: String,
    pub agent_version: u64,
    pub principal_id: String,
    pub status: String,
    pub lease_expires_at: i64,
    pub created_at: i64,
    pub input_digest: String,
    pub input_text: String,
    pub output_digest: Option<String>,
    pub transcript: Option<String>,
    pub error_code: Option<String>,
    pub resumed_from_session_id: Option<String>,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AgentSessionAdmission {
    Created,
    Replay(AgentSessionRecord),
    Conflict { existing_session_id: String },
}

impl AgentDefinitionStore {
    pub(crate) fn open(path: &Path) -> AccessStoreResult<Self> {
        let connection = Connection::open(path).map_err(super::store::map_sqlite_error)?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(super::store::map_sqlite_error)?;
        Ok(Self { connection })
    }

    pub(crate) fn put(
        &mut self,
        definition: &AgentDefinition,
        actor: &str,
        now: i64,
    ) -> AccessStoreResult<()> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(super::store::map_sqlite_error)?;
        Self::put_in_transaction(&tx, definition, actor, now)?;
        tx.commit().map_err(super::store::map_sqlite_error)
    }

    pub(super) fn put_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        definition: &AgentDefinition,
        actor: &str,
        now: i64,
    ) -> AccessStoreResult<()> {
        definition
            .validate()
            .map_err(|_| AccessStoreError::MalformedVocabulary)?;
        let (owner_kind, owner_id) = owner(&definition.owner);
        let state = state(definition.state);
        let payload = serde_json::json!({
            "schemaVersion": 1,
            "agentId": definition.id,
            "catalogGeneration": definition.revision.catalog_generation,
            "contentDigest": definition.revision.content_digest,
            "repositoryDigest": definition.revision.repository_digest,
            "imageDigest": definition.revision.image_digest,
            "harnessDigest": definition.revision.harness_digest,
            "loadoutDigest": definition.revision.loadout_digest,
            "credentialReferences": definition.revision.credential_references,
            "capabilitySchemaVersion": labby_primitives::access::Capability::SCHEMA_VERSION.get(),
            "requiredCapabilities": definition.required_capabilities.iter().map(|value| value.as_wire()).collect::<Vec<_>>(),
            "revocationPolicy": match definition.revocation_policy {
                labby_primitives::agent::RunningRevocationPolicy::StopAtSafeBoundary => "stop_at_safe_boundary",
                labby_primitives::agent::RunningRevocationPolicy::StopImmediately => "stop_immediately",
            },
        }).to_string();
        let changed = tx.execute("INSERT INTO agent_definitions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(agent_id) DO UPDATE SET version=excluded.version,definition_json=excluded.definition_json,state=excluded.state,authority_epoch=excluded.authority_epoch,publication_epoch=excluded.publication_epoch,updated_at=excluded.updated_at WHERE excluded.version=agent_definitions.version+1 AND excluded.owner_kind=agent_definitions.owner_kind AND excluded.owner_id=agent_definitions.owner_id", params![definition.id,owner_kind,owner_id,i64::try_from(definition.revision.version).map_err(|_|AccessStoreError::MalformedVocabulary)?,payload,state,i64::try_from(definition.authority_epoch).map_err(|_|AccessStoreError::MalformedVocabulary)?,i64::try_from(definition.publication_epoch).map_err(|_|AccessStoreError::MalformedVocabulary)?,now]).map_err(super::store::map_sqlite_error)?;
        if changed != 1 {
            return Err(AccessStoreError::IntegrityViolation {
                check: "agent_version",
            });
        }
        tx.execute(
            "INSERT INTO agent_definition_audit VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                format!("agent-{}-{}", definition.id, definition.revision.version),
                definition.id,
                actor,
                if definition.revision.version == 1 {
                    "create"
                } else {
                    "update"
                },
                i64::try_from(definition.authority_epoch)
                    .map_err(|_| AccessStoreError::MalformedVocabulary)?,
                now
            ],
        )
        .map_err(super::store::map_sqlite_error)?;
        Ok(())
    }

    pub(crate) fn get(&self, id: &str) -> AccessStoreResult<Option<AgentDefinition>> {
        self.connection.query_row("SELECT owner_kind,owner_id,version,definition_json,state,authority_epoch,publication_epoch FROM agent_definitions WHERE agent_id=?1 AND state!='deleted'", [id], decode).optional().map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn list_page(
        &self,
        after: &str,
        limit: usize,
    ) -> AccessStoreResult<Vec<AgentDefinition>> {
        if limit == 0 || limit > 100 {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let mut statement = self.connection.prepare("SELECT owner_kind,owner_id,version,definition_json,state,authority_epoch,publication_epoch FROM agent_definitions WHERE state!='deleted' AND agent_id>?1 ORDER BY agent_id LIMIT ?2").map_err(super::store::map_sqlite_error)?;
        statement
            .query_map(
                params![
                    after,
                    i64::try_from(limit).map_err(|_| AccessStoreError::MalformedVocabulary)?
                ],
                decode,
            )
            .map_err(super::store::map_sqlite_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn set_state(
        &mut self,
        id: &str,
        new_state: AgentState,
        actor: &str,
        now: i64,
    ) -> AccessStoreResult<()> {
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(super::store::map_sqlite_error)?;
        Self::set_state_in_transaction(&tx, id, new_state, actor, now)?;
        tx.commit().map_err(super::store::map_sqlite_error)
    }

    pub(super) fn set_state_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        new_state: AgentState,
        actor: &str,
        now: i64,
    ) -> AccessStoreResult<()> {
        let action = match new_state {
            AgentState::Active => "update",
            AgentState::Suspended => "suspend",
            AgentState::Deleted => "delete",
        };
        let changed = tx.execute("UPDATE agent_definitions SET state=?2,updated_at=?3 WHERE agent_id=?1 AND state!='deleted'", params![id,state(new_state),now]).map_err(super::store::map_sqlite_error)?;
        if changed != 1 {
            return Err(AccessStoreError::NotAuthorized);
        }
        let epoch: i64 = tx
            .query_row(
                "SELECT authority_epoch FROM agent_definitions WHERE agent_id=?1",
                [id],
                |row| row.get(0),
            )
            .map_err(super::store::map_sqlite_error)?;
        tx.execute(
            "INSERT INTO agent_definition_audit VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                format!("agent-{id}-{action}-{now}"),
                id,
                actor,
                action,
                epoch,
                now
            ],
        )
        .map_err(super::store::map_sqlite_error)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_session(
        &mut self,
        session_id: &str,
        definition: &AgentDefinition,
        principal: &str,
        request_key: &str,
        operation: &str,
        authority_fingerprint: &str,
        lease_expires_at: i64,
        input_digest: &str,
        input_text: &str,
        resumed_from_session_id: Option<&str>,
        now: i64,
    ) -> AccessStoreResult<AgentSessionAdmission> {
        validate_admission(
            session_id,
            request_key,
            operation,
            input_digest,
            input_text,
            resumed_from_session_id,
        )?;
        let (owner_kind, owner_id) = owner(&definition.owner);
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(super::store::map_sqlite_error)?;
        if let Some(existing) = find_session_request(
            &tx,
            principal,
            owner_kind,
            owner_id,
            request_key,
            operation,
            &definition.id,
            definition.revision.version,
            input_digest,
            resumed_from_session_id,
        )? {
            return Ok(existing);
        }
        tx.execute("INSERT INTO agent_sessions(session_id,agent_id,agent_version,principal_id,authority_fingerprint,status,lease_expires_at,created_at) VALUES(?1,?2,?3,?4,?5,'admitted',?6,?7)", params![session_id,definition.id,i64::try_from(definition.revision.version).map_err(|_|AccessStoreError::MalformedVocabulary)?,principal,authority_fingerprint,lease_expires_at,now]).map_err(super::store::map_sqlite_error)?;
        tx.execute(
            "INSERT INTO agent_session_evidence(session_id,input_digest,input_text,resumed_from_session_id,updated_at) VALUES(?1,?2,?3,?4,?5)",
            params![session_id, input_digest, input_text, resumed_from_session_id, now],
        )
        .map_err(super::store::map_sqlite_error)?;
        tx.execute(
            "INSERT INTO agent_session_requests(principal_id,owner_kind,owner_id,request_key,operation,agent_id,agent_version,input_digest,resumed_from_session_id,session_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![principal,owner_kind,owner_id,request_key,operation,definition.id,i64::try_from(definition.revision.version).map_err(|_|AccessStoreError::MalformedVocabulary)?,input_digest,resumed_from_session_id,session_id,now],
        )
        .map_err(super::store::map_sqlite_error)?;
        tx.commit().map_err(super::store::map_sqlite_error)?;
        Ok(AgentSessionAdmission::Created)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn session_request(
        &self,
        definition: &AgentDefinition,
        principal: &str,
        request_key: &str,
        operation: &str,
        input_digest: &str,
        resumed_from_session_id: Option<&str>,
    ) -> AccessStoreResult<Option<AgentSessionAdmission>> {
        validate_request(request_key, operation, resumed_from_session_id)?;
        let (owner_kind, owner_id) = owner(&definition.owner);
        find_session_request(
            &self.connection,
            principal,
            owner_kind,
            owner_id,
            request_key,
            operation,
            &definition.id,
            definition.revision.version,
            input_digest,
            resumed_from_session_id,
        )
    }

    pub(crate) fn recover_expired_sessions(&self, now: i64) -> AccessStoreResult<usize> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(super::store::map_sqlite_error)?;
        let changed = tx
            .execute(
                "UPDATE agent_sessions SET status='interrupted' WHERE status IN ('admitted','running') AND lease_expires_at<=?1",
                [now],
            )
            .map_err(super::store::map_sqlite_error)?;
        tx.execute(
            "UPDATE agent_session_evidence SET error_code='runtime_restarted',updated_at=?1,completed_at=?1 WHERE session_id IN (SELECT session_id FROM agent_sessions WHERE status='interrupted' AND lease_expires_at<=?1) AND completed_at IS NULL",
            [now],
        )
        .map_err(super::store::map_sqlite_error)?;
        tx.commit().map_err(super::store::map_sqlite_error)?;
        Ok(changed)
    }

    pub(crate) fn session_status(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> AccessStoreResult<Option<String>> {
        self.connection
            .query_row(
                "SELECT status FROM agent_sessions WHERE agent_id=?1 AND session_id=?2",
                params![agent_id, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn session(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> AccessStoreResult<Option<AgentSessionRecord>> {
        self.connection
            .query_row(
                "SELECT s.session_id,s.agent_id,s.agent_version,s.principal_id,s.status,s.lease_expires_at,s.created_at,e.input_digest,e.input_text,e.output_digest,e.transcript,e.error_code,e.resumed_from_session_id,e.updated_at,e.completed_at FROM agent_sessions s JOIN agent_session_evidence e ON e.session_id=s.session_id WHERE s.agent_id=?1 AND s.session_id=?2",
                params![agent_id, session_id],
                decode_session,
            )
            .optional()
            .map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn list_sessions(
        &self,
        agent_id: &str,
        after: &str,
        limit: usize,
    ) -> AccessStoreResult<Vec<AgentSessionRecord>> {
        if limit == 0 || limit > 100 {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let mut statement = self.connection.prepare(
            "SELECT s.session_id,s.agent_id,s.agent_version,s.principal_id,s.status,s.lease_expires_at,s.created_at,e.input_digest,e.input_text,e.output_digest,e.transcript,e.error_code,e.resumed_from_session_id,e.updated_at,e.completed_at FROM agent_sessions s JOIN agent_session_evidence e ON e.session_id=s.session_id WHERE s.agent_id=?1 AND s.session_id>?2 ORDER BY s.session_id LIMIT ?3",
        ).map_err(super::store::map_sqlite_error)?;
        statement
            .query_map(
                params![
                    agent_id,
                    after,
                    i64::try_from(limit).map_err(|_| AccessStoreError::MalformedVocabulary)?
                ],
                decode_session,
            )
            .map_err(super::store::map_sqlite_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn set_session_status(
        &self,
        agent_id: &str,
        session_id: &str,
        expected: &str,
        next: &str,
    ) -> AccessStoreResult<()> {
        const VALID: &[&str] = &[
            "admitted",
            "running",
            "completed",
            "failed",
            "cancelled",
            "revoked",
            "interrupted",
        ];
        if !VALID.contains(&expected) || !VALID.contains(&next) {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let changed = self.connection.execute(
            "UPDATE agent_sessions SET status=?4 WHERE agent_id=?1 AND session_id=?2 AND status=?3",
            params![agent_id, session_id, expected, next],
        ).map_err(super::store::map_sqlite_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(AccessStoreError::NotAuthorized)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn settle_session(
        &self,
        agent_id: &str,
        session_id: &str,
        expected: &str,
        next: &str,
        output_digest: Option<&str>,
        transcript: Option<&str>,
        error_code: Option<&str>,
        now: i64,
    ) -> AccessStoreResult<()> {
        const TERMINAL: &[&str] = &["completed", "failed", "cancelled", "revoked", "interrupted"];
        if !matches!(expected, "admitted" | "running")
            || !TERMINAL.contains(&next)
            || transcript.is_some_and(|value| value.len() > 1024 * 1024)
            || error_code.is_some_and(|value| value.len() > 128)
        {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(super::store::map_sqlite_error)?;
        let changed = tx
            .execute(
                "UPDATE agent_sessions SET status=?4 WHERE agent_id=?1 AND session_id=?2 AND status=?3",
                params![agent_id, session_id, expected, next],
            )
            .map_err(super::store::map_sqlite_error)?;
        if changed != 1 {
            return Err(AccessStoreError::NotAuthorized);
        }
        tx.execute(
            "UPDATE agent_session_evidence SET output_digest=?2,transcript=?3,error_code=?4,updated_at=?5,completed_at=?5 WHERE session_id=?1",
            params![session_id, output_digest, transcript, error_code, now],
        )
        .map_err(super::store::map_sqlite_error)?;
        tx.commit().map_err(super::store::map_sqlite_error)
    }
}

fn decode_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentSessionRecord> {
    Ok(AgentSessionRecord {
        session_id: row.get(0)?,
        agent_id: row.get(1)?,
        agent_version: u64::try_from(row.get::<_, i64>(2)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        principal_id: row.get(3)?,
        status: row.get(4)?,
        lease_expires_at: row.get(5)?,
        created_at: row.get(6)?,
        input_digest: row.get(7)?,
        input_text: row.get(8)?,
        output_digest: row.get(9)?,
        transcript: row.get(10)?,
        error_code: row.get(11)?,
        resumed_from_session_id: row.get(12)?,
        updated_at: row.get(13)?,
        completed_at: row.get(14)?,
    })
}

fn validate_input(input_digest: &str, input_text: &str) -> AccessStoreResult<()> {
    use sha2::{Digest as _, Sha256};
    if input_text.is_empty()
        || input_text.len() > 1024 * 1024
        || format!(
            "sha256:{}",
            hex::encode(Sha256::digest(input_text.as_bytes()))
        ) != input_digest
    {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    Ok(())
}

fn validate_admission(
    session_id: &str,
    request_key: &str,
    operation: &str,
    input_digest: &str,
    input_text: &str,
    resumed_from_session_id: Option<&str>,
) -> AccessStoreResult<()> {
    validate_input(input_digest, input_text)?;
    validate_request(request_key, operation, resumed_from_session_id)?;
    if session_id.is_empty() || resumed_from_session_id == Some(session_id) {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    Ok(())
}

fn validate_request(
    request_key: &str,
    operation: &str,
    resumed_from_session_id: Option<&str>,
) -> AccessStoreResult<()> {
    if request_key.is_empty()
        || request_key.len() > 256
        || request_key.trim() != request_key
        || request_key.chars().any(char::is_control)
        || !matches!(
            (operation, resumed_from_session_id),
            ("run", None) | ("resume", Some(_))
        )
    {
        return Err(AccessStoreError::MalformedVocabulary);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn find_session_request(
    connection: &Connection,
    principal: &str,
    owner_kind: &str,
    owner_id: &str,
    request_key: &str,
    operation: &str,
    agent_id: &str,
    agent_version: u64,
    input_digest: &str,
    resumed_from_session_id: Option<&str>,
) -> AccessStoreResult<Option<AgentSessionAdmission>> {
    let existing = connection
        .query_row(
            "SELECT operation,agent_id,agent_version,input_digest,resumed_from_session_id,session_id FROM agent_session_requests WHERE principal_id=?1 AND owner_kind=?2 AND owner_id=?3 AND request_key=?4",
            params![principal, owner_kind, owner_id, request_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(super::store::map_sqlite_error)?;
    let Some((
        stored_operation,
        stored_agent_id,
        stored_version,
        stored_input,
        stored_source,
        session_id,
    )) = existing
    else {
        return Ok(None);
    };
    let requested_version =
        i64::try_from(agent_version).map_err(|_| AccessStoreError::MalformedVocabulary)?;
    if stored_operation != operation
        || stored_agent_id != agent_id
        || stored_version != requested_version
        || stored_input != input_digest
        || stored_source.as_deref() != resumed_from_session_id
    {
        return Ok(Some(AgentSessionAdmission::Conflict {
            existing_session_id: session_id,
        }));
    }
    let session = connection
        .query_row(
            "SELECT s.session_id,s.agent_id,s.agent_version,s.principal_id,s.status,s.lease_expires_at,s.created_at,e.input_digest,e.input_text,e.output_digest,e.transcript,e.error_code,e.resumed_from_session_id,e.updated_at,e.completed_at FROM agent_sessions s JOIN agent_session_evidence e ON e.session_id=s.session_id WHERE s.session_id=?1",
            [&session_id],
            decode_session,
        )
        .map_err(super::store::map_sqlite_error)?;
    Ok(Some(AgentSessionAdmission::Replay(session)))
}

pub(super) fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentDefinition> {
    use labby_primitives::access::{InstallationId, PrincipalId, ProjectId, TeamId};
    use labby_primitives::agent::{AgentRevision, RunningRevocationPolicy};
    let kind: String = row.get(0)?;
    let id: String = row.get(1)?;
    let payload: String = row.get(3)?;
    let value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let string = |key: &str| -> rusqlite::Result<String> {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .ok_or(rusqlite::Error::InvalidQuery)
    };
    if value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || value
            .get("capabilitySchemaVersion")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(
                labby_primitives::access::Capability::SCHEMA_VERSION.get(),
            ))
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let required_capabilities = value
        .get("requiredCapabilities")
        .and_then(serde_json::Value::as_array)
        .ok_or(rusqlite::Error::InvalidQuery)?
        .iter()
        .map(|value| {
            value
                .as_str()
                .and_then(|value| {
                    labby_primitives::access::Capability::from_wire(
                        labby_primitives::access::Capability::SCHEMA_VERSION,
                        value,
                    )
                })
                .ok_or(rusqlite::Error::InvalidQuery)
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let revocation_policy = match value
        .get("revocationPolicy")
        .and_then(serde_json::Value::as_str)
    {
        Some("stop_at_safe_boundary") => RunningRevocationPolicy::StopAtSafeBoundary,
        Some("stop_immediately") => RunningRevocationPolicy::StopImmediately,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let owner = match kind.as_str() {
        "installation" => OwnerScope::Installation(
            InstallationId::new(id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        ),
        "team" => OwnerScope::Team(TeamId::new(id).map_err(|_| rusqlite::Error::InvalidQuery)?),
        "project" => {
            OwnerScope::Project(ProjectId::new(id).map_err(|_| rusqlite::Error::InvalidQuery)?)
        }
        "personal" => {
            OwnerScope::Personal(PrincipalId::new(id).map_err(|_| rusqlite::Error::InvalidQuery)?)
        }
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let definition = AgentDefinition {
        id: string("agentId")?,
        owner,
        revision: AgentRevision {
            version: u64::try_from(row.get::<_, i64>(2)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            content_digest: string("contentDigest")?,
            repository_digest: string("repositoryDigest")?,
            image_digest: string("imageDigest")?,
            harness_digest: string("harnessDigest")?,
            loadout_digest: string("loadoutDigest")?,
            catalog_generation: string("catalogGeneration")?,
            credential_references: value
                .get("credentialReferences")
                .and_then(serde_json::Value::as_array)
                .ok_or(rusqlite::Error::InvalidQuery)?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .ok_or(rusqlite::Error::InvalidQuery)
                })
                .collect::<rusqlite::Result<Vec<_>>>()?,
        },
        state: match row.get::<_, String>(4)?.as_str() {
            "active" => AgentState::Active,
            "suspended" => AgentState::Suspended,
            _ => AgentState::Deleted,
        },
        required_capabilities,
        authority_epoch: u64::try_from(row.get::<_, i64>(5)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        publication_epoch: u64::try_from(row.get::<_, i64>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        revocation_policy,
    };
    definition
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(definition)
}

fn owner(owner: &OwnerScope) -> (&'static str, &str) {
    match owner {
        OwnerScope::Installation(id) => ("installation", id.as_str()),
        OwnerScope::Team(id) => ("team", id.as_str()),
        OwnerScope::Project(id) => ("project", id.as_str()),
        OwnerScope::Personal(id) => ("personal", id.as_str()),
    }
}
fn state(value: AgentState) -> &'static str {
    match value {
        AgentState::Active => "active",
        AgentState::Suspended => "suspended",
        AgentState::Deleted => "deleted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::{
        access::{Capability, PrincipalId},
        agent::{AgentRevision, RunningRevocationPolicy},
    };
    fn digest() -> String {
        format!("sha256:{}", "a".repeat(64))
    }
    fn input_digest() -> String {
        input_digest_for("test input")
    }
    fn input_digest_for(input: &str) -> String {
        use sha2::{Digest as _, Sha256};
        format!("sha256:{}", hex::encode(Sha256::digest(input.as_bytes())))
    }
    fn definition(version: u64) -> AgentDefinition {
        AgentDefinition {
            id: "agent-1".into(),
            owner: OwnerScope::Personal(PrincipalId::new("p-1").unwrap()),
            revision: AgentRevision {
                version,
                content_digest: digest(),
                repository_digest: digest(),
                image_digest: digest(),
                harness_digest: digest(),
                loadout_digest: digest(),
                catalog_generation: "catalog-1".into(),
                credential_references: vec![],
            },
            state: AgentState::Active,
            required_capabilities: vec![Capability::ScopeOperate],
            authority_epoch: 1,
            publication_epoch: 1,
            revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
        }
    }
    #[test]
    fn definitions_and_audit_commit_together_and_versions_are_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(super::super::migrations::AGENT_TASK_SCHEMA)
            .unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch(super::super::migrations::EXECUTION_EVIDENCE_SCHEMA)
            .unwrap();
        let mut store = AgentDefinitionStore::open(&path).unwrap();
        store.put(&definition(1), "p-1", 1).unwrap();
        store.put(&definition(2), "p-1", 2).unwrap();
        assert!(store.put(&definition(4), "p-1", 3).is_err());
        let counts:(i64,i64)=store.connection.query_row("SELECT (SELECT count(*) FROM agent_definitions),(SELECT count(*) FROM agent_definition_audit)",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(counts, (1, 2));
        assert_eq!(store.get("agent-1").unwrap(), Some(definition(2)));
        assert_eq!(store.list_page("", 100).unwrap(), vec![definition(2)]);
        let current = definition(2);
        assert_eq!(
            store
                .create_session(
                    "session-1",
                    &current,
                    "p-1",
                    "request-1",
                    "run",
                    "authority-1",
                    100,
                    &input_digest(),
                    "test input",
                    None,
                    4,
                )
                .unwrap(),
            AgentSessionAdmission::Created
        );
        assert_eq!(
            store.session_status("agent-1", "session-1").unwrap(),
            Some("admitted".to_owned())
        );
        assert_eq!(store.session_status("agent-1", "guessed").unwrap(), None);
        let session = store.session("agent-1", "session-1").unwrap().unwrap();
        assert_eq!(session.input_text, "test input");
        assert_eq!(session.input_digest, input_digest());
        store
            .set_session_status("agent-1", "session-1", "admitted", "running")
            .unwrap();
        store
            .settle_session(
                "agent-1",
                "session-1",
                "running",
                "completed",
                Some(&digest()),
                Some("retained output"),
                None,
                5,
            )
            .unwrap();
        let session = store.session("agent-1", "session-1").unwrap().unwrap();
        assert_eq!(session.status, "completed");
        assert_eq!(session.output_digest.as_deref(), Some(digest().as_str()));
        assert_eq!(session.transcript.as_deref(), Some("retained output"));
        assert_eq!(session.completed_at, Some(5));
    }

    #[test]
    fn expired_live_sessions_recover_as_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(super::super::migrations::AGENT_TASK_SCHEMA)
            .unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch(super::super::migrations::EXECUTION_EVIDENCE_SCHEMA)
            .unwrap();
        let mut store = AgentDefinitionStore::open(&path).unwrap();
        let current = definition(1);
        store.put(&current, "p-1", 1).unwrap();
        assert_eq!(
            store
                .create_session(
                    "session-1",
                    &current,
                    "p-1",
                    "request-1",
                    "run",
                    "authority-1",
                    100,
                    &input_digest(),
                    "test input",
                    None,
                    2,
                )
                .unwrap(),
            AgentSessionAdmission::Created
        );
        store
            .set_session_status("agent-1", "session-1", "admitted", "running")
            .unwrap();
        assert_eq!(store.recover_expired_sessions(99).unwrap(), 0);
        assert_eq!(
            store
                .session_status("agent-1", "session-1")
                .unwrap()
                .as_deref(),
            Some("running")
        );
        assert_eq!(store.recover_expired_sessions(100).unwrap(), 1);
        assert_eq!(
            store
                .session_status("agent-1", "session-1")
                .unwrap()
                .as_deref(),
            Some("interrupted")
        );
        let recovered = store.session("agent-1", "session-1").unwrap().unwrap();
        assert_eq!(recovered.error_code.as_deref(), Some("runtime_restarted"));
        assert_eq!(recovered.completed_at, Some(100));
    }

    #[test]
    fn session_request_replay_is_restart_safe_and_conflicts_on_changed_intent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(super::super::migrations::AGENT_TASK_SCHEMA)
            .unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch(super::super::migrations::EXECUTION_EVIDENCE_SCHEMA)
            .unwrap();
        let mut store = AgentDefinitionStore::open(&path).unwrap();
        let current = definition(1);
        store.put(&current, "p-1", 1).unwrap();
        assert_eq!(
            store
                .create_session(
                    "session-original",
                    &current,
                    "p-1",
                    "logical-request",
                    "run",
                    "authority-1",
                    100,
                    &input_digest(),
                    "test input",
                    None,
                    2,
                )
                .unwrap(),
            AgentSessionAdmission::Created
        );
        drop(store);

        let mut store = AgentDefinitionStore::open(&path).unwrap();
        let replay = store
            .session_request(
                &current,
                "p-1",
                "logical-request",
                "run",
                &input_digest(),
                None,
            )
            .unwrap();
        assert!(matches!(
            replay,
            Some(AgentSessionAdmission::Replay(AgentSessionRecord { session_id, .. }))
                if session_id == "session-original"
        ));
        assert!(matches!(
            store
                .create_session(
                    "session-never-inserted",
                    &current,
                    "p-1",
                    "logical-request",
                    "run",
                    "authority-2",
                    200,
                    &input_digest(),
                    "test input",
                    None,
                    3,
                )
                .unwrap(),
            AgentSessionAdmission::Replay(AgentSessionRecord { session_id, .. })
                if session_id == "session-original"
        ));
        let changed_input = "changed input";
        assert_eq!(
            store
                .create_session(
                    "session-conflict",
                    &current,
                    "p-1",
                    "logical-request",
                    "run",
                    "authority-2",
                    200,
                    &input_digest_for(changed_input),
                    changed_input,
                    None,
                    3,
                )
                .unwrap(),
            AgentSessionAdmission::Conflict {
                existing_session_id: "session-original".into()
            }
        );
        let session_count = store
            .connection
            .query_row("SELECT count(*) FROM agent_sessions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(session_count, 1);
    }

    #[test]
    fn legacy_or_malformed_security_definition_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(super::super::migrations::AGENT_TASK_SCHEMA)
            .unwrap();
        let mut store = AgentDefinitionStore::open(&path).unwrap();
        store.put(&definition(1), "p-1", 1).unwrap();
        store.connection.execute(
            "UPDATE agent_definitions SET definition_json=json_remove(definition_json,'$.requiredCapabilities') WHERE agent_id='agent-1'",
            [],
        ).unwrap();
        assert!(store.get("agent-1").is_err());
    }
}
