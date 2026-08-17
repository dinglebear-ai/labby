//! `StepJournalStore`: a small connection-pooled append-only SQLite store for
//! `codemode.step` boundaries. Mirrors `crate::usage::store::UsageStore`'s
//! pool/pragma/permission scaffolding (owner-only `0600`, WAL,
//! `synchronous=NORMAL`). It exposes only append and internal read operations;
//! it never gates, resumes, replays, or pauses a run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use labby_runtime::agent_error::redact_secret_like_segments;
use labby_runtime::error::ToolError;
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use serde_json::Value;

use super::StepJournalRow;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
// Bounds read/write interleaving under WAL mode — SQLite still serializes
// actual writers regardless of connection count, so this does not buy write
// parallelism, only concurrent readers alongside a writer.
const SQLITE_POOL_SIZE: usize = 4;
const SCHEMA_VERSION: i64 = 1;
/// Max rows deleted per `DELETE` statement in `prune_older_than`'s batching
/// loop, so a large prune backlog doesn't hold the writer lock in one shot.
const PRUNE_BATCH_SIZE: i64 = 5_000;

const CREATE_TABLE: &str = "\
CREATE TABLE IF NOT EXISTS step_journal (
    execution_id TEXT NOT NULL,
    step_ordinal INTEGER NOT NULL,
    seq_base INTEGER NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    ok INTEGER NOT NULL,
    elapsed_ms INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    actor_key TEXT,
    route_scope TEXT NOT NULL,
    capability_filter_fingerprint TEXT,
    replayed_from TEXT,
    PRIMARY KEY (execution_id, step_ordinal)
);";

#[derive(Clone)]
pub struct StepJournalStore {
    conns: Arc<Vec<Mutex<Connection>>>,
    next_conn: Arc<AtomicUsize>,
    path: Arc<PathBuf>,
}

impl std::fmt::Debug for StepJournalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepJournalStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl StepJournalStore {
    pub async fn open(path: PathBuf) -> Result<Self, ToolError> {
        let path_for_open = path.clone();
        let conns = tokio::task::spawn_blocking(move || {
            open_connections(path_for_open.as_path(), SQLITE_POOL_SIZE)
        })
        .await
        .map_err(|error| storage_error(format!("journal open task failed: {error}")))??;
        Ok(Self {
            conns: Arc::new(conns.into_iter().map(Mutex::new).collect()),
            next_conn: Arc::new(AtomicUsize::new(0)),
            path: Arc::new(path),
        })
    }

