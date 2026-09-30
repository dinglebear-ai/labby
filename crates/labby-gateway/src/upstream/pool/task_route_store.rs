//! Durable metadata store for public MCP task routes.
//!
//! This store intentionally owns no live relay/peer state. A persisted route is
//! sufficient to re-authorize and identify the upstream/native task after a
//! process restart; connection reacquisition is a separate pool concern.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};

use super::task_route::TaskRouteAuthorization;

const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TaskRouteRecord {
    pub(super) public_task_id: String,
    pub(super) native_task_id: String,
    pub(super) upstream_name: String,
    pub(super) caller_subject: Option<String>,
    pub(super) oauth_subject: Option<String>,
    pub(super) authorization: TaskRouteAuthorization,
    pub(super) config_fingerprint: String,
    pub(super) created_at_unix_ms: i64,
    pub(super) updated_at_unix_ms: i64,
    pub(super) ttl_ms: Option<u64>,
    pub(super) poll_interval_ms: Option<u64>,
}

#[derive(Clone)]
pub struct TaskRouteStore {
    connection: Arc<Mutex<Connection>>,
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
        let connection = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "create task route database directory {}: {error}",
                        parent.display()
                    )
                })?;
            }
            prepare_task_route_database_file(&path)?;
            let connection = Connection::open(&path).map_err(sqlite_error)?;
            initialize_schema(&connection)?;
            ensure_restrictive_permissions(&path)?;
            for suffix in ["-wal", "-shm"] {
                let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
                if sidecar.exists() {
                    ensure_restrictive_permissions(&sidecar)?;
                }
            }
            Ok::<_, String>(connection)
        })
        .await
        .map_err(|error| format!("task route sqlite open task failed: {error}"))??;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            #[cfg(test)]
            fail_writes: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    #[cfg(test)]
    pub(super) async fn open_in_memory() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(sqlite_error)?;
        initialize_schema(&connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            fail_writes: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub(super) async fn insert(&self, route: TaskRouteRecord) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("injected task route persistence failure".to_string());
        }

        self.with_connection(move |connection| {
            let allowed_upstreams_json = route
                .authorization
                .allowed_upstreams
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| format!("serialize task route authorization: {error}"))?;
            let ttl_ms = route.ttl_ms.map(u64_to_i64).transpose()?;
            let poll_interval_ms = route.poll_interval_ms.map(u64_to_i64).transpose()?;
            connection
                .execute(
                    "INSERT INTO task_routes (
                        public_task_id, native_task_id, upstream_name, caller_subject,
                        oauth_subject, route_key, allowed_upstreams_json, config_fingerprint,
                        created_at_unix_ms, updated_at_unix_ms, ttl_ms, poll_interval_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        route.public_task_id,
                        route.native_task_id,
                        route.upstream_name,
                        route.caller_subject,
                        route.oauth_subject,
                        route.authorization.route_key,
                        allowed_upstreams_json,
                        route.config_fingerprint,
                        route.created_at_unix_ms,
                        route.updated_at_unix_ms,
                        ttl_ms,
                        poll_interval_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub(super) async fn get(
        &self,
        public_task_id: &str,
    ) -> Result<Option<TaskRouteRecord>, String> {
        let public_task_id = public_task_id.to_string();
        self.with_connection(move |connection| {
            connection
                .query_row(
                    "SELECT public_task_id, native_task_id, upstream_name, caller_subject,
                            oauth_subject, route_key, allowed_upstreams_json, config_fingerprint,
                            created_at_unix_ms, updated_at_unix_ms, ttl_ms, poll_interval_ms
                     FROM task_routes WHERE public_task_id = ?1",
                    [public_task_id],
                    |row| {
                        let allowed_json: Option<String> = row.get(6)?;
                        let allowed_upstreams = allowed_json
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
                        let ttl_ms: Option<i64> = row.get(10)?;
                        let poll_interval_ms: Option<i64> = row.get(11)?;
                        Ok(TaskRouteRecord {
                            public_task_id: row.get(0)?,
                            native_task_id: row.get(1)?,
                            upstream_name: row.get(2)?,
                            caller_subject: row.get(3)?,
                            oauth_subject: row.get(4)?,
                            authorization: TaskRouteAuthorization::new(
                                row.get::<_, String>(5)?,
                                allowed_upstreams,
                            ),
                            config_fingerprint: row.get(7)?,
                            created_at_unix_ms: row.get(8)?,
                            updated_at_unix_ms: row.get(9)?,
                            ttl_ms: ttl_ms
                                .map(i64_to_u64)
                                .transpose()
                                .map_err(to_from_sql_error)?,
                            poll_interval_ms: poll_interval_ms
                                .map(i64_to_u64)
                                .transpose()
                                .map_err(to_from_sql_error)?,
                        })
                    },
                )
                .optional()
                .map_err(sqlite_error)
        })
        .await
    }

    pub(super) async fn update_hints(
        &self,
        public_task_id: &str,
        updated_at_unix_ms: i64,
        ttl_ms: Option<u64>,
        poll_interval_ms: Option<u64>,
    ) -> Result<(), String> {
        let public_task_id = public_task_id.to_string();
        let ttl_ms = ttl_ms.map(u64_to_i64).transpose()?;
        let poll_interval_ms = poll_interval_ms.map(u64_to_i64).transpose()?;
        self.with_connection(move |connection| {
            connection
                .execute(
                    "UPDATE task_routes
                     SET updated_at_unix_ms = ?2, ttl_ms = ?3, poll_interval_ms = ?4
                     WHERE public_task_id = ?1",
                    params![public_task_id, updated_at_unix_ms, ttl_ms, poll_interval_ms],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    async fn with_connection<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
    {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let guard = connection
                .lock()
                .map_err(|_| "task route sqlite mutex poisoned".to_string())?;
            operation(&guard)
        })
        .await
        .map_err(|error| format!("task route sqlite task failed: {error}"))?
    }

    #[cfg(test)]
    pub(super) fn set_fail_writes_for_tests(&self, fail: bool) {
        self.fail_writes
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(unix)]
fn prepare_task_route_database_file(path: &Path) -> Result<(), String> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|error| format!("open task route database {}: {error}", path.display()))?;
    drop(fd);
    ensure_restrictive_permissions(path)
}

#[cfg(not(unix))]
fn prepare_task_route_database_file(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn ensure_restrictive_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("chmod 0600 {}: {error}", path.display()))
}

#[cfg(windows)]
fn ensure_restrictive_permissions(path: &Path) -> Result<(), String> {
    labby_auth::util::harden_secret_file(path)
        .map_err(|error| format!("harden ACL {}: {error}", path.display()))
}

#[cfg(not(any(unix, windows)))]
fn ensure_restrictive_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn initialize_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS task_route_meta (
                 schema_version INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS task_routes (
                 public_task_id TEXT PRIMARY KEY,
                 native_task_id TEXT NOT NULL,
                 upstream_name TEXT NOT NULL,
                 caller_subject TEXT,
                 oauth_subject TEXT,
                 route_key TEXT NOT NULL,
                 allowed_upstreams_json TEXT,
                 config_fingerprint TEXT NOT NULL,
                 created_at_unix_ms INTEGER NOT NULL,
                 updated_at_unix_ms INTEGER NOT NULL,
                 ttl_ms INTEGER,
                 poll_interval_ms INTEGER
             );
             CREATE INDEX IF NOT EXISTS task_routes_upstream_oauth
                 ON task_routes (upstream_name, oauth_subject);",
        )
        .map_err(sqlite_error)?;

    let version = connection
        .query_row(
            "SELECT schema_version FROM task_route_meta LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    match version {
        Some(current) if current == SCHEMA_VERSION => Ok(()),
        Some(current) => Err(format!(
            "unsupported task route schema version {current}; expected {SCHEMA_VERSION}"
        )),
        None => {
            connection
                .execute(
                    "INSERT INTO task_route_meta (schema_version) VALUES (?1)",
                    [SCHEMA_VERSION],
                )
                .map_err(sqlite_error)?;
            Ok(())
        }
    }
}

