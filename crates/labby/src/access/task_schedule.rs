//! Durable schedule definitions and leased, idempotent occurrences.
use super::error::AccessStoreResult;
use super::store::map_sqlite_error;
use super::{AccessStore, AccessStoreError, AuthorityRequest};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TaskSchedule {
    pub schedule_id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub creator_principal_id: String,
    pub name: String,
    pub task_template_json: String,
    pub schedule_spec_json: String,
    pub retry_policy_json: String,
    #[serde(skip_serializing)]
    pub identity_ref_json: String,
    #[serde(skip_serializing)]
    pub ceiling_json: String,
    pub armed: bool,
    pub next_run_at: i64,
    pub revision: i64,
    pub last_task_id: Option<String>,
    pub last_error_kind: Option<String>,
    pub next_retry_at: Option<i64>,
}
const COLUMNS: &str = "schedule_id,owner_kind,owner_id,creator_principal_id,name,task_template_json,schedule_spec_json,identity_ref_json,ceiling_json,armed,next_run_at,revision,last_task_id,last_error_kind,retry_policy_json,(SELECT min(a.next_attempt_at) FROM agent_task_schedule_attempts a WHERE a.schedule_id=agent_task_schedules.schedule_id AND a.state='pending' AND a.attempt_number>0) AS next_retry_at";
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskSchedule> {
    Ok(TaskSchedule {
        schedule_id: row.get(0)?,
        owner_kind: row.get(1)?,
        owner_id: row.get(2)?,
        creator_principal_id: row.get(3)?,
        name: row.get(4)?,
        task_template_json: row.get(5)?,
        schedule_spec_json: row.get(6)?,
        identity_ref_json: row.get(7)?,
        ceiling_json: row.get(8)?,
        armed: row.get(9)?,
        next_run_at: row.get(10)?,
        revision: row.get(11)?,
        last_task_id: row.get(12)?,
        last_error_kind: row.get(13)?,
        retry_policy_json: row.get(14)?,
        next_retry_at: row.get(15)?,
    })
}
#[derive(Clone, Debug)]
pub(crate) struct ScheduleAdmission {
    pub schedule_id: String,
    pub occurrence_key: String,
    pub claim_token: String,
    pub revision: i64,
    pub attempt_number: u32,
}
#[derive(Clone, Debug)]
pub(crate) struct ScheduleOccurrence {
    pub schedule: TaskSchedule,
    pub key: String,
    pub task_id: String,
    pub original_task_id: String,
    pub claim_token: String,
    pub attempt_number: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RetryPolicy {
    pub max_retries: u32,
    pub backoff_ms: i64,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            backoff_ms: 300_000,
        }
    }
}
impl RetryPolicy {
    pub(crate) fn validate(&self) -> AccessStoreResult<()> {
        if self.max_retries > 10 || !(60_000..=86_400_000).contains(&self.backoff_ms) {
            return Err(AccessStoreError::MalformedVocabulary);
        }
        Ok(())
    }
}
impl AccessStore {
    pub(crate) async fn task_schedules_due(
        &self,
        now: i64,
    ) -> AccessStoreResult<Vec<TaskSchedule>> {
        self.with_connection(move |connection| {
            let mut query=connection.prepare(&format!("SELECT {COLUMNS} FROM agent_task_schedules WHERE armed=1 AND next_run_at<=?1 ORDER BY next_run_at,schedule_id LIMIT 100")).map_err(map_sqlite_error)?;
            query.query_map([now],decode).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)
        }).await
    }

    pub(crate) async fn task_schedule_get(
        &self,
        id: String,
    ) -> AccessStoreResult<Option<TaskSchedule>> {
        self.with_connection(move |connection| {
            connection
                .query_row(
                    &format!("SELECT {COLUMNS} FROM agent_task_schedules WHERE schedule_id=?1"),
                    [id],
                    decode,
                )
                .optional()
                .map_err(map_sqlite_error)
        })
        .await
    }
    /// Bounded candidate listing; dispatch applies the shared authority evaluator before returning rows.
    pub(crate) async fn task_schedule_page(
        &self,
        after: String,
        limit: u32,
    ) -> AccessStoreResult<Vec<TaskSchedule>> {
        self.with_connection(move |connection| {
            let mut query = connection.prepare(&format!("SELECT {COLUMNS} FROM agent_task_schedules WHERE schedule_id>?1 ORDER BY schedule_id LIMIT ?2")).map_err(map_sqlite_error)?;
            query.query_map(params![after, limit.min(100)], decode).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)
        }).await
    }
    pub(crate) async fn task_schedule_save(
        &self,
        row: TaskSchedule,
        request: AuthorityRequest,
        create: bool,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
            let (kind, id) = schedule_owner(lease.binding().owner_scope());
            if kind != row.owner_kind || id != row.owner_id || lease.binding().resource_id().as_str() != row.schedule_id { return Err(AccessStoreError::NotAuthorized); }
            if create {
                let count:i64=tx.query_row("SELECT count(*) FROM agent_task_schedules WHERE owner_kind=?1 AND owner_id=?2",params![row.owner_kind,row.owner_id],|r|r.get(0)).map_err(map_sqlite_error)?;
                if count>=100 {return Err(AccessStoreError::MalformedVocabulary);}

                tx.execute("INSERT INTO agent_task_schedules(schedule_id,owner_kind,owner_id,creator_principal_id,name,task_template_json,schedule_spec_json,identity_ref_json,ceiling_json,armed,next_run_at,revision,created_at,updated_at,retry_policy_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,1,?12,?12,?13)", params![row.schedule_id,row.owner_kind,row.owner_id,lease.binding().principal_id(),row.name,row.task_template_json,row.schedule_spec_json,row.identity_ref_json,row.ceiling_json,row.armed,row.next_run_at,now,row.retry_policy_json]).map_err(map_sqlite_error)?;
            } else {
                let changed = tx.execute("UPDATE agent_task_schedules SET name=?2,task_template_json=?3,schedule_spec_json=?4,identity_ref_json=?5,ceiling_json=?6,armed=?7,next_run_at=?8,revision=revision+1,updated_at=?9,retry_policy_json=?12 WHERE schedule_id=?1 AND revision=?10 AND creator_principal_id=?11", params![row.schedule_id,row.name,row.task_template_json,row.schedule_spec_json,row.identity_ref_json,row.ceiling_json,row.armed,row.next_run_at,now,row.revision,lease.binding().principal_id(),row.retry_policy_json]).map_err(map_sqlite_error)?;
                if changed != 1 { return Err(AccessStoreError::NotAuthorized); }
                tx.execute("UPDATE agent_task_schedule_occurrences SET state='skipped',error_kind='schedule_changed' WHERE schedule_id=?1 AND state='pending'", [&row.schedule_id]).map_err(map_sqlite_error)?;
                tx.execute("UPDATE agent_task_schedule_attempts SET state='skipped',error_kind='schedule_changed' WHERE schedule_id=?1 AND state='pending'", [&row.schedule_id]).map_err(map_sqlite_error)?;
            }
            tx.commit().map_err(map_sqlite_error)
        }).await
    }
    pub(crate) async fn task_schedule_delete(
        &self,
        id: String,
        request: AuthorityRequest,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
            let changed = tx.execute("DELETE FROM agent_task_schedules WHERE schedule_id=?1 AND creator_principal_id=?2", params![id,lease.binding().principal_id()]).map_err(map_sqlite_error)?;
            if changed != 1 { return Err(AccessStoreError::NotAuthorized); }
            tx.commit().map_err(map_sqlite_error)
        }).await
    }
    /// One definition per tick, atomic occurrence insertion plus advancing its cursor.
    pub(crate) async fn task_schedule_enqueue(
        &self,
        row: TaskSchedule,
        key: String,
        due_at: i64,
        next: Option<i64>,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let current: Option<(i64,bool,i64)> = tx.query_row("SELECT revision,armed,next_run_at FROM agent_task_schedules WHERE schedule_id=?1", [&row.schedule_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(map_sqlite_error)?;
            let manual = key.starts_with("manual:");
            if !current.is_some_and(|(revision,armed,due)| revision == row.revision && (manual || armed && due <= now)) { return Ok(()); }
            let pending:i64=tx.query_row("SELECT count(*) FROM agent_task_schedule_occurrences WHERE schedule_id=?1 AND state='pending'",[&row.schedule_id],|r|r.get(0)).map_err(map_sqlite_error)?;
            if pending>=100 {return Err(AccessStoreError::MalformedVocabulary);}
            let task_id = format!("schedule-{}", hex::encode(Sha256::digest(format!("{}:{key}",row.schedule_id))));
            tx.execute("INSERT OR IGNORE INTO agent_task_schedule_occurrences(schedule_id,occurrence_key,task_id,due_at,schedule_revision,state) VALUES(?1,?2,?3,?4,?5,'pending')", params![row.schedule_id,key,task_id,due_at,row.revision]).map_err(map_sqlite_error)?;
            tx.execute("INSERT OR IGNORE INTO agent_task_schedule_attempts(schedule_id,occurrence_key,attempt_number,task_id,next_attempt_at,state) VALUES(?1,?2,0,?3,?4,'pending')",params![row.schedule_id,key,task_id,due_at]).map_err(map_sqlite_error)?;
            if !manual {
                tx.execute("UPDATE agent_task_schedules SET next_run_at=?2,armed=?3,updated_at=?4 WHERE schedule_id=?1",params![row.schedule_id,next.unwrap_or(due_at),next.is_some(),now]).map_err(map_sqlite_error)?;
            }
            tx.commit().map_err(map_sqlite_error)
        }).await
    }
    /// Observe only settled immutable Tasks. An eligible failure creates a new
    /// delayed attempt atomically; a duplicate poll cannot create another retry.
    pub(crate) async fn task_schedule_observe_settlements(
        &self,
        now: i64,
    ) -> AccessStoreResult<usize> {
        self.with_connection(move |connection| {
            let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let completed={
                let mut query=tx.prepare("SELECT a.schedule_id,a.occurrence_key,a.attempt_number,a.task_id,t.state,t.error_code,s.retry_policy_json,o.schedule_revision,s.revision FROM agent_task_schedule_attempts a JOIN agent_tasks t ON t.task_id=a.task_id JOIN agent_task_schedule_occurrences o USING(schedule_id,occurrence_key) JOIN agent_task_schedules s ON s.schedule_id=a.schedule_id WHERE a.state='submitted' AND t.state IN ('succeeded','failed','cancelled','expired') ORDER BY a.next_attempt_at LIMIT 100").map_err(map_sqlite_error)?;
                query.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u32>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?,r.get::<_,i64>(7)?,r.get::<_,i64>(8)?))).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)?
            };
            for (id,key,attempt,task_id,state,error,policy_json,occurrence_revision,current_revision) in &completed {
                let policy:RetryPolicy=serde_json::from_str(policy_json).map_err(|_|AccessStoreError::MalformedVocabulary)?;
                policy.validate()?;
                let terminal=if state=="succeeded"{"succeeded"}else if state=="cancelled"{"cancelled"}else{"failed"};
                tx.execute("UPDATE agent_task_schedule_attempts SET state=?4,error_kind=?5 WHERE schedule_id=?1 AND occurrence_key=?2 AND attempt_number=?3 AND state='submitted'",params![id,key,attempt,terminal,error]).map_err(map_sqlite_error)?;
                // Invalid input, resource bounds, missing executors and authority
                // outcomes require intervention; never retry stop/cancel or lease loss.
                if occurrence_revision==current_revision && state=="failed" && error.as_deref()==Some("execution_failed") && *attempt<policy.max_retries {
                    let next_attempt=*attempt+1;
                    let retry_task_id=format!("schedule-{}",hex::encode(Sha256::digest(format!("{id}:{key}:retry:{next_attempt}"))));
                    let at=now.checked_add(policy.backoff_ms).ok_or(AccessStoreError::MalformedVocabulary)?;
                    tx.execute("INSERT OR IGNORE INTO agent_task_schedule_attempts(schedule_id,occurrence_key,attempt_number,task_id,next_attempt_at,state) VALUES(?1,?2,?3,?4,?5,'pending')",params![id,key,next_attempt,retry_task_id,at]).map_err(map_sqlite_error)?;
                    tx.execute("UPDATE agent_task_schedule_occurrences SET state='pending',error_kind=?3 WHERE schedule_id=?1 AND occurrence_key=?2",params![id,key,error]).map_err(map_sqlite_error)?;
                }
                tx.execute("UPDATE agent_task_schedules SET last_error_kind=?3 WHERE schedule_id=?1 AND last_task_id=?2",params![id,task_id,error]).map_err(map_sqlite_error)?;
            }
            tx.commit().map_err(map_sqlite_error)?;
            Ok(completed.len())
        }).await
    }
    pub(crate) async fn task_schedule_claim(
        &self,
        now: i64,
    ) -> AccessStoreResult<Option<ScheduleOccurrence>> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let pending: Option<(String,String,String,u32,String)> = tx.query_row("SELECT a.schedule_id,a.occurrence_key,a.task_id,a.attempt_number,o.task_id FROM agent_task_schedule_attempts a JOIN agent_task_schedule_occurrences o USING(schedule_id,occurrence_key) JOIN agent_task_schedules s ON s.schedule_id=o.schedule_id AND s.revision=o.schedule_revision WHERE a.state='pending' AND a.next_attempt_at<=?1 AND (a.claim_expires_at IS NULL OR a.claim_expires_at<=?1) ORDER BY a.next_attempt_at,a.schedule_id,a.occurrence_key LIMIT 1", [now], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(map_sqlite_error)?;
            let Some((id,key,task_id,attempt_number,original_task_id)) = pending else { return Ok(None); };
            let schedule = tx.query_row(&format!("SELECT {COLUMNS} FROM agent_task_schedules WHERE schedule_id=?1"), [&id], decode).map_err(map_sqlite_error)?;
            let claim_token = uuid::Uuid::new_v4().to_string();
            tx.execute("UPDATE agent_task_schedule_attempts SET claim_token=?3,claim_expires_at=?4 WHERE schedule_id=?1 AND occurrence_key=?2 AND attempt_number=?5", params![id,key,claim_token,now.saturating_add(60_000),attempt_number]).map_err(map_sqlite_error)?;
            tx.commit().map_err(map_sqlite_error)?;
            Ok(Some(ScheduleOccurrence { schedule, key, task_id, original_task_id, claim_token, attempt_number }))
        }).await
    }
    pub(crate) async fn task_schedule_validate_claim(
        &self,
        occurrence: ScheduleOccurrence,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let valid:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM agent_task_schedule_attempts a JOIN agent_task_schedule_occurrences o USING(schedule_id,occurrence_key) JOIN agent_task_schedules s ON s.schedule_id=o.schedule_id AND s.revision=o.schedule_revision WHERE a.schedule_id=?1 AND a.occurrence_key=?2 AND a.claim_token=?3 AND a.claim_expires_at>?4 AND a.state='pending' AND a.attempt_number=?5)",params![occurrence.schedule.schedule_id,occurrence.key,occurrence.claim_token,now,occurrence.attempt_number],|r|r.get(0)).map_err(map_sqlite_error)?;
            if valid {Ok(())}else{Err(AccessStoreError::NotAuthorized)}
        }).await
    }
    pub(crate) async fn task_schedule_finish(
        &self,
        occurrence: ScheduleOccurrence,
        error: Option<String>,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let changed = tx.execute("UPDATE agent_task_schedule_attempts SET state=?4,error_kind=?5,claim_token=NULL,claim_expires_at=NULL WHERE schedule_id=?1 AND occurrence_key=?2 AND claim_token=?3 AND state='pending' AND attempt_number=?6", params![occurrence.schedule.schedule_id,occurrence.key,occurrence.claim_token,if error.is_some(){"skipped"}else{"submitted"},error,occurrence.attempt_number]).map_err(map_sqlite_error)?;
            if changed == 1 {
                tx.execute("UPDATE agent_task_schedule_occurrences SET state=?3,error_kind=?4 WHERE schedule_id=?1 AND occurrence_key=?2",params![occurrence.schedule.schedule_id,occurrence.key,if error.is_some(){"skipped"}else{"submitted"},error]).map_err(map_sqlite_error)?;
                tx.execute("UPDATE agent_task_schedules SET last_task_id=?2,last_error_kind=?3 WHERE schedule_id=?1", params![occurrence.schedule.schedule_id,occurrence.task_id,error]).map_err(map_sqlite_error)?;
            }
            tx.execute("DELETE FROM agent_task_schedule_occurrences WHERE schedule_id=?1 AND state<>'pending' AND NOT EXISTS(SELECT 1 FROM agent_task_schedule_attempts a WHERE a.schedule_id=agent_task_schedule_occurrences.schedule_id AND a.occurrence_key=agent_task_schedule_occurrences.occurrence_key AND a.state IN ('pending','submitted')) AND occurrence_key NOT IN (SELECT occurrence_key FROM agent_task_schedule_occurrences WHERE schedule_id=?1 ORDER BY due_at DESC LIMIT 100)", [&occurrence.schedule.schedule_id]).map_err(map_sqlite_error)?;
            tx.commit().map_err(map_sqlite_error)
        }).await
    }
}