    /// Persist a batch of journal rows in ONE transaction, one prepared
    /// statement reused per row. `INSERT OR IGNORE` makes the flush idempotent
    /// on the `(execution_id, step_ordinal)` key. All values are bound via
    /// `params![]` — never `format!`-ed into SQL.
    pub async fn flush(&self, rows: Vec<StepJournalRow>) -> Result<(), ToolError> {
        if rows.is_empty() {
            return Ok(());
        }
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(sqlite_error)?;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT OR IGNORE INTO step_journal \
                         (execution_id, step_ordinal, seq_base, name, value, ok, elapsed_ms, recorded_at, \
                          actor_key, route_scope, capability_filter_fingerprint, replayed_from) \
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    )
                    .map_err(sqlite_error)?;
                for r in &rows {
                    stmt.execute(params![
                        r.execution_id,
                        r.step_ordinal as i64,
                        r.seq_base as i64,
                        r.name,
                        r.value,
                        r.ok as i64,
                        r.elapsed_ms as i64,
                        r.recorded_at,
                        r.actor_key,
                        r.route_scope,
                        r.capability_filter_fingerprint,
                        r.replayed_from,
                    ])
                    .map_err(sqlite_error)?;
                }
            }
            tx.commit().map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Load every row for one execution, ascending by `step_ordinal`.
    pub async fn load(&self, execution_id: &str) -> Result<Vec<StepJournalRow>, ToolError> {
        let execution_id = execution_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT execution_id, step_ordinal, seq_base, name, value, ok, elapsed_ms, \
                     recorded_at, actor_key, route_scope, capability_filter_fingerprint, replayed_from \
                     FROM step_journal WHERE execution_id = ?1 ORDER BY step_ordinal ASC",
                )
                .map_err(sqlite_error)?;
            let rows = stmt
                .query_map(params![execution_id], |row| {
                    Ok(StepJournalRow {
                        execution_id: row.get(0)?,
                        step_ordinal: row.get::<_, i64>(1)? as u64,
                        seq_base: row.get::<_, i64>(2)? as u64,
                        name: row.get(3)?,
                        value: row.get(4)?,
                        ok: row.get::<_, i64>(5)? != 0,
                        elapsed_ms: row.get::<_, i64>(6)? as u128,
                        recorded_at: row.get(7)?,
                        actor_key: row.get(8)?,
                        route_scope: row.get(9)?,
                        capability_filter_fingerprint: row.get(10)?,
                        replayed_from: row.get(11)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;
            Ok(rows)
        })
        .await
    }

    /// Delete rows recorded before `cutoff_unix`, in bounded batches so a large
    /// backlog doesn't hold SQLite's writer lock in one shot. Returns the total
    /// number of deleted rows.
    pub async fn prune_older_than(&self, cutoff_unix: i64) -> Result<usize, ToolError> {
        let mut total_deleted: usize = 0;
        loop {
            let deleted = self
                .with_conn(move |conn| {
                    let deleted = conn
                        .execute(
                            "DELETE FROM step_journal WHERE rowid IN (
                                SELECT rowid FROM step_journal WHERE recorded_at < ?1 LIMIT ?2
                             )",
                            params![cutoff_unix, PRUNE_BATCH_SIZE],
                        )
                        .map_err(sqlite_error)?;
                    Ok(deleted)
                })
                .await?;
            total_deleted += deleted;
            if deleted == 0 {
                break;
            }
        }
        Ok(total_deleted)
    }

    /// Spawn a background loop that periodically prunes journal rows older than
    /// `retention_secs`. Ticks every `interval`; missed ticks are skipped (not
    /// backlogged) so a slow prune never causes a burst of catch-up runs. A
    /// sustained failure (disk full, permissions) escalates to `error` after
    /// three consecutive failures. Mirrors `UsageStore::spawn_prune_loop`.
    pub fn spawn_prune_loop(self: Arc<Self>, retention_secs: i64, interval: std::time::Duration) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut consecutive_failures: u32 = 0;
            loop {
                ticker.tick().await;
                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let cutoff = now_unix.saturating_sub(retention_secs);
                match self.prune_older_than(cutoff).await {
                    Ok(deleted) => {
                        consecutive_failures = 0;
                        if deleted > 0 {
                            tracing::info!(deleted, "pruned stale Code Mode step-journal rows");
                        }
                    }
                    Err(error) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            tracing::error!(
                                error = %error,
                                consecutive_failures,
                                "Code Mode step-journal prune failed repeatedly"
                            );
                        } else {
                            tracing::warn!(
                                error = %error,
                                consecutive_failures,
                                "Code Mode step-journal prune failed"
                            );
                        }
                    }
                }
            }
        });
    }

    async fn with_conn<T, F>(&self, op: F) -> Result<T, ToolError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, ToolError> + Send + 'static,
    {
        let conns = Arc::clone(&self.conns);
        let len = conns.len();
        let idx = self.next_conn.fetch_add(1, Ordering::Relaxed) % len;
        tokio::task::spawn_blocking(move || {
            let mut guard = conns[idx]
                .lock()
                .map_err(|_| storage_error("journal conn mutex poisoned".to_string()))?;
            op(&mut guard)
        })
        .await
        .map_err(|error| storage_error(format!("journal task failed: {error}")))?
    }
}

