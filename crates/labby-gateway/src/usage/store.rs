//! `UsageStore`: a small connection-pooled SQLite store for gateway call
//! telemetry. Mirrors `labby-auth`'s `SqliteStore` (`crates/labby-auth/src/sqlite.rs`)
//! but carries no secrets, so there is no at-rest encryption or restrictive
//! file-permission enforcement here.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};

use labby_runtime::error::ToolError;

use super::types::UpstreamCallRecord;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
const SQLITE_POOL_SIZE: usize = 4;
const SCHEMA_VERSION: i64 = 1;

#[derive(Clone)]
pub struct UsageStore {
    conns: Arc<Vec<Mutex<Connection>>>,
    next_conn: Arc<AtomicUsize>,
    path: Arc<PathBuf>,
}

impl std::fmt::Debug for UsageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl UsageStore {
    pub async fn open(path: PathBuf) -> Result<Self, ToolError> {
        let path_for_open = path.clone();
        let conns = tokio::task::spawn_blocking(move || {
            open_connections(path_for_open.as_path(), SQLITE_POOL_SIZE)
        })
        .await
        .map_err(|error| storage_error(format!("sqlite open task failed: {error}")))??;
        Ok(Self {
            conns: Arc::new(conns.into_iter().map(Mutex::new).collect()),
            next_conn: Arc::new(AtomicUsize::new(0)),
            path: Arc::new(path),
        })
    }

    pub async fn record_call(&self, record: UpstreamCallRecord) -> Result<(), ToolError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO upstream_calls (
                    ts_unix, upstream_name, tool_name, capability, operation,
                    subject_scoped, actor, outcome, error_kind, elapsed_ms, response_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    record.ts_unix,
                    record.upstream_name,
                    record.tool_name,
                    record.capability,
                    record.operation,
                    i64::from(record.subject_scoped),
                    record.actor,
                    record.outcome,
                    record.error_kind,
                    record.elapsed_ms,
                    record.response_bytes,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Delete rows older than `cutoff_unix`. Returns the number of deleted rows.
    pub async fn prune_older_than(&self, cutoff_unix: i64) -> Result<u64, ToolError> {
        self.with_conn(move |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM upstream_calls WHERE ts_unix < ?1",
                    params![cutoff_unix],
                )
                .map_err(sqlite_error)?;
            Ok(deleted as u64)
        })
        .await
    }

    pub(crate) async fn with_conn<T, F>(&self, op: F) -> Result<T, ToolError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, ToolError> + Send + 'static,
    {
        let conns = Arc::clone(&self.conns);
        let len = conns.len();
        let idx = self.next_conn.fetch_add(1, Ordering::Relaxed) % len;
        tokio::task::spawn_blocking(move || {
            let guard = conns[idx]
                .lock()
                .map_err(|_| storage_error("sqlite mutex poisoned".to_string()))?;
            op(&guard)
        })
        .await
        .map_err(|error| storage_error(format!("sqlite task failed: {error}")))?
    }
}

fn open_connections(path: &Path, count: usize) -> Result<Vec<Connection>, ToolError> {
    (0..count).map(|_| open_connection(path)).collect()
}

fn open_connection(path: &Path) -> Result<Connection, ToolError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            storage_error(format!(
                "create usage database directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }
    let conn = Connection::open(path).map_err(sqlite_error)?;
    conn.busy_timeout(std::time::Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(sqlite_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS upstream_calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_unix INTEGER NOT NULL,
            upstream_name TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            capability TEXT NOT NULL,
            operation TEXT NOT NULL,
            subject_scoped INTEGER NOT NULL,
            actor TEXT,
            outcome TEXT NOT NULL,
            error_kind TEXT,
            elapsed_ms INTEGER NOT NULL,
            response_bytes INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_ts ON upstream_calls(ts_unix);
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_upstream ON upstream_calls(upstream_name, ts_unix);",
    )
    .map_err(sqlite_error)?;
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
        .map_err(sqlite_error)?;
    Ok(conn)
}

impl UsageStore {
    pub async fn metrics(
        &self,
        query: super::query::UsageMetricsQuery,
    ) -> Result<super::query::UsageMetrics, ToolError> {
        self.with_conn(move |conn| {
            let (where_clause, bind) = usage_where_clause(&query.since_unix, &query.until_unix, &query.upstream);

            let (total_calls, error_calls, avg_elapsed_ms): (i64, i64, f64) = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*), SUM(CASE WHEN outcome != 'ok' THEN 1 ELSE 0 END), \
                         COALESCE(AVG(elapsed_ms), 0.0) FROM upstream_calls {where_clause}"
                    ),
                    rusqlite::params_from_iter(bind.iter()),
                    |row| Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?.unwrap_or(0), row.get(2)?)),
                )
                .map_err(sqlite_error)?;

            let mut top_tools_stmt = conn
                .prepare(&format!(
                    "SELECT upstream_name, tool_name, COUNT(*) as calls FROM upstream_calls {where_clause} \
                     GROUP BY upstream_name, tool_name ORDER BY calls DESC LIMIT {}",
                    super::query::TOP_N
                ))
                .map_err(sqlite_error)?;
            let top_tools = top_tools_stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UsageToolCount {
                        upstream: row.get(0)?,
                        tool: row.get(1)?,
                        calls: row.get(2)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            let mut top_actors_stmt = conn
                .prepare(&format!(
                    "SELECT COALESCE(actor, 'unattributed'), COUNT(*) as calls FROM upstream_calls {where_clause} \
                     GROUP BY COALESCE(actor, 'unattributed') ORDER BY calls DESC LIMIT {}",
                    super::query::TOP_N
                ))
                .map_err(sqlite_error)?;
            let top_actors = top_actors_stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UsageActorCount {
                        actor: row.get(0)?,
                        calls: row.get(1)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            Ok(super::query::UsageMetrics {
                total_calls,
                error_calls,
                avg_elapsed_ms,
                top_tools,
                top_actors,
            })
        })
        .await
    }

    /// Returns the requested page plus the total row count matching the
    /// filter (ignoring `limit`/`offset`), newest calls first.
    pub async fn list_calls(
        &self,
        query: super::query::UsageCallsQuery,
    ) -> Result<(Vec<super::query::UpstreamCallRecordView>, i64), ToolError> {
        self.with_conn(move |conn| {
            let (where_clause, mut bind) =
                usage_where_clause(&query.since_unix, &query.until_unix, &query.upstream);

            let total: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM upstream_calls {where_clause}"),
                    rusqlite::params_from_iter(bind.iter()),
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?;

            bind.push(rusqlite::types::Value::Integer(query.limit as i64));
            bind.push(rusqlite::types::Value::Integer(query.offset as i64));
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT ts_unix, upstream_name, tool_name, actor, outcome, elapsed_ms \
                     FROM upstream_calls {where_clause} \
                     ORDER BY ts_unix DESC, id DESC LIMIT ?{} OFFSET ?{}",
                    bind.len() - 1,
                    bind.len()
                ))
                .map_err(sqlite_error)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UpstreamCallRecordView {
                        ts_unix: row.get(0)?,
                        upstream: row.get(1)?,
                        tool: row.get(2)?,
                        actor: row.get(3)?,
                        outcome: row.get(4)?,
                        elapsed_ms: row.get(5)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            Ok((rows, total))
        })
        .await
    }
}

