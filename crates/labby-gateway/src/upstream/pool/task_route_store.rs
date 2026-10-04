//! Durable metadata-only MCP task routing. No connection or credential storage.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use tokio::sync::{Mutex, Semaphore};

use super::task_route::TaskRouteAuthorization;
pub(super) use super::task_route_record::TaskRouteRecord;
use super::task_route_record::valid_public_task_id;
use super::task_route_schema::{self, sqlite_error};

const MAX_PENDING_OPERATIONS: usize = 32;
const MAX_ROUTES: i64 = 4096;
const MAX_OWNER_ROUTES: i64 = 256;

#[derive(Clone)]
pub struct TaskRouteStore {
    connection: Arc<Mutex<Connection>>,
    admission: Arc<Semaphore>,
    revocation_failed: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    fail_writes: Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for TaskRouteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRouteStore").finish_non_exhaustive()
    }
}

impl TaskRouteStore {
    pub async fn open(path: PathBuf) -> Result<Self, String> {
        let connection = tokio::task::spawn_blocking(move || task_route_schema::open(&path))
            .await
            .map_err(|_| "task route SQLite open worker failed".to_string())??;
        Ok(Self::from_connection(connection))
    }

    fn from_connection(connection: Connection) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
            admission: Arc::new(Semaphore::new(MAX_PENDING_OPERATIONS)),
            revocation_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            fail_writes: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    pub(super) async fn open_in_memory() -> Result<Self, String> {
        let connection = tokio::task::spawn_blocking(|| {
            let mut connection = Connection::open_in_memory().map_err(sqlite_error)?;
            task_route_schema::initialize(&mut connection)?;
            Ok::<_, String>(connection)
        })
        .await
        .map_err(|_| "task route SQLite test worker failed".to_string())??;
        Ok(Self::from_connection(connection))
    }