fn open_connections(path: &Path, count: usize) -> Result<Vec<Connection>, ToolError> {
    if path.exists() {
        validate_existing_database(path)?;
    } else {
        initialize_database(path)?;
    }
    (0..count)
        .map(|_| open_validated_connection(path))
        .collect()
}

#[derive(Clone, Copy)]
enum SchemaFailure {
    UnsupportedVersion,
    SchemaMismatch,
    Corrupt,
}

impl SchemaFailure {
    const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "unsupported_version",
            Self::SchemaMismatch => "schema_mismatch",
            Self::Corrupt => "corrupt",
        }
    }
}

fn schema_error(path: &Path, reason: SchemaFailure, detail: impl std::fmt::Display) -> ToolError {
    storage_error(format!(
        "journal schema validation failed [{}] for `{}`: {detail}",
        reason.as_str(),
        path.display()
    ))
}

fn validate_existing_database(path: &Path) -> Result<(), ToolError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| schema_error(path, SchemaFailure::Corrupt, error))?;
    let version = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(|error| schema_error(path, classify_validation_error(&error), error))?;
    if version != SCHEMA_VERSION {
        return Err(schema_error(
            path,
            SchemaFailure::UnsupportedVersion,
            format_args!("expected version {SCHEMA_VERSION}, found {version}"),
        ));
    }

    let mut stmt = conn
        .prepare("PRAGMA table_info(step_journal)")
        .map_err(|error| schema_error(path, classify_validation_error(&error), error))?;
    let columns = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| schema_error(path, classify_validation_error(&error), error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema_error(path, classify_validation_error(&error), error))?;
    let expected = [
        ("execution_id", "TEXT", 1, 1),
        ("step_ordinal", "INTEGER", 1, 2),
        ("seq_base", "INTEGER", 1, 0),
        ("name", "TEXT", 1, 0),
        ("value", "TEXT", 1, 0),
        ("ok", "INTEGER", 1, 0),
        ("elapsed_ms", "INTEGER", 1, 0),
        ("recorded_at", "INTEGER", 1, 0),
        ("actor_key", "TEXT", 0, 0),
        ("route_scope", "TEXT", 1, 0),
        ("capability_filter_fingerprint", "TEXT", 0, 0),
        ("replayed_from", "TEXT", 0, 0),
    ];
    let compatible = columns.len() == expected.len()
        && columns.iter().zip(expected).all(|(actual, expected)| {
            actual.0 == expected.0
                && actual.1.eq_ignore_ascii_case(expected.1)
                && actual.2 == expected.2
                && actual.3 == expected.3
        });
    if !compatible {
        return Err(schema_error(
            path,
            SchemaFailure::SchemaMismatch,
            "step_journal columns do not match schema v1",
        ));
    }
    Ok(())
}

fn classify_validation_error(error: &rusqlite::Error) -> SchemaFailure {
    match error {
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            SchemaFailure::Corrupt
        }
        _ => SchemaFailure::SchemaMismatch,
    }
}

#[cfg(unix)]
fn ensure_restrictive_permissions(path: &Path) -> Result<(), ToolError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| storage_error(format!("chmod 0600 `{}`: {error}", path.display())))
}

#[cfg(windows)]
fn ensure_restrictive_permissions(path: &Path) -> Result<(), ToolError> {
    labby_auth::util::harden_secret_file(path)
        .map_err(|error| storage_error(format!("harden ACL `{}`: {error}", path.display())))
}

