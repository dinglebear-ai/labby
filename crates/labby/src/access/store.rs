use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rusqlite::TransactionBehavior;
#[cfg(test)]
use rusqlite::types::Value;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension};

use super::authorization::{
    AuthorizeProjectInput, LibraryAccessSnapshot, ProjectPermissionSnapshot,
};
use super::bootstrap::{BootstrapOutcome, BootstrapOwnerInput, bootstrap_owner};
use super::error::{AccessStoreError, AccessStoreResult};
use super::loadout::{AssignProjectLoadoutInput, AssignProjectLoadoutOutcome};
use super::read::{AccessibleProjectSnapshot, ProjectAccessSnapshot, SessionAuthoritySnapshot};
use super::team::{
    AcceptTeamInvitationInput, AddTeamMemberInput, AssignTeamProjectInput, CreateTeamInput,
    CreateTeamInvitationInput, EffectiveProjectRoleSnapshot, PlatformAdministratorInput,
    TeamInvitationSnapshot, TeamMembershipInput, TeamMembershipSnapshot,
    TeamProjectAssignmentSnapshot, TeamSnapshot,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const AUTHORIZED_OWNERS_CTE: &str = "authorized_owners(owner_kind,owner_id) AS (SELECT 'personal',?3 UNION SELECT 'team',g.group_id FROM groups g JOIN team_memberships tm ON tm.organization_id=g.organization_id AND tm.team_id=g.group_id WHERE tm.principal_id=?3 AND tm.status='active' AND g.kind='team' AND g.status='active' UNION SELECT 'project',p.project_id FROM projects p JOIN project_memberships pm ON pm.organization_id=p.organization_id AND pm.project_id=p.project_id WHERE pm.principal_id=?3 AND pm.status='active' AND p.status='active' UNION SELECT 'project',p.project_id FROM projects p JOIN team_project_assignments a ON a.organization_id=p.organization_id AND a.project_id=p.project_id JOIN groups g ON g.organization_id=a.organization_id AND g.group_id=a.team_id JOIN team_memberships tm ON tm.organization_id=g.organization_id AND tm.team_id=g.group_id WHERE tm.principal_id=?3 AND tm.status='active' AND a.status='active' AND g.kind='team' AND g.status='active' AND p.status='active' UNION SELECT 'personal',p.principal_id FROM principals p WHERE ?4 AND p.status='active' UNION SELECT 'team',g.group_id FROM groups g WHERE ?4 AND g.kind='team' AND g.status='active' UNION SELECT 'project',p.project_id FROM projects p WHERE ?4 AND p.status='active')";
#[derive(Clone)]
pub(crate) struct AccessStore {
    connection: Arc<Mutex<Connection>>,
    connection_admission: Arc<tokio::sync::Semaphore>,
    file_stash_principal_gates: Arc<Mutex<HashMap<String, Weak<tokio::sync::RwLock<()>>>>>,
    path: Arc<PathBuf>,
    #[cfg(test)]
    skill_library_authorizations: Arc<AtomicUsize>,
    #[cfg(test)]
    bulk_authorization_transactions: Arc<AtomicUsize>,
}

impl std::fmt::Debug for AccessStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccessStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl AccessStore {
    pub(crate) async fn installation_id(&self) -> AccessStoreResult<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT installation_id FROM access_installations WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite_error)
        })
        .await
    }

    pub(crate) async fn authorize_action_batch(
        &self,
        requests: Vec<super::AuthorityRequest>,
    ) -> AccessStoreResult<Vec<bool>> {
        #[cfg(test)]
        let transaction_count = Arc::clone(&self.bulk_authorization_transactions);
        self.with_connection(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(map_sqlite_error)?;
            #[cfg(test)]
            transaction_count.fetch_add(1, Ordering::Relaxed);
            let mut authorized = Vec::with_capacity(requests.len());
            for request in requests {
                match super::authority::authorize_action_in_transaction(&transaction, request) {
                    Ok(_) => authorized.push(true),
                    Err(AccessStoreError::NotAuthorized) => authorized.push(false),
                    Err(error) => return Err(error),
                }
            }
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(authorized)
        })
        .await
    }

    #[cfg(test)]
    pub(crate) fn bulk_authorization_transaction_count(&self) -> usize {
        self.bulk_authorization_transactions.load(Ordering::Relaxed)
    }
    #[cfg(feature = "gateway")]
    pub(crate) async fn get_team_gateway_credential_binding(
        &self,
        team_id: String,
        upstream_name: String,
    ) -> AccessStoreResult<Option<labby_runtime::gateway_authority::TeamCredentialBinding>> {
        self.with_connection(move |connection| {
            super::gateway_credential::get(connection, &team_id, &upstream_name)
        })
        .await
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn put_team_gateway_credential_binding(
        &self,
        input: super::gateway_credential::PutTeamCredentialBinding,
    ) -> AccessStoreResult<labby_runtime::gateway_authority::TeamCredentialBinding> {
        self.with_connection(move |connection| super::gateway_credential::put(connection, &input))
            .await
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn put_team_gateway_credential_binding_authorized(
        &self,
        request: super::AuthorityRequest,
        input: super::gateway_credential::PutTeamCredentialBinding,
    ) -> AccessStoreResult<labby_runtime::gateway_authority::TeamCredentialBinding> {
        self.with_connection(move |connection| {
            super::gateway_credential::put_authorized(connection, request, &input)
        })
        .await
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn list_team_gateway_credential_bindings(
        &self,
        team_id: String,
    ) -> AccessStoreResult<Vec<labby_runtime::gateway_authority::TeamCredentialBinding>> {
        self.with_connection(move |connection| {
            super::gateway_credential::list(connection, &team_id)
        })
        .await
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn revoke_team_gateway_credential_binding_authorized(
        &self,
        request: super::AuthorityRequest,
        team_id: String,
        upstream_name: String,
        now_millis: u64,
    ) -> AccessStoreResult<labby_runtime::gateway_authority::TeamCredentialBinding> {
        self.with_connection(move |connection| {
            super::gateway_credential::revoke_authorized(
                connection,
                request,
                &team_id,
                &upstream_name,
                now_millis,
            )
        })
        .await
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn revoke_team_gateway_credential_binding(
        &self,
        team_id: String,
        upstream_name: String,
        now_millis: u64,
    ) -> AccessStoreResult<Option<labby_runtime::gateway_authority::TeamCredentialBinding>> {
        // Revoking a missing or already-revoked binding is
        // `TeamCredentialBindingUnavailable`; the `Option` wrapper is retained
        // for the adapter signature and is always `Some` on success.
        self.with_connection(move |connection| {
            super::gateway_credential::revoke(connection, &team_id, &upstream_name, now_millis)
                .map(Some)
        })
        .await
    }

    pub(crate) async fn create_agent_task(
        &self,
        intent: labby_primitives::task::TaskIntent,
        now: i64,
    ) -> AccessStoreResult<String> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::task::TaskStore::open(&path)?.create(&intent, now)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn authorize_and_create_agent_task(
        &self,
        request: super::AuthorityRequest,
        mut intent: labby_primitives::task::TaskIntent,
        now: i64,
    ) -> AccessStoreResult<String> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
            if lease.binding().owner_scope() != &intent.owner
                || lease.binding().resource_id().as_str() != intent.id
            {
                return Err(AccessStoreError::NotAuthorized);
            }
            intent.creator = labby_primitives::access::PrincipalId::new(lease.binding().principal_id().to_owned()).map_err(|_| AccessStoreError::MalformedVocabulary)?;
            let (owner_kind, owner_id) = match &intent.owner {
                labby_primitives::access::OwnerScope::Installation(id) => ("installation", id.as_str()),
                labby_primitives::access::OwnerScope::Team(id) => ("team", id.as_str()),
                labby_primitives::access::OwnerScope::Project(id) => ("project", id.as_str()),
                labby_primitives::access::OwnerScope::Personal(id) => ("personal", id.as_str()),
            };
            let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_definitions WHERE agent_id=?1 AND owner_kind=?2 AND owner_id=?3 AND version=?4 AND state='active' AND json_extract(definition_json,'$.contentDigest')=?5)", rusqlite::params![intent.agent_id, owner_kind, owner_id, i64::try_from(intent.agent_version).map_err(|_| AccessStoreError::MalformedVocabulary)?, intent.agent_revision_digest], |row| row.get(0)).map_err(map_sqlite_error)?;
            if !active { return Err(AccessStoreError::NotAuthorized); }
            let id = super::task::TaskStore::create_in_transaction(&tx, &intent, now)?;
            tx.commit().map_err(map_sqlite_error)?;
            Ok(id)
        }).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn authorize_and_transition_agent_task(
        &self,
        request: super::AuthorityRequest,
        id: String,
        from: labby_primitives::task::TaskState,
        to: labby_primitives::task::TaskState,
        actor: String,
        attempt: u32,
        now: i64,
    ) -> AccessStoreResult<labby_runtime::authority::AuthorityLease> {
        self.with_connection(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
            let task_owner: Option<(String, String)> = tx
                .query_row(
                    "SELECT owner_kind,owner_id FROM agent_tasks WHERE task_id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite_error)?;
            let (owner_kind, owner_id) = task_owner.ok_or(AccessStoreError::NotAuthorized)?;
            if lease.binding().resource_id().as_str() != id
                || !owner_matches(lease.binding().owner_scope(), &owner_kind, &owner_id)
            {
                return Err(AccessStoreError::NotAuthorized);
            }
            super::task::TaskStore::transition_in_transaction(
                &tx, &id, from, to, &actor, attempt, None, None, now,
            )?;
            tx.commit().map_err(map_sqlite_error)?;
            Ok(lease)
        })
        .await
    }

    pub(crate) async fn get_agent_task(
        &self,
        id: String,
    ) -> AccessStoreResult<Option<super::task::TaskRecord>> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || super::task::TaskStore::open(&path)?.get(&id))
            .await
            .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn list_agent_tasks(
        &self,
        after: String,
        limit: usize,
    ) -> AccessStoreResult<Vec<super::task::TaskRecord>> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::task::TaskStore::open(&path)?.list_page(&after, limit)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn list_authorized_agent_tasks(
        &self,
        after: String,
        limit: usize,
        request: super::AuthorityRequest,
    ) -> AccessStoreResult<Vec<super::task::TaskRecord>> {
        #[cfg(test)]
        let transaction_count = Arc::clone(&self.bulk_authorization_transactions);
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred).map_err(map_sqlite_error)?;
            #[cfg(test)]
            transaction_count.fetch_add(1, Ordering::Relaxed);
            let principal = super::read::resolve_principal(&tx, request.identity()).map_err(super::authority::collapse_denial)?;
            let admin: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM platform_administrators WHERE principal_id=?1 AND status='active')", [&principal.id], |r| r.get(0)).map_err(map_sqlite_error)?;
            let sql = format!("WITH {AUTHORIZED_OWNERS_CTE}, visible AS (SELECT t.* FROM agent_tasks t JOIN authorized_owners a USING(owner_kind,owner_id) UNION ALL SELECT t.* FROM agent_tasks t WHERE ?4 AND t.owner_kind='installation') SELECT task_id,idempotency_key,owner_kind,owner_id,project_id,creator_principal_id,agent_id,agent_version,agent_revision_digest,input_digest,catalog_generation,authority_fingerprint,state,attempt,output_digest,error_code FROM visible WHERE task_id>?1 ORDER BY task_id LIMIT ?2");
            let mut statement = tx.prepare(&sql).map_err(map_sqlite_error)?;
            let records = statement.query_map(rusqlite::params![after, i64::try_from(limit).map_err(|_| AccessStoreError::MalformedVocabulary)?, principal.id, admin], super::task::decode).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)?;
            drop(statement);
            // Per-record re-authorization filters the page: a row the caller
            // may not act on is omitted, it does not abort the listing.
            let mut authorized = Vec::with_capacity(records.len());
            for record in records {
                let resource = labby_primitives::access::ResourceRef::new(record.intent.owner.clone(), labby_primitives::access::ResourceFamily::Task, labby_primitives::access::ResourceId::new(record.intent.id.clone()).map_err(|_| AccessStoreError::MalformedVocabulary)?);
                match super::authority::authorize_action_in_transaction(&tx, request.for_resource(resource)) {
                    Ok(_) => authorized.push(record),
                    Err(AccessStoreError::NotAuthorized) => {}
                    Err(error) => return Err(error),
                }
            }
            tx.commit().map_err(map_sqlite_error)?;
            Ok(authorized)
        }).await
    }

    pub(crate) async fn transition_agent_task(
        &self,
        id: String,
        from: labby_primitives::task::TaskState,
        to: labby_primitives::task::TaskState,
        actor: String,
        attempt: u32,
        now: i64,
    ) -> AccessStoreResult<()> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::task::TaskStore::open(&path)?
                .transition(&id, from, to, &actor, attempt, None, None, now)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn acquire_agent_task_lease(
        &self,
        id: String,
        attempt: u32,
        fence: String,
        expires_at: i64,
        now: i64,
    ) -> AccessStoreResult<()> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::task::TaskStore::open(&path)?
                .acquire_lease(&id, attempt, &fence, expires_at, now)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn settle_agent_task(
        &self,
        id: String,
        from: labby_primitives::task::TaskState,
        to: labby_primitives::task::TaskState,
        actor: String,
        attempt: u32,
        fence: String,
        settlement: labby_primitives::task::TaskSettlement,
        now: i64,
    ) -> AccessStoreResult<()> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::task::TaskStore::open(&path)?.transition(
                &id,
                from,
                to,
                &actor,
                attempt,
                Some(&fence),
                Some(&settlement),
                now,
            )
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn recover_expired_agent_tasks(&self, now: i64) -> AccessStoreResult<usize> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::task::TaskStore::open(&path)?.recover_expired(now)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn put_agent_definition(
        &self,
        definition: labby_primitives::agent::AgentDefinition,
        actor: String,
        now: i64,
    ) -> AccessStoreResult<()> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.put(&definition, &actor, now)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn authorize_and_put_agent_definition(
        &self,
        request: super::AuthorityRequest,
        definition: labby_primitives::agent::AgentDefinition,
        actor: String,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
            if lease.binding().owner_scope() != &definition.owner
                || lease.binding().resource_id().as_str() != definition.id
            {
                return Err(AccessStoreError::NotAuthorized);
            }
            super::agent::AgentDefinitionStore::put_in_transaction(&tx, &definition, &actor, now)?;
            tx.commit().map_err(map_sqlite_error)
        })
        .await
    }

    pub(crate) async fn authorize_and_set_agent_definition_state(
        &self,
        request: super::AuthorityRequest,
        id: String,
        state: labby_primitives::agent::AgentState,
        actor: String,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
            let agent_owner: Option<(String, String)> = tx
                .query_row(
                    "SELECT owner_kind,owner_id FROM agent_definitions WHERE agent_id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(map_sqlite_error)?;
            let (owner_kind, owner_id) = agent_owner.ok_or(AccessStoreError::NotAuthorized)?;
            if lease.binding().resource_id().as_str() != id
                || !owner_matches(lease.binding().owner_scope(), &owner_kind, &owner_id)
            {
                return Err(AccessStoreError::NotAuthorized);
            }
            super::agent::AgentDefinitionStore::set_state_in_transaction(
                &tx, &id, state, &actor, now,
            )?;
            tx.commit().map_err(map_sqlite_error)
        })
        .await
    }

    pub(crate) async fn create_agent_session(
        &self,
        session_id: String,
        definition: labby_primitives::agent::AgentDefinition,
        principal: String,
        authority_fingerprint: String,
        lease_expires_at: i64,
        now: i64,
    ) -> AccessStoreResult<()> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.create_session(
                &session_id,
                &definition,
                &principal,
                &authority_fingerprint,
                lease_expires_at,
                now,
            )
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn recover_expired_agent_sessions(
        &self,
        now: i64,
    ) -> AccessStoreResult<usize> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.recover_expired_sessions(now)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn get_agent_session_status(
        &self,
        agent_id: String,
        session_id: String,
    ) -> AccessStoreResult<Option<String>> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.session_status(&agent_id, &session_id)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn set_agent_session_status(
        &self,
        agent_id: String,
        session_id: String,
        expected: String,
        next: String,
    ) -> AccessStoreResult<()> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.set_session_status(
                &agent_id,
                &session_id,
                &expected,
                &next,
            )
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn get_agent_definition(
        &self,
        id: String,
    ) -> AccessStoreResult<Option<labby_primitives::agent::AgentDefinition>> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.get(&id)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn list_agent_definitions(
        &self,
        after: String,
        limit: usize,
    ) -> AccessStoreResult<Vec<labby_primitives::agent::AgentDefinition>> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.list_page(&after, limit)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn list_authorized_agent_definitions(
        &self,
        after: String,
        limit: usize,
        request: super::AuthorityRequest,
    ) -> AccessStoreResult<Vec<labby_primitives::agent::AgentDefinition>> {
        #[cfg(test)]
        let transaction_count = Arc::clone(&self.bulk_authorization_transactions);
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred).map_err(map_sqlite_error)?;
            #[cfg(test)]
            transaction_count.fetch_add(1, Ordering::Relaxed);
            let principal = super::read::resolve_principal(&tx, request.identity()).map_err(super::authority::collapse_denial)?;
            let admin: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM platform_administrators WHERE principal_id=?1 AND status='active')", [&principal.id], |r| r.get(0)).map_err(map_sqlite_error)?;
            let sql = format!("WITH {AUTHORIZED_OWNERS_CTE}, visible AS (SELECT d.* FROM agent_definitions d JOIN authorized_owners a USING(owner_kind,owner_id) WHERE d.state!='deleted' UNION ALL SELECT d.* FROM agent_definitions d WHERE ?4 AND d.owner_kind='installation' AND d.state!='deleted') SELECT owner_kind,owner_id,version,definition_json,state,authority_epoch,publication_epoch FROM visible WHERE agent_id>?1 ORDER BY agent_id LIMIT ?2");
            let mut statement = tx.prepare(&sql).map_err(map_sqlite_error)?;
            let records = statement.query_map(rusqlite::params![after, i64::try_from(limit).map_err(|_| AccessStoreError::MalformedVocabulary)?, principal.id, admin], super::agent::decode).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)?;
            drop(statement);
            let mut authorized = Vec::with_capacity(records.len());
            for record in records {
                let resource = labby_primitives::access::ResourceRef::new(record.owner.clone(), labby_primitives::access::ResourceFamily::Agent, labby_primitives::access::ResourceId::new(record.id.clone()).map_err(|_| AccessStoreError::MalformedVocabulary)?);
                match super::authority::authorize_action_in_transaction(&tx, request.for_resource(resource)) {
                    Ok(_) => authorized.push(record),
                    Err(AccessStoreError::NotAuthorized) => {}
                    Err(error) => return Err(error),
                }
            }
            tx.commit().map_err(map_sqlite_error)?;
            Ok(authorized)
        }).await
    }

    pub(crate) async fn list_authorized_dev_containers(
        &self,
        after: String,
        limit: usize,
        request: super::AuthorityRequest,
    ) -> AccessStoreResult<Vec<super::RecoveryRecord>> {
        #[cfg(test)]
        let transaction_count = Arc::clone(&self.bulk_authorization_transactions);
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred).map_err(map_sqlite_error)?;
            #[cfg(test)]
            transaction_count.fetch_add(1, Ordering::Relaxed);
            let principal = super::read::resolve_principal(&tx, request.identity()).map_err(super::authority::collapse_denial)?;
            let admin: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM platform_administrators WHERE principal_id=?1 AND status='active')", [&principal.id], |r| r.get(0)).map_err(map_sqlite_error)?;
            let records = super::dev_container::authorized_recovery_inventory_page(&tx, &after, limit, &principal.id, admin).map_err(|error| match error {
                super::DevContainerLedgerError::InvalidInput => AccessStoreError::MalformedVocabulary,
                other => AccessStoreError::Unavailable(other.to_string()),
            })?;
            let mut authorized = Vec::with_capacity(records.len());
            for record in records {
                use labby_primitives::access::{InstallationId, PrincipalId, ProjectId, TeamId};
                let owner = match record.owner_kind {
                    labby_primitives::access::OwnerKind::Installation => labby_primitives::access::OwnerScope::Installation(InstallationId::new(record.owner_id.clone()).map_err(|_| AccessStoreError::MalformedVocabulary)?),
                    labby_primitives::access::OwnerKind::Team => labby_primitives::access::OwnerScope::Team(TeamId::new(record.owner_id.clone()).map_err(|_| AccessStoreError::MalformedVocabulary)?),
                    labby_primitives::access::OwnerKind::Project => labby_primitives::access::OwnerScope::Project(ProjectId::new(record.owner_id.clone()).map_err(|_| AccessStoreError::MalformedVocabulary)?),
                    labby_primitives::access::OwnerKind::Personal => labby_primitives::access::OwnerScope::Personal(PrincipalId::new(record.owner_id.clone()).map_err(|_| AccessStoreError::MalformedVocabulary)?),
                };
                let resource = labby_primitives::access::ResourceRef::new(owner, labby_primitives::access::ResourceFamily::DevContainer, labby_primitives::access::ResourceId::new(record.instance_id.clone()).map_err(|_| AccessStoreError::MalformedVocabulary)?);
                match super::authority::authorize_action_in_transaction(&tx, request.for_resource(resource)) {
                    Ok(_) => authorized.push(record),
                    Err(AccessStoreError::NotAuthorized) => {}
                    Err(error) => return Err(error),
                }
            }
            tx.commit().map_err(map_sqlite_error)?;
            Ok(authorized)
        }).await
    }

    pub(crate) async fn set_agent_definition_state(
        &self,
        id: String,
        state: labby_primitives::agent::AgentState,
        actor: String,
        now: i64,
    ) -> AccessStoreResult<()> {
        let path = Arc::clone(&self.path);
        tokio::task::spawn_blocking(move || {
            super::agent::AgentDefinitionStore::open(&path)?.set_state(&id, state, &actor, now)
        })
        .await
        .map_err(|error| AccessStoreError::Unavailable(error.to_string()))?
    }

    pub(crate) async fn claim_authority_projection_batch(
        &self,