fn sqlite_error(error: rusqlite::Error) -> String {
    format!("task route sqlite error: {error}")
}

fn u64_to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value)
        .map_err(|_| "task route millisecond value exceeds SQLite range".to_string())
}

fn i64_to_u64(value: i64) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| "task route millisecond value is negative".to_string())
}

fn to_from_sql_error(error: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Integer,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{TaskRouteRecord, TaskRouteStore};
    use crate::upstream::pool::TaskRouteAuthorization;

    fn record() -> TaskRouteRecord {
        TaskRouteRecord {
            public_task_id: "labby-task-0123456789abcdef0123456789abcdef".to_string(),
            native_task_id: "native-42".to_string(),
            upstream_name: "example".to_string(),
            caller_subject: Some("alice".to_string()),
            oauth_subject: Some("oauth-alice".to_string()),
            authorization: TaskRouteAuthorization::new(
                "protected:test",
                Some(BTreeSet::from(["example".to_string()])),
            ),
            config_fingerprint: "cfg-123".to_string(),
            created_at_unix_ms: 100,
            updated_at_unix_ms: 101,
            ttl_ms: Some(60_000),
            poll_interval_ms: Some(250),
        }
    }

    #[tokio::test]
    async fn route_survives_store_reopen() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("task-routes.db");
        let expected = record();

        {
            let store = TaskRouteStore::open(path.clone())
                .await
                .expect("store opens");
            store
                .insert(expected.clone())
                .await
                .expect("route persists");
        }

        let reopened = TaskRouteStore::open(path.clone())
            .await
            .expect("store reopens");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("task route database metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            reopened
                .get(&expected.public_task_id)
                .await
                .expect("route lookup succeeds"),
            Some(expected)
        );
    }
}