    /// Reject admission instead of evicting an acknowledged, unexpired task.
    pub(super) async fn insert(&self, route: TaskRouteRecord) -> Result<(), String> {
        route.validate()?;
        #[cfg(test)]
        if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("injected task route persistence failure".to_string());
        }
        let allowed = route
            .authorization
            .allowed_upstreams
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| "cannot encode task authorization".to_string())?;
        if allowed.as_ref().is_some_and(|s| s.len() > 65536) {
            return Err("task authorization metadata exceeds storage bound".to_string());
        }
        self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            // Bounded cleanup of expired routes only. Unlimited TTL is never
            // converted into an idle timeout or discarded to make room.
            tx.execute("DELETE FROM task_routes WHERE public_task_id IN (
                SELECT public_task_id FROM task_routes WHERE ttl_ms IS NOT NULL
                AND created_at_unix_ms + ttl_ms <= ?1 ORDER BY created_at_unix_ms + ttl_ms LIMIT 256)",
                [jiff::Timestamp::now().as_millisecond()]).map_err(sqlite_error)?;
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM task_routes", [], |row| row.get(0)).map_err(sqlite_error)?;
            let owner_count: i64 = tx.query_row("SELECT COUNT(*) FROM task_routes WHERE caller_subject IS ?1",
                [route.caller_subject.as_deref()], |row| row.get(0)).map_err(sqlite_error)?;
            if count >= MAX_ROUTES || owner_count >= MAX_OWNER_ROUTES {
                return Err("task route capacity exhausted".to_string());
            }
            tx.execute("INSERT INTO task_routes (public_task_id, native_task_id, upstream_name,
                caller_subject, oauth_subject, route_key, allowed_upstreams_json, config_fingerprint,
                created_at_unix_ms, updated_at_unix_ms, ttl_ms, poll_interval_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![route.public_task_id, route.native_task_id, route.upstream_name,
                    route.caller_subject, route.oauth_subject, route.authorization.route_key, allowed,
                    route.config_fingerprint, route.created_at_unix_ms, route.updated_at_unix_ms,
                    route.ttl_ms.map(|value| value as i64), route.poll_interval_ms.map(|value| value as i64)],
            ).map_err(sqlite_error)?;
            tx.commit().map_err(sqlite_error)
        }).await
    }

    #[cfg(test)]
    pub(super) async fn get(&self, id: &str) -> Result<Option<TaskRouteRecord>, String> {
        self.get_filtered(id, None).await
    }

    pub(super) async fn get_for_caller(
        &self,
        id: &str,
        caller: Option<&str>,
        authorization: &TaskRouteAuthorization,
    ) -> Result<Option<TaskRouteRecord>, String> {
        self.get_filtered(id, Some((caller.map(str::to_owned), authorization.clone())))
            .await
    }

    async fn get_filtered(
        &self,
        public_task_id: &str,
        scope: Option<(Option<String>, TaskRouteAuthorization)>,
    ) -> Result<Option<TaskRouteRecord>, String> {
        if !valid_public_task_id(public_task_id) {
            return Ok(None);
        }
        let id = public_task_id.to_string();
        let scoped = scope.is_some();
        let (caller, authorization) =
            scope.unwrap_or_else(|| (None, TaskRouteAuthorization::root()));
        let allowed = authorization
            .allowed_upstreams
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| "cannot encode task authorization".to_string())?;
        self.with_connection(move |connection| {
            // Filter ownership before decoding any row content. Even a damaged
            // row must not reveal its existence to a different principal/route.
            let record = connection.query_row("SELECT public_task_id, native_task_id, upstream_name,
                caller_subject, oauth_subject, route_key, allowed_upstreams_json, config_fingerprint,
                created_at_unix_ms, updated_at_unix_ms, ttl_ms, poll_interval_ms
                FROM task_routes WHERE public_task_id = ?1 AND (?2 = 0 OR
                    (caller_subject IS ?3 AND route_key = ?4 AND allowed_upstreams_json IS ?5))",
                params![id, scoped, caller, authorization.route_key, allowed], read_record)
                .optional().map_err(sqlite_error)?;
            if let Some(record) = &record { record.validate()?; }
            Ok(record)
        }).await
    }

    /// Persist an observation before returning its potentially changed retention
    /// promise. Older polls cannot overwrite a newer observation.
    pub(super) async fn update_hints(&self, record: TaskRouteRecord) -> Result<(), String> {
        record.validate()?;
        self.with_connection(move |connection| {
            let changed = connection.execute(
                "UPDATE task_routes SET updated_at_unix_ms = ?2, ttl_ms = ?3, poll_interval_ms = ?4
                 WHERE public_task_id = ?1 AND native_task_id = ?5 AND created_at_unix_ms = ?6
                   AND updated_at_unix_ms <= ?2 AND config_fingerprint = ?7",
                params![record.public_task_id, record.updated_at_unix_ms,
                    record.ttl_ms.map(|value| value as i64), record.poll_interval_ms.map(|value| value as i64),
                    record.native_task_id, record.created_at_unix_ms, record.config_fingerprint],
            ).map_err(sqlite_error)?;
            if changed != 1 { return Err("task route observation is stale or unavailable".to_string()); }
            Ok(())
        }).await
    }

    pub(super) async fn remove(&self, id: &str) -> Result<(), String> {
        let id = id.to_string();
        self.with_connection(move |connection| {
            connection
                .execute("DELETE FROM task_routes WHERE public_task_id = ?1", [id])
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Revocation is a durable delete: public UUIDs are never reused, so stale
    /// observations can neither recreate nor update a removed route.
    pub(super) async fn remove_oauth_subject(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<usize, String> {
        let upstream = upstream.to_owned();
        let subject = subject.to_owned();
        let result = self
            .with_connection(move |connection| {
                connection
                    .execute(
                        "DELETE FROM task_routes WHERE upstream_name = ?1 AND oauth_subject = ?2",
                        params![upstream, subject],
                    )
                    .map_err(sqlite_error)
            })
            .await;
        self.finish_revocation(result)
    }

    pub(super) async fn remove_upstreams(&self, upstreams: Vec<String>) -> Result<usize, String> {
        let result = self
            .with_connection(move |connection| {
                let tx = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(sqlite_error)?;
                let mut removed = 0;
                for upstream in upstreams {
                    removed += tx
                        .execute(
                            "DELETE FROM task_routes WHERE upstream_name = ?1",
                            [upstream],
                        )
                        .map_err(sqlite_error)?;
                }
                tx.commit().map_err(sqlite_error)?;
                Ok(removed)
            })
            .await;
        self.finish_revocation(result)
    }

    pub(super) async fn remove_oauth_upstreams(
        &self,
        upstreams: Vec<String>,
    ) -> Result<usize, String> {
        let result = self.with_connection(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            let mut removed = 0;
            for upstream in upstreams {
                removed += tx.execute("DELETE FROM task_routes WHERE upstream_name = ?1 AND oauth_subject IS NOT NULL", [upstream]).map_err(sqlite_error)?;
            }
            tx.commit().map_err(sqlite_error)?;
            Ok(removed)
        }).await;
        self.finish_revocation(result)
    }

    #[cfg(test)]
    pub(super) async fn remove_all_oauth(&self) -> Result<usize, String> {
        let result = self
            .with_connection(|connection| {
                connection
                    .execute(
                        "DELETE FROM task_routes WHERE oauth_subject IS NOT NULL",
                        [],
                    )
                    .map_err(sqlite_error)
            })
            .await;
        self.finish_revocation(result)
    }

    fn finish_revocation(&self, result: Result<usize, String>) -> Result<usize, String> {
        if result.is_err() {
            self.revocation_failed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        result
    }

    async fn with_connection<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
    {
        if self
            .revocation_failed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(
                "task routing quarantined after revocation persistence failure".to_string(),
            );
        }
        let permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| "task route store busy".to_string())?;
        // Wait asynchronously before spawning, not on a blocking-pool thread.
        // Keep the permit and guard in the worker even if its caller is cancelled.
        let mut connection = self.connection.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(&mut connection)
        })
        .await
        .map_err(|_| "task route SQLite worker failed".to_string())?
    }

    #[cfg(test)]
    pub(super) fn set_fail_writes_for_tests(&self, fail: bool) {
        self.fail_writes
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

fn read_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRouteRecord> {
    let allowed: Option<String> = row.get(6)?;
    let allowed = allowed
        .as_deref()
        .map(serde_json::from_str::<BTreeSet<String>>)
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    let ttl: Option<i64> = row.get(10)?;
    let poll: Option<i64> = row.get(11)?;
    let positive = |value| {
        u64::try_from(value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })
    };
    Ok(TaskRouteRecord {
        public_task_id: row.get(0)?,
        native_task_id: row.get(1)?,
        upstream_name: row.get(2)?,
        caller_subject: row.get(3)?,
        oauth_subject: row.get(4)?,
        authorization: TaskRouteAuthorization::new(row.get::<_, String>(5)?, allowed),
        config_fingerprint: row.get(7)?,
        created_at_unix_ms: row.get(8)?,
        updated_at_unix_ms: row.get(9)?,
        ttl_ms: ttl.map(positive).transpose()?,
        poll_interval_ms: poll.map(positive).transpose()?,
    })
}

#[cfg(test)]
#[path = "task_route_store_tests.rs"]
mod tests;