fn initialize_database(path: &Path) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            storage_error(format!(
                "create journal database directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }
    let conn = Connection::open(path).map_err(sqlite_error)?;
    ensure_restrictive_permissions(path)?;
    configure_connection(&conn)?;
    conn.execute_batch(CREATE_TABLE).map_err(sqlite_error)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(sqlite_error)?;
    restrict_sidecars(path)?;
    Ok(())
}

fn open_validated_connection(path: &Path) -> Result<Connection, ToolError> {
    let conn = Connection::open(path).map_err(sqlite_error)?;
    ensure_restrictive_permissions(path)?;
    configure_connection(&conn)?;
    restrict_sidecars(path)?;
    Ok(conn)
}

fn configure_connection(conn: &Connection) -> Result<(), ToolError> {
    conn.busy_timeout(std::time::Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(sqlite_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    // Safe alongside WAL: reduces per-insert fsync cost. The journal is a
    // best-effort forensic record (fail-open), not durability-critical — losing
    // the last few writes on a hard crash is an acceptable tradeoff.
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(sqlite_error)?;
    Ok(())
}

fn restrict_sidecars(path: &Path) -> Result<(), ToolError> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.exists() {
            ensure_restrictive_permissions(&sidecar)?;
        }
    }
    Ok(())
}

/// An `io::Write` that errors as soon as the accumulated byte count would exceed
/// `cap`, so `serde_json` aborts serialization early instead of materializing an
/// unbounded value.
struct BoundedWriter<'a> {
    buf: &'a mut Vec<u8>,
    cap: usize,
}

impl<'a> BoundedWriter<'a> {
    fn new(buf: &'a mut Vec<u8>, cap: usize) -> Self {
        Self { buf, cap }
    }
}

