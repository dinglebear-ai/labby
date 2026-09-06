//! Durable Agent definition records. Runtime sessions intentionally live elsewhere.

use labby_primitives::access::OwnerScope;
use labby_primitives::agent::{AgentDefinition, AgentState};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

use super::error::{AccessStoreError, AccessStoreResult};

pub(crate) struct AgentDefinitionStore {
    connection: Connection,
}

impl AgentDefinitionStore {
    pub(crate) fn open(path: &Path) -> AccessStoreResult<Self> {
        let connection = Connection::open(path).map_err(super::store::map_sqlite_error)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS agent_definitions(agent_id TEXT PRIMARY KEY,owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),owner_id TEXT NOT NULL,version INTEGER NOT NULL CHECK(version>0),definition_json TEXT NOT NULL CHECK(json_valid(definition_json)),state TEXT NOT NULL CHECK(state IN ('active','suspended','deleted')),authority_epoch INTEGER NOT NULL CHECK(authority_epoch>=0),publication_epoch INTEGER NOT NULL CHECK(publication_epoch>=0),updated_at INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS agent_definitions_owner ON agent_definitions(owner_kind,owner_id,state,agent_id); CREATE TABLE IF NOT EXISTS agent_definition_audit(event_id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,actor_principal_id TEXT NOT NULL,action TEXT NOT NULL CHECK(action IN ('create','update','suspend','delete')),authority_epoch INTEGER NOT NULL,occurred_at INTEGER NOT NULL,FOREIGN KEY(agent_id) REFERENCES agent_definitions(agent_id)); CREATE TABLE IF NOT EXISTS agent_sessions(session_id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,agent_version INTEGER NOT NULL,principal_id TEXT NOT NULL,authority_fingerprint TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN ('admitted','running','completed','failed','cancelled','revoked','interrupted')),lease_expires_at INTEGER NOT NULL,created_at INTEGER NOT NULL,FOREIGN KEY(agent_id) REFERENCES agent_definitions(agent_id)); CREATE INDEX IF NOT EXISTS agent_sessions_agent ON agent_sessions(agent_id,created_at,session_id);").map_err(super::store::map_sqlite_error)?;
        Ok(Self { connection })
    }

    pub(crate) fn put(
        &mut self,
        definition: &AgentDefinition,
        actor: &str,
        now: i64,
    ) -> AccessStoreResult<()> {
        definition
            .validate()
            .map_err(|_| AccessStoreError::MalformedVocabulary)?;
        let (owner_kind, owner_id) = owner(&definition.owner);
        let state = state(definition.state);
        let payload = serde_json::json!({"agentId": definition.id, "catalogGeneration": definition.revision.catalog_generation, "contentDigest": definition.revision.content_digest, "repositoryDigest": definition.revision.repository_digest, "imageDigest": definition.revision.image_digest, "harnessDigest": definition.revision.harness_digest, "loadoutDigest": definition.revision.loadout_digest, "credentialReferences": definition.revision.credential_references}).to_string();
        let tx = self
            .connection
            .transaction()
            .map_err(super::store::map_sqlite_error)?;
        let changed = tx.execute("INSERT INTO agent_definitions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(agent_id) DO UPDATE SET owner_kind=excluded.owner_kind,owner_id=excluded.owner_id,version=excluded.version,definition_json=excluded.definition_json,state=excluded.state,authority_epoch=excluded.authority_epoch,publication_epoch=excluded.publication_epoch,updated_at=excluded.updated_at WHERE excluded.version=agent_definitions.version+1", params![definition.id,owner_kind,owner_id,i64::try_from(definition.revision.version).map_err(|_|AccessStoreError::MalformedVocabulary)?,payload,state,i64::try_from(definition.authority_epoch).map_err(|_|AccessStoreError::MalformedVocabulary)?,i64::try_from(definition.publication_epoch).map_err(|_|AccessStoreError::MalformedVocabulary)?,now]).map_err(super::store::map_sqlite_error)?;
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
        tx.commit().map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn get(&self, id: &str) -> AccessStoreResult<Option<AgentDefinition>> {
        self.connection.query_row("SELECT owner_kind,owner_id,version,definition_json,state,authority_epoch,publication_epoch FROM agent_definitions WHERE agent_id=?1 AND state!='deleted'", [id], decode).optional().map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn list(&self) -> AccessStoreResult<Vec<AgentDefinition>> {
        let mut statement = self.connection.prepare("SELECT owner_kind,owner_id,version,definition_json,state,authority_epoch,publication_epoch FROM agent_definitions WHERE state!='deleted' ORDER BY owner_kind,owner_id,agent_id").map_err(super::store::map_sqlite_error)?;
        statement
            .query_map([], decode)
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
            .transaction()
            .map_err(super::store::map_sqlite_error)?;
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
        tx.commit().map_err(super::store::map_sqlite_error)
    }

    pub(crate) fn create_session(
        &self,
        session_id: &str,
        definition: &AgentDefinition,
        principal: &str,
        authority_fingerprint: &str,
        lease_expires_at: i64,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.connection.execute("INSERT INTO agent_sessions(session_id,agent_id,agent_version,principal_id,authority_fingerprint,status,lease_expires_at,created_at) VALUES(?1,?2,?3,?4,?5,'admitted',?6,?7)", params![session_id,definition.id,i64::try_from(definition.revision.version).map_err(|_|AccessStoreError::MalformedVocabulary)?,principal,authority_fingerprint,lease_expires_at,now]).map_err(super::store::map_sqlite_error)?;
        Ok(())
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
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentDefinition> {
    use labby_primitives::access::{InstallationId, PrincipalId, ProjectId, TeamId};
    use labby_primitives::agent::{AgentRevision, RunningRevocationPolicy};
    let kind: String = row.get(0)?;
    let id: String = row.get(1)?;
    let payload: String = row.get(3)?;
    let value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let string = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
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
    Ok(AgentDefinition {
        id: string("agentId"),
        owner,
        revision: AgentRevision {
            version: u64::try_from(row.get::<_, i64>(2)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            content_digest: string("contentDigest"),
            repository_digest: string("repositoryDigest"),
            image_digest: string("imageDigest"),
            harness_digest: string("harnessDigest"),
            loadout_digest: string("loadoutDigest"),
            catalog_generation: string("catalogGeneration"),
            credential_references: value
                .get("credentialReferences")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect(),
        },
        state: match row.get::<_, String>(4)?.as_str() {
            "active" => AgentState::Active,
            "suspended" => AgentState::Suspended,
            _ => AgentState::Deleted,
        },
        required_capabilities: vec![],
        authority_epoch: u64::try_from(row.get::<_, i64>(5)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        publication_epoch: u64::try_from(row.get::<_, i64>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
    })
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
        let mut store = AgentDefinitionStore::open(&dir.path().join("agents.db")).unwrap();
        store.put(&definition(1), "p-1", 1).unwrap();
        store.put(&definition(2), "p-1", 2).unwrap();
        assert!(store.put(&definition(4), "p-1", 3).is_err());
        let counts:(i64,i64)=store.connection.query_row("SELECT (SELECT count(*) FROM agent_definitions),(SELECT count(*) FROM agent_definition_audit)",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(counts, (1, 2));
        let current = definition(2);
        store
            .create_session("session-1", &current, "p-1", "authority-1", 100, 4)
            .unwrap();
        assert_eq!(
            store.session_status("agent-1", "session-1").unwrap(),
            Some("admitted".to_owned())
        );
        assert_eq!(store.session_status("agent-1", "guessed").unwrap(), None);
    }
}