fn schedule_owner(owner: &labby_primitives::access::OwnerScope) -> (&'static str, &str) {
    use labby_primitives::access::OwnerScope;
    match owner {
        OwnerScope::Personal(id) => ("personal", id.as_str()),
        OwnerScope::Team(id) => ("team", id.as_str()),
        OwnerScope::Project(id) => ("project", id.as_str()),
        OwnerScope::Installation(id) => ("installation", id.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn fixture() -> (tempfile::TempDir, AccessStore) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        store.with_connection(|connection|{
            connection.execute_batch("INSERT INTO organizations VALUES('org','Org','active',0,0,0); INSERT INTO principals VALUES('principal','org','user','active','Principal',0,0); INSERT INTO agent_task_schedules(schedule_id,owner_kind,owner_id,creator_principal_id,name,task_template_json,schedule_spec_json,identity_ref_json,ceiling_json,armed,next_run_at,revision,created_at,updated_at) VALUES('daily','personal','principal','principal','Daily','{}','{}','{}','[]',1,100,1,0,0);").map_err(map_sqlite_error)
        }).await.unwrap();
        (directory, store)
    }
    async fn settle_fixture(
        store: &AccessStore,
        occurrence: &ScheduleOccurrence,
        state: &str,
        error: Option<&str>,
    ) {
        let id = occurrence.task_id.clone();
        let state = state.to_owned();
        let error = error.map(str::to_owned);
        store.with_connection(move|connection|{
            connection.execute("INSERT INTO agent_tasks(task_id,idempotency_key,owner_kind,owner_id,creator_principal_id,agent_id,agent_version,agent_revision_digest,input_digest,catalog_generation,authority_fingerprint,state,attempt,created_at,updated_at,error_code) VALUES(?1,?1,'personal','principal','principal','agent',1,'revision','input','catalog','authority',?2,1,0,0,?3)",params![id,state,error]).map_err(map_sqlite_error)?;Ok(())
        }).await.unwrap();
        store
            .task_schedule_finish(occurrence.clone(), None)
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn schedule_retry_backoff_restart_and_limit_create_distinct_immutable_tasks() {
        let (directory, store) = fixture().await;
        store.with_connection(|connection|{connection.execute(r#"UPDATE agent_task_schedules SET retry_policy_json='{"max_retries":2,"backoff_ms":300000}'"#,[]).map_err(map_sqlite_error)?;Ok(())}).await.unwrap();
        let row = store
            .task_schedule_get("daily".into())
            .await
            .unwrap()
            .unwrap();
        store
            .task_schedule_enqueue(row, "due:1:100".into(), 100, Some(900_000), 100)
            .await
            .unwrap();
        let first = store.task_schedule_claim(100).await.unwrap().unwrap();
        settle_fixture(&store, &first, "failed", Some("execution_failed")).await;
        assert_eq!(
            store.task_schedule_observe_settlements(200).await.unwrap(),
            1
        );
        assert_eq!(
            store.task_schedule_observe_settlements(201).await.unwrap(),
            0
        );
        assert_eq!(
            store
                .task_schedule_get("daily".into())
                .await
                .unwrap()
                .unwrap()
                .next_retry_at,
            Some(300_200)
        );
        assert!(store.task_schedule_claim(300_199).await.unwrap().is_none());
        drop(store);
        // This store-only fixture deliberately has no installation bootstrap.
        // Reopen through the same constructor to prove persisted retry state.
        let reopened = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let retry = reopened
            .task_schedule_claim(300_200)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retry.attempt_number, 1);
        assert_ne!(retry.task_id, first.task_id);
        assert_eq!(retry.original_task_id, first.task_id);
        settle_fixture(&reopened, &retry, "failed", Some("execution_failed")).await;
        reopened
            .task_schedule_observe_settlements(300_300)
            .await
            .unwrap();
        let final_retry = reopened
            .task_schedule_claim(600_300)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_retry.attempt_number, 2);
        assert_ne!(final_retry.task_id, retry.task_id);
        settle_fixture(&reopened, &final_retry, "failed", Some("execution_failed")).await;
        reopened
            .task_schedule_observe_settlements(600_400)
            .await
            .unwrap();
        assert!(
            reopened
                .task_schedule_claim(999_999)
                .await
                .unwrap()
                .is_none()
        );
        let count = reopened
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT count(*) FROM agent_task_schedule_attempts",
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                    .map_err(map_sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 3);
    }
    #[tokio::test]
    async fn schedule_retry_never_replays_cancellation_authority_or_permanent_failure() {
        for (state, error) in [
            ("cancelled", Some("cancelled")),
            ("expired", Some("execution_failed")),
            ("failed", Some("authority_revoked")),
            ("failed", Some("invalid_input")),
            ("failed", Some("executor_unavailable")),
            ("failed", Some("resource_limit")),
        ] {
            let (_directory, store) = fixture().await;
            store.with_connection(|connection|{connection.execute(r#"UPDATE agent_task_schedules SET retry_policy_json='{"max_retries":2,"backoff_ms":300000}'"#,[]).map_err(map_sqlite_error)?;Ok(())}).await.unwrap();
            let row = store
                .task_schedule_get("daily".into())
                .await
                .unwrap()
                .unwrap();
            store
                .task_schedule_enqueue(row, "manual:one".into(), 100, None, 100)
                .await
                .unwrap();
            let first = store.task_schedule_claim(100).await.unwrap().unwrap();
            settle_fixture(&store, &first, state, error).await;
            store.task_schedule_observe_settlements(200).await.unwrap();
            assert!(
                store.task_schedule_claim(900_000).await.unwrap().is_none(),
                "{state} {error:?}"
            );
        }
    }
    #[tokio::test]
    async fn schedule_retry_revision_change_fences_a_delayed_claim() {
        let (_directory, store) = fixture().await;
        store.with_connection(|connection| {
            connection.execute(r#"UPDATE agent_task_schedules SET retry_policy_json='{"max_retries":2,"backoff_ms":300000}'"#, []).map_err(map_sqlite_error)?;
            Ok(())
        }).await.unwrap();
        let row = store
            .task_schedule_get("daily".into())
            .await
            .unwrap()
            .unwrap();
        store
            .task_schedule_enqueue(row, "once".into(), 100, None, 100)
            .await
            .unwrap();
        let original = store.task_schedule_claim(100).await.unwrap().unwrap();
        settle_fixture(&store, &original, "failed", Some("execution_failed")).await;
        store.task_schedule_observe_settlements(200).await.unwrap();
        let retry = store.task_schedule_claim(300_200).await.unwrap().unwrap();
        assert_eq!(retry.attempt_number, 1);
        // Pause/edit change the same schedule revision checked atomically at admission.
        store
            .with_connection(|connection| {
                connection
                    .execute(
                        "UPDATE agent_task_schedules SET armed=0,revision=revision+1",
                        [],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            store
                .task_schedule_validate_claim(retry, 300_201)
                .await
                .is_err()
        );
        assert!(store.task_schedule_claim(400_000).await.unwrap().is_none());
    }

    #[test]
    fn schedule_retry_policy_is_bounded_and_disabled_by_default() {
        assert_eq!(RetryPolicy::default().max_retries, 0);
        assert!(
            RetryPolicy {
                max_retries: 2,
                backoff_ms: 300_000
            }
            .validate()
            .is_ok()
        );
        assert!(
            RetryPolicy {
                max_retries: 11,
                backoff_ms: 300_000
            }
            .validate()
            .is_err()
        );
        assert!(
            RetryPolicy {
                max_retries: 2,
                backoff_ms: 0
            }
            .validate()
            .is_err()
        );
    }
    #[tokio::test]
    async fn schedule_claim_restart_and_duplicate_enqueue_preserve_one_task() {
        let (_directory, store) = fixture().await;
        let row = store
            .task_schedule_get("daily".into())
            .await
            .unwrap()
            .unwrap();
        store
            .task_schedule_enqueue(row.clone(), "due:1:100".into(), 100, Some(200), 100)
            .await
            .unwrap();
        store
            .task_schedule_enqueue(row, "due:1:100".into(), 100, Some(200), 100)
            .await
            .unwrap();
        let first = store.task_schedule_claim(100).await.unwrap().unwrap();
        assert!(store.task_schedule_claim(101).await.unwrap().is_none());
        let recovered = store.task_schedule_claim(60_100).await.unwrap().unwrap();
        assert_eq!(first.task_id, recovered.task_id);
        assert_ne!(first.claim_token, recovered.claim_token);
        store.task_schedule_finish(first, None).await.unwrap();
        assert!(
            store
                .task_schedule_get("daily".into())
                .await
                .unwrap()
                .unwrap()
                .last_task_id
                .is_none()
        );
        store.task_schedule_finish(recovered, None).await.unwrap();
        assert!(store.task_schedule_claim(120_100).await.unwrap().is_none());
        assert_eq!(
            store
                .task_schedule_get("daily".into())
                .await
                .unwrap()
                .unwrap()
                .next_run_at,
            200
        );
    }
    #[tokio::test]
    async fn schedule_changed_revision_cannot_enqueue_old_work() {
        let (_directory, store) = fixture().await;
        let row = store
            .task_schedule_get("daily".into())
            .await
            .unwrap()
            .unwrap();
        store
            .with_connection(|connection| {
                connection
                    .execute("UPDATE agent_task_schedules SET revision=2,armed=0", [])
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();
        store
            .task_schedule_enqueue(row, "due:1:100".into(), 100, Some(200), 100)
            .await
            .unwrap();
        assert!(store.task_schedule_claim(100).await.unwrap().is_none());
    }
}
