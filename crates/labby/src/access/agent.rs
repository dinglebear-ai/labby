//! Durable Agent definition records. Runtime sessions intentionally live elsewhere.

use labby_primitives::access::OwnerScope;
use labby_primitives::agent::{AgentDefinition, AgentState};
use rusqlite::{Connection, params};
use std::path::Path;

use super::error::{AccessStoreError, AccessStoreResult};

pub(crate) struct AgentDefinitionStore {
    connection: Connection,
}

impl AgentDefinitionStore {
    pub(crate) fn open(path: &Path) -> AccessStoreResult<Self> {
        let connection = Connection::open(path).map_err(super::store::map_sqlite_error)?;
        connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS agent_definitions(agent_id TEXT PRIMARY KEY,owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),owner_id TEXT NOT NULL,version INTEGER NOT NULL CHECK(version>0),definition_json TEXT NOT NULL CHECK(json_valid(definition_json)),state TEXT NOT NULL CHECK(state IN ('active','suspended','deleted')),authority_epoch INTEGER NOT NULL CHECK(authority_epoch>=0),publication_epoch INTEGER NOT NULL CHECK(publication_epoch>=0),updated_at INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS agent_definitions_owner ON agent_definitions(owner_kind,owner_id,state,agent_id); CREATE TABLE IF NOT EXISTS agent_definition_audit(event_id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,actor_principal_id TEXT NOT NULL,action TEXT NOT NULL CHECK(action IN ('create','update','suspend','delete')),authority_epoch INTEGER NOT NULL,occurred_at INTEGER NOT NULL,FOREIGN KEY(agent_id) REFERENCES agent_definitions(agent_id));").map_err(super::store::map_sqlite_error)?;
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
        let payload = serde_json::json!({"agentId": definition.id, "catalogGeneration": definition.revision.catalog_generation, "contentDigest": definition.revision.content_digest, "credentialReferences": definition.revision.credential_references}).to_string();
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
    }
}