/// Build a `WHERE ...` clause (or empty string) plus its positional bind
/// values for the optional since/until/upstream filters shared by `metrics`
/// and `list_calls`.
fn usage_where_clause(
    since_unix: &Option<i64>,
    until_unix: &Option<i64>,
    upstream: &Option<String>,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut clauses = Vec::new();
    let mut bind = Vec::new();
    if let Some(since) = since_unix {
        clauses.push(format!("ts_unix >= ?{}", bind.len() + 1));
        bind.push(rusqlite::types::Value::Integer(*since));
    }
    if let Some(until) = until_unix {
        clauses.push(format!("ts_unix <= ?{}", bind.len() + 1));
        bind.push(rusqlite::types::Value::Integer(*until));
    }
    if let Some(upstream) = upstream {
        clauses.push(format!("upstream_name = ?{}", bind.len() + 1));
        bind.push(rusqlite::types::Value::Text(upstream.clone()));
    }
    if clauses.is_empty() {
        (String::new(), bind)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), bind)
    }
}

pub(crate) fn sqlite_error(error: rusqlite::Error) -> ToolError {
    storage_error(format!("sqlite error: {error}"))
}

fn storage_error(message: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "usage_store_error".to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::UsageStore;
    use crate::usage::types::UpstreamCallRecord;

    fn sample_record(ts_unix: i64) -> UpstreamCallRecord {
        UpstreamCallRecord {
            ts_unix,
            upstream_name: "github".to_string(),
            tool_name: "search_repos".to_string(),
            capability: "tools".to_string(),
            operation: "tool.call".to_string(),
            subject_scoped: false,
            actor: None,
            outcome: "ok".to_string(),
            error_kind: None,
            elapsed_ms: 42,
            response_bytes: Some(128),
        }
    }

    #[tokio::test]
    async fn record_call_persists_and_is_queryable_by_count() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        store.record_call(sample_record(1_000)).await.unwrap();
        store.record_call(sample_record(1_001)).await.unwrap();

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM upstream_calls", [], |row| row.get(0))
                    .map_err(super::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn prune_older_than_deletes_only_stale_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        store.record_call(sample_record(100)).await.unwrap();
        store.record_call(sample_record(200)).await.unwrap();

        let deleted = store.prune_older_than(150).await.unwrap();
        assert_eq!(deleted, 1);

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM upstream_calls", [], |row| row.get(0))
                    .map_err(super::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn metrics_aggregates_totals_and_top_tools() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        let mut ok = sample_record(1_000);
        ok.tool_name = "search_repos".to_string();
        store.record_call(ok.clone()).await.unwrap();
        store.record_call(ok).await.unwrap();

        let mut failed = sample_record(1_001);
        failed.outcome = "timeout".to_string();
        failed.tool_name = "search_repos".to_string();
        store.record_call(failed).await.unwrap();

        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
            })
            .await
            .unwrap();

        assert_eq!(metrics.total_calls, 3);
        assert_eq!(metrics.error_calls, 1);
        assert_eq!(metrics.top_tools.len(), 1);
        assert_eq!(metrics.top_tools[0].tool, "search_repos");
        assert_eq!(metrics.top_tools[0].calls, 3);
    }

    #[tokio::test]
    async fn list_calls_respects_limit_and_reports_total_matching() {
        use super::super::query::UsageCallsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        for ts in 0..5 {
            store.record_call(sample_record(ts)).await.unwrap();
        }

        let (page, total) = store
            .list_calls(UsageCallsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
                limit: 2,
                offset: 0,
            })
            .await
            .unwrap();

        assert_eq!(page.len(), 2);
        assert_eq!(total, 5);
        // Newest first.
        assert_eq!(page[0].ts_unix, 4);
    }
}