impl std::io::Write for BoundedWriter<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.cap {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "journal value exceeds byte cap",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serialize `raw` to JSON text bounded at `cap_bytes` (early-aborting), then
/// redact secret-shaped segments. An oversize value yields a small sentinel
/// object rather than a truncated-and-invalid JSON fragment.
#[must_use]
pub fn redact_journal_text(raw: &Value, cap_bytes: usize) -> String {
    let mut buf = Vec::with_capacity(cap_bytes.min(4096));
    let bounded = {
        let mut ser = serde_json::Serializer::new(BoundedWriter::new(&mut buf, cap_bytes));
        raw.serialize(&mut ser).is_ok()
    };
    if !bounded {
        // Conflating "over cap" with "genuine serialize error" is safe here
        // ONLY because serializing a `serde_json::Value` is infallible except
        // for the `BoundedWriter` IO abort — so a non-`Ok` result can only mean
        // the value exceeded `cap_bytes`.
        return format!("{{\"__journal_truncated\":true,\"cap_bytes\":{cap_bytes}}}");
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    redact_secret_like_segments(&text)
}

fn sqlite_error(error: rusqlite::Error) -> ToolError {
    storage_error(format!("sqlite error: {error}"))
}

fn storage_error(message: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "journal_store_error".to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_schema_failure(error: ToolError, reason: &str) {
        let message = error.to_string();
        assert!(
            message.contains(&format!("[{reason}]")),
            "expected {reason}, got {message}"
        );
    }

    fn create_database(path: &Path, sql: &str, version: i64) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(sql).unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
    }

    fn row(exec: &str, ord: u64, name: &str) -> StepJournalRow {
        StepJournalRow {
            execution_id: exec.into(),
            step_ordinal: ord,
            seq_base: ord * 3,
            name: name.into(),
            value: "\"v\"".into(),
            ok: true,
            elapsed_ms: 1,
            recorded_at: 100,
            actor_key: Some("actor1".into()),
            route_scope: "default".into(),
            capability_filter_fingerprint: None,
            replayed_from: None,
        }
    }

    #[tokio::test]
    async fn flush_then_load_returns_rows_in_ordinal_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = StepJournalStore::open(dir.path().join("journal.db"))
            .await
            .unwrap();
        store
            .flush(vec![row("e1", 1, "b"), row("e1", 0, "a")])
            .await
            .unwrap();
        let got = store.load("e1").await.unwrap();
        assert_eq!(
            got.iter().map(|r| r.step_ordinal).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(got[0].name, "a");
        assert_eq!(got[0].seq_base, 0);
        assert_eq!(got[1].seq_base, 3);
        assert!(store.load("missing").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn flush_is_idempotent_on_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = StepJournalStore::open(dir.path().join("journal.db"))
            .await
            .unwrap();
        store.flush(vec![row("e1", 0, "a")]).await.unwrap();
        // Re-flushing the same key must not error or duplicate.
        store.flush(vec![row("e1", 0, "a")]).await.unwrap();
        assert_eq!(store.load("e1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn valid_v1_reopens_from_two_independent_stores() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let first = StepJournalStore::open(path.clone()).await.unwrap();
        first.flush(vec![row("e1", 0, "a")]).await.unwrap();
        drop(first);

        let second = StepJournalStore::open(path.clone()).await.unwrap();
        let third = StepJournalStore::open(path).await.unwrap();
        assert_eq!(second.load("e1").await.unwrap().len(), 1);
        third.flush(vec![row("e2", 0, "b")]).await.unwrap();
        assert_eq!(second.load("e2").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn newer_schema_is_rejected_without_relabeling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.db");
        create_database(&path, CREATE_TABLE, SCHEMA_VERSION + 1);
        let before = std::fs::read(&path).unwrap();

        let error = StepJournalStore::open(path.clone()).await.unwrap_err();
        assert_schema_failure(error, "unsupported_version");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION + 1);
    }

    #[tokio::test]
    async fn corrupt_database_is_rejected_without_rewriting_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let before = b"this is not a sqlite database".to_vec();
        std::fs::write(&path, &before).unwrap();

        let error = StepJournalStore::open(path.clone()).await.unwrap_err();
        assert_schema_failure(error, "corrupt");
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn v1_missing_table_is_rejected_without_rewriting_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.db");
        create_database(
            &path,
            "CREATE TABLE unrelated (id INTEGER);",
            SCHEMA_VERSION,
        );
        let before = std::fs::read(&path).unwrap();

        let error = StepJournalStore::open(path.clone()).await.unwrap_err();
        assert_schema_failure(error, "schema_mismatch");
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn v1_wrong_columns_are_rejected_without_rewriting_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.db");
        create_database(
            &path,
            "CREATE TABLE step_journal (execution_id TEXT PRIMARY KEY, value BLOB);",
            SCHEMA_VERSION,
        );
        let before = std::fs::read(&path).unwrap();

        let error = StepJournalStore::open(path.clone()).await.unwrap_err();
        assert_schema_failure(error, "schema_mismatch");
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn prune_older_than_deletes_only_stale_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = StepJournalStore::open(dir.path().join("journal.db"))
            .await
            .unwrap();
        let mut old = row("e_old", 0, "a");
        old.recorded_at = 50;
        let mut fresh = row("e_new", 0, "b");
        fresh.recorded_at = 200;
        store.flush(vec![old, fresh]).await.unwrap();

        let deleted = store.prune_older_than(100).await.unwrap();
        assert_eq!(deleted, 1);
        assert!(store.load("e_old").await.unwrap().is_empty());
        assert_eq!(store.load("e_new").await.unwrap().len(), 1);
    }

    #[test]
    fn redact_journal_text_bounds_and_redacts() {
        // Oversize: a huge array must not be fully materialized; result is a
        // small bounded sentinel, never the full payload.
        let big = serde_json::json!(vec!["x".repeat(1024); 1024]);
        let out = redact_journal_text(&big, 4096);
        assert!(
            out.len() <= 4096 + 64,
            "must be bounded near cap, got {}",
            out.len()
        );
        assert!(out.contains("__journal_truncated"));
        // Secret-shaped: a value that looks like a token is masked.
        let secret = serde_json::json!({"authorization": "Bearer sk-abcdef1234567890extra"});
        let red = redact_journal_text(&secret, 4096);
        assert!(
            !red.contains("sk-abcdef1234567890extra"),
            "token must be redacted: {red}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn db_file_is_owner_only_after_open() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let _store = StepJournalStore::open(path.clone()).await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "journal db must be 0600");
    }
}
