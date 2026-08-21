//! `UsageStore`: a small connection-pooled SQLite store for gateway call
//! telemetry. Mirrors `labby-auth`'s `SqliteStore` (`crates/labby-auth/src/sqlite.rs`).
//! No at-rest encryption is needed here (the store holds no credentials), but
//! file permissions ARE restricted to owner-only: `actor` is a stable
//! per-user OAuth subject identifier, which is privacy-sensitive even though
//! it is not a secret.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};

use labby_runtime::error::ToolError;

use super::types::UpstreamCallRecord;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
// Bounds read/write interleaving under WAL mode — SQLite still serializes
// actual writers regardless of connection count, so this does not buy write
// parallelism, only concurrent readers alongside a writer.
const SQLITE_POOL_SIZE: usize = 4;
const SCHEMA_VERSION: i64 = 2;
/// Max rows deleted per `DELETE` statement in `prune_older_than`'s batching
/// loop, so a large prune backlog doesn't hold the writer lock in one shot.
const PRUNE_BATCH_SIZE: i64 = 5_000;
/// Caps in-flight fire-and-forget usage-write tasks (see
/// `upstream/pool/usage_record.rs`). Telemetry writes are best-effort: when
/// saturated, a write is dropped and logged rather than the caller blocking
/// or an unbounded number of tasks/connections piling up under a burst.
const WRITE_SEMAPHORE_PERMITS: usize = 64;

#[derive(Clone)]
pub struct UsageStore {
    conns: Arc<Vec<Mutex<Connection>>>,
    next_conn: Arc<AtomicUsize>,
    path: Arc<PathBuf>,
    write_semaphore: Arc<tokio::sync::Semaphore>,
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
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(WRITE_SEMAPHORE_PERMITS)),
        })
    }

    /// `pub(crate)` accessor so `upstream/pool/usage_record.rs` (a sibling
    /// module tree in this crate) can acquire a permit before spawning a
    /// fire-and-forget write, without exposing the field itself.
    pub(crate) fn write_semaphore(&self) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(&self.write_semaphore)
    }

    pub async fn record_call(&self, record: UpstreamCallRecord) -> Result<(), ToolError> {
        debug_assert!(
            !record.actor.is_empty(),
            "UpstreamCallRecord.actor must not be empty — use \"unattributed\" for missing subjects"
        );
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO upstream_calls (
                    ts_unix, upstream_name, tool_name, capability, operation,
                    subject_scoped, actor, outcome, elapsed_ms, response_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.ts_unix,
                    record.upstream_name,
                    record.tool_name,
                    record.capability,
                    record.operation,
                    record.subject_scoped,
                    record.actor,
                    record.outcome,
                    record.elapsed_ms,
                    record.response_bytes,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Delete rows older than `cutoff_unix`. Returns the total number of
    /// deleted rows.
    ///
    /// Deletes in bounded batches (`PRUNE_BATCH_SIZE` rows per statement)
    /// rather than one unbounded `DELETE`, so a large backlog doesn't hold
    /// SQLite's single writer lock for an extended stretch. Loops until a
    /// batch deletes zero rows.
    pub async fn prune_older_than(&self, cutoff_unix: i64) -> Result<u64, ToolError> {
        let mut total_deleted: u64 = 0;
        loop {
            let deleted = self
                .with_conn(move |conn| {
                    let deleted = conn
                        .execute(
                            "DELETE FROM upstream_calls WHERE id IN (
                                SELECT id FROM upstream_calls WHERE ts_unix < ?1 LIMIT ?2
                             )",
                            params![cutoff_unix, PRUNE_BATCH_SIZE],
                        )
                        .map_err(sqlite_error)?;
                    Ok(deleted as u64)
                })
                .await?;
            total_deleted += deleted;
            if deleted == 0 {
                break;
            }
        }
        Ok(total_deleted)
    }

    /// Spawn a background loop that periodically prunes rows older than
    /// `retention_secs`. Ticks every `interval`; missed ticks are skipped
    /// (not backlogged) so a slow prune never causes a burst of catch-up runs.
    pub fn spawn_prune_loop(self: Arc<Self>, retention_secs: i64, interval: std::time::Duration) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Tracks consecutive prune failures so a sustained failure (disk
            // full, permissions) escalates to `error` instead of looking
            // identical to a single transient blip in the logs.
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
                            tracing::info!(deleted, "pruned stale gateway usage records");
                        }
                    }
                    Err(error) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            tracing::error!(
                                error = %error,
                                consecutive_failures,
                                "gateway usage prune failed repeatedly"
                            );
                        } else {
                            tracing::warn!(
                                error = %error,
                                consecutive_failures,
                                "gateway usage prune failed"
                            );
                        }
                    }
                }
            }
        });
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
    ensure_restrictive_permissions(path)?;
    conn.busy_timeout(std::time::Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(sqlite_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    // Safe alongside WAL: reduces per-insert fsync cost. This is a write-heavy
    // best-effort telemetry table, not a durability-critical one — losing the
    // last few writes on a hard crash is an acceptable tradeoff.
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(sqlite_error)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS upstream_calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_unix INTEGER NOT NULL,
            upstream_name TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            capability TEXT NOT NULL DEFAULT 'tools',
            operation TEXT NOT NULL DEFAULT 'tool.call',
            subject_scoped INTEGER NOT NULL DEFAULT 0,
            actor TEXT NOT NULL DEFAULT 'unattributed',
            outcome TEXT NOT NULL,
            elapsed_ms INTEGER NOT NULL,
            response_bytes INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_ts ON upstream_calls(ts_unix);
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_page ON upstream_calls(ts_unix DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_upstream ON upstream_calls(upstream_name, ts_unix);
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_tool ON upstream_calls(upstream_name, tool_name);
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_qualified_tool ON upstream_calls((upstream_name || '::' || tool_name));
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_actor ON upstream_calls(actor);",
    )
    .map_err(sqlite_error)?;
    migrate_v2(&conn)?;
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
        .map_err(sqlite_error)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", path.display()));
        if sidecar.exists() {
            ensure_restrictive_permissions(&sidecar)?;
        }
    }
    Ok(conn)
}

fn migrate_v2(conn: &Connection) -> Result<(), ToolError> {
    let mut statement = conn
        .prepare("PRAGMA table_info(upstream_calls)")
        .map_err(sqlite_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_error)?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()
        .map_err(sqlite_error)?;
    drop(statement);
    for (name, definition) in [
        ("capability", "TEXT NOT NULL DEFAULT 'tools'"),
        ("operation", "TEXT NOT NULL DEFAULT 'tool.call'"),
        ("subject_scoped", "INTEGER NOT NULL DEFAULT 0"),
        ("response_bytes", "INTEGER"),
    ] {
        if !columns.contains(name) {
            conn.execute_batch(&format!(
                "ALTER TABLE upstream_calls ADD COLUMN {name} {definition};"
            ))
            .map_err(sqlite_error)?;
        }
    }
    Ok(())
}

impl UsageStore {
    pub async fn metrics(
        &self,
        query: super::query::UsageMetricsQuery,
    ) -> Result<super::query::UsageMetrics, ToolError> {
        self.with_conn(move |conn| {
            let tx = conn.unchecked_transaction().map_err(sqlite_error)?;
            let result = (|| {
            let conn = &*tx;
            let (where_clause, bind) = usage_where_clause(
                &query.since_unix, &query.until_unix, &query.upstream, &query.tool,
                &query.actor, &query.outcome, &query.search, &query.allowed_upstreams,
            );
            let (window_where_clause, window_bind) = usage_where_clause(
                &query.since_unix, &query.until_unix, &None, &None,
                &None, &None, &None, &query.allowed_upstreams,
            );
            let has_detail_filters = query.upstream.is_some()
                || query.tool.is_some()
                || query.actor.is_some()
                || query.outcome.is_some()
                || query
                    .search
                    .as_deref()
                    .is_some_and(|search| !search.trim().is_empty());

            let bounded_total = bounded_matching_count(conn, &where_clause, &bind)?;
            ensure_metrics_row_limit("usage metrics query", bounded_total)?;

            let (total_calls, error_calls, avg_elapsed_ms): (i64, i64, f64) = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*), SUM(CASE WHEN outcome != 'ok' THEN 1 ELSE 0 END), COALESCE(AVG(elapsed_ms), 0.0) FROM upstream_calls {where_clause}"
                    ),
                    rusqlite::params_from_iter(bind.iter()),
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                            row.get(2)?,
                        ))
                    },
                )
                .map_err(sqlite_error)?;

            if query.include_facets && has_detail_filters {
                let bounded_window_total =
                    bounded_matching_count(conn, &window_where_clause, &window_bind)?;
                ensure_metrics_row_limit("usage facet window", bounded_window_total)?;
            }
            let window_total_calls = if has_detail_filters {
                conn.query_row(
                    &format!("SELECT COUNT(*) FROM upstream_calls {window_where_clause}"),
                    rusqlite::params_from_iter(window_bind.iter()),
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?
            } else {
                total_calls
            };
            if query.include_facets && !has_detail_filters {
                ensure_metrics_row_limit("usage facet window", window_total_calls)?;
            }

            let time_zone = query
                .timezone
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| {
                    if name.len() > 128 {
                        return Err(ToolError::InvalidParam {
                            message: "timezone must be a valid IANA zone name".to_string(),
                            param: "timezone".to_string(),
                        });
                    }
                    jiff::tz::TimeZone::get(name).map_err(|error| ToolError::InvalidParam {
                        message: format!("invalid IANA timezone {name:?}: {error}"),
                        param: "timezone".to_string(),
                    })
                })
                .transpose()?;
            let offset_seconds = i64::from(query.timezone_offset_minutes) * 60;

            // Stream the ordered rows instead of materializing two full vectors.
            // The exact aggregate row ceiling above bounds the SQLite sort and
            // the iterator keeps host heap use constant.
            let mut latency_stmt = conn
                .prepare(&format!(
                    "SELECT elapsed_ms, ts_unix FROM upstream_calls {where_clause} ORDER BY elapsed_ms ASC"
                ))
                .map_err(sqlite_error)?;
            let latency_rows = latency_stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(sqlite_error)?;
            let mut hourly_counts = [0_i64; 24];
            let rank = |percentile: i64| {
                total_calls
                    .saturating_mul(percentile)
                    .saturating_add(99)
                    .div_euclid(100)
                    .saturating_sub(1)
            };
            let (p50_rank, p95_rank, p99_rank) = (rank(50), rank(95), rank(99));
            let mut p50_elapsed_ms = 0;
            let mut p95_elapsed_ms = 0;
            let mut p99_elapsed_ms = 0;
            for (index, row) in latency_rows.enumerate() {
                let (elapsed_ms, ts_unix) = row.map_err(sqlite_error)?;
                let index = i64::try_from(index).unwrap_or(i64::MAX);
                if index == p50_rank { p50_elapsed_ms = elapsed_ms; }
                if index == p95_rank { p95_elapsed_ms = elapsed_ms; }
                if index == p99_rank { p99_elapsed_ms = elapsed_ms; }
                let hour = if let Some(time_zone) = &time_zone {
                    let timestamp = jiff::Timestamp::from_second(ts_unix).map_err(|error| {
                        ToolError::internal_message(format!(
                            "invalid persisted usage timestamp {ts_unix}: {error}"
                        ))
                    })?;
                    timestamp.to_zoned(time_zone.clone()).hour()
                } else {
                    i8::try_from(
                        ts_unix
                            .saturating_add(offset_seconds)
                            .div_euclid(3600)
                            .rem_euclid(24),
                    )
                    .unwrap_or_default()
                };
                let hour_index = usize::from(u8::try_from(hour).unwrap_or_default());
                hourly_counts[hour_index] += 1;
            }
            let hourly = hourly_counts
                .into_iter()
                .enumerate()
                .map(|(hour, calls)| super::query::UsageHourCount {
                    hour: u8::try_from(hour).unwrap_or_default(),
                    calls,
                })
                .collect::<Vec<_>>();

            let peak_per_min = conn
                .query_row(
                    &format!(
                        "SELECT COALESCE(MAX(calls), 0) FROM (SELECT COUNT(*) AS calls FROM upstream_calls {where_clause} GROUP BY (ts_unix / 60))"
                    ),
                    rusqlite::params_from_iter(bind.iter()),
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?;

            // One dimensional target rollup powers top, least-used, distinct,
            // and stable-target latency rankings. The previous implementation
            // repeated the same grouped scan three times.
            let mut target_stmt = conn
                .prepare(&format!(
                    "SELECT upstream_name, tool_name, capability, operation, subject_scoped, COUNT(*) AS calls, SUM(CASE WHEN outcome != 'ok' THEN 1 ELSE 0 END) AS failed, CAST(COUNT(*) AS REAL) AS calls_real, TOTAL(elapsed_ms) AS elapsed_total FROM upstream_calls {where_clause} GROUP BY upstream_name, tool_name, capability, operation, subject_scoped"
                ))
                .map_err(sqlite_error)?;
            let target_rollups = target_stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok((
                        super::query::UsageToolCount {
                            upstream: row.get(0)?,
                            tool: row.get(1)?,
                            capability: row.get(2)?,
                            operation: row.get(3)?,
                            subject_scoped: row.get(4)?,
                            calls: row.get(5)?,
                            failed: row.get(6)?,
                        },
                        row.get::<_, f64>(7)?,
                        row.get::<_, f64>(8)?,
                    ))
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            let mut top_tools = target_rollups
                .iter()
                .map(|(tool, _, _)| tool.clone())
                .collect::<Vec<_>>();
            top_tools.sort_by(|left, right| {
                right
                    .calls
                    .cmp(&left.calls)
                    .then_with(|| left.upstream.cmp(&right.upstream))
                    .then_with(|| left.tool.cmp(&right.tool))
                    .then_with(|| left.capability.cmp(&right.capability))
                    .then_with(|| left.operation.cmp(&right.operation))
                    .then_with(|| left.subject_scoped.cmp(&right.subject_scoped))
            });
            top_tools.truncate(super::query::TOP_N);

            let mut least_tools = target_rollups
                .iter()
                .map(|(tool, _, _)| tool.clone())
                .collect::<Vec<_>>();
            least_tools.sort_by(|left, right| {
                left.calls
                    .cmp(&right.calls)
                    .then_with(|| left.upstream.cmp(&right.upstream))
                    .then_with(|| left.tool.cmp(&right.tool))
                    .then_with(|| left.capability.cmp(&right.capability))
                    .then_with(|| left.operation.cmp(&right.operation))
                    .then_with(|| left.subject_scoped.cmp(&right.subject_scoped))
            });
            least_tools.truncate(super::query::TOP_N);

            let mut stable_targets: BTreeMap<(String, String), (f64, f64)> = BTreeMap::new();
            for (tool, calls_real, elapsed_total) in &target_rollups {
                let entry = stable_targets
                    .entry((tool.upstream.clone(), tool.tool.clone()))
                    .or_default();
                entry.0 += *calls_real;
                entry.1 += *elapsed_total;
            }
            let distinct_tools = i64::try_from(stable_targets.len()).unwrap_or(i64::MAX);
            let mut slowest_tools = stable_targets
                .iter()
                .map(|((upstream, tool), (calls, elapsed_total))| {
                    super::query::UsageLatencyStat {
                        upstream: upstream.clone(),
                        tool: tool.clone(),
                        avg_elapsed_ms: elapsed_total / calls,
                    }
                })
                .collect::<Vec<_>>();
            slowest_tools.sort_by(|left, right| {
                right
                    .avg_elapsed_ms
                    .total_cmp(&left.avg_elapsed_ms)
                    .then_with(|| left.upstream.cmp(&right.upstream))
                    .then_with(|| left.tool.cmp(&right.tool))
            });
            slowest_tools.truncate(super::query::TOP_N);

            // One actor grouping supplies both the exact distinct count and the
            // top-actor ranking.
            let mut actor_stmt = conn
                .prepare(&format!(
                    "SELECT actor, COUNT(*) AS calls FROM upstream_calls {where_clause} GROUP BY actor ORDER BY calls DESC, actor ASC"
                ))
                .map_err(sqlite_error)?;
            let actor_counts = actor_stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UsageActorCount {
                        actor: row.get(0)?,
                        calls: row.get(1)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;
            let distinct_actors = i64::try_from(actor_counts.len()).unwrap_or(i64::MAX);
            let top_actors = actor_counts
                .into_iter()
                .take(super::query::TOP_N)
                .collect::<Vec<_>>();

            let error_where = append_usage_predicate(&where_clause, "outcome != 'ok'");
            let mut errors_stmt = conn.prepare(&format!("SELECT outcome, COUNT(*) AS calls FROM upstream_calls {error_where} GROUP BY outcome ORDER BY calls DESC, outcome ASC")).map_err(sqlite_error)?;
            let errors = errors_stmt.query_map(rusqlite::params_from_iter(bind.iter()), |row| Ok(super::query::UsageErrorCount { kind: row.get(0)?, calls: row.get(1)? })).map_err(sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sqlite_error)?;

            let mut upstreams_stmt = conn.prepare(&format!("SELECT upstream_name, COUNT(*) AS calls, SUM(CASE WHEN outcome != 'ok' THEN 1 ELSE 0 END) AS failed FROM upstream_calls {where_clause} GROUP BY upstream_name ORDER BY calls DESC, upstream_name ASC")).map_err(sqlite_error)?;
            let upstreams = upstreams_stmt.query_map(rusqlite::params_from_iter(bind.iter()), |row| Ok(super::query::UsageUpstreamCount { upstream: row.get(0)?, calls: row.get(1)?, failed: row.get(2)? })).map_err(sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sqlite_error)?;

            let timeseries = if query.bucket_count > 0 {
                if let (Some(since), Some(until)) = (query.since_unix, query.until_unix) {
                    let count = query.bucket_count.clamp(1, super::query::MAX_METRICS_BUCKETS);
                    let span = until.saturating_sub(since);
                    if span > 0 {
                        let width = (span + count as i64 - 1) / count as i64;
                        let mut buckets = (0..count).map(|index| super::query::UsageTimeBucket { ts_unix: since + index as i64 * width, calls: 0, failed: 0 }).collect::<Vec<_>>();
                        let mut bucket_bind = bind.clone();
                        bucket_bind.push(rusqlite::types::Value::Integer((count - 1) as i64)); let max_index_param = bucket_bind.len();
                        bucket_bind.push(rusqlite::types::Value::Integer(since)); let since_param = bucket_bind.len();
                        bucket_bind.push(rusqlite::types::Value::Integer(width)); let width_param = bucket_bind.len();
                        let mut bucket_stmt = conn.prepare(&format!("SELECT MIN(?{max_index_param}, ((ts_unix - ?{since_param}) / ?{width_param})) AS bucket_index, COUNT(*) AS calls, SUM(CASE WHEN outcome != 'ok' THEN 1 ELSE 0 END) AS failed FROM upstream_calls {where_clause} GROUP BY bucket_index ORDER BY bucket_index ASC")).map_err(sqlite_error)?;
                        let rows = bucket_stmt.query_map(rusqlite::params_from_iter(bucket_bind.iter()), |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))).map_err(sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sqlite_error)?;
                        for (index, calls, failed) in rows { if let Ok(index) = usize::try_from(index) && let Some(bucket) = buckets.get_mut(index) { bucket.calls = calls; bucket.failed = failed; } }
                        buckets
                    } else { Vec::new() }
                } else { Vec::new() }
            } else { Vec::new() };

            let facets = if query.include_facets {
                let limit = super::query::MAX_METRICS_FACETS + 1;
                let mut tools_stmt = conn.prepare(&format!("SELECT DISTINCT upstream_name, tool_name FROM upstream_calls {window_where_clause} ORDER BY upstream_name ASC, tool_name ASC LIMIT {limit}")).map_err(sqlite_error)?;
                let tools = tools_stmt.query_map(rusqlite::params_from_iter(window_bind.iter()), |row| Ok(super::query::UsageToolFacet { upstream: row.get(0)?, tool: row.get(1)? })).map_err(sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(sqlite_error)?;
                let mut actors_stmt = conn.prepare(&format!("SELECT DISTINCT actor FROM upstream_calls {window_where_clause} ORDER BY actor ASC LIMIT {limit}")).map_err(sqlite_error)?;
                let actors = actors_stmt.query_map(rusqlite::params_from_iter(window_bind.iter()), |row| row.get(0)).map_err(sqlite_error)?.collect::<rusqlite::Result<Vec<String>>>().map_err(sqlite_error)?;
                let mut upstreams_stmt = conn.prepare(&format!("SELECT DISTINCT upstream_name FROM upstream_calls {window_where_clause} ORDER BY upstream_name ASC LIMIT {limit}")).map_err(sqlite_error)?;
                let upstreams = upstreams_stmt.query_map(rusqlite::params_from_iter(window_bind.iter()), |row| row.get(0)).map_err(sqlite_error)?.collect::<rusqlite::Result<Vec<String>>>().map_err(sqlite_error)?;
                let mut outcomes_stmt = conn.prepare(&format!("SELECT DISTINCT outcome FROM upstream_calls {window_where_clause} ORDER BY outcome ASC LIMIT {limit}")).map_err(sqlite_error)?;
                let outcomes = outcomes_stmt.query_map(rusqlite::params_from_iter(window_bind.iter()), |row| row.get(0)).map_err(sqlite_error)?.collect::<rusqlite::Result<Vec<String>>>().map_err(sqlite_error)?;
                ensure_facet_limit("tools", tools.len())?;
                ensure_facet_limit("actors", actors.len())?;
                ensure_facet_limit("upstreams", upstreams.len())?;
                ensure_facet_limit("outcomes", outcomes.len())?;
                super::query::UsageFacets { tools, actors, upstreams, outcomes }
            } else { super::query::UsageFacets::default() };

            Ok(super::query::UsageMetrics { window_total_calls, total_calls, error_calls, avg_elapsed_ms, p50_elapsed_ms, p95_elapsed_ms, p99_elapsed_ms, distinct_tools, distinct_actors, peak_per_min, top_tools, least_tools, top_actors, slowest_tools, errors, upstreams, hourly, timeseries, facets })
            })();
            match result {
                Ok(metrics) => {
                    tx.commit().map_err(sqlite_error)?;
                    Ok(metrics)
                }
                Err(error) => Err(error),
            }
        }).await
    }

    /// Returns a keyset-paginated page, an optional total, and the next cursor.
    pub async fn list_calls(
        &self,
        query: super::query::UsageCallsQuery,
    ) -> Result<
        (
            Vec<super::query::UpstreamCallRecordView>,
            Option<i64>,
            Option<super::query::UsageCursor>,
        ),
        ToolError,
    > {
        self.with_conn(move |conn| {
            let tx = conn.unchecked_transaction().map_err(sqlite_error)?;
            let conn = &*tx;
            let (where_clause, mut bind) = usage_where_clause(
                &query.since_unix,
                &query.until_unix,
                &query.upstream,
                &query.tool,
                &query.actor,
                &query.outcome,
                &query.search,
                &query.allowed_upstreams,
            );

            let total = if query.include_total {
                Some(
                    conn.query_row(
                        &format!("SELECT COUNT(*) FROM upstream_calls {where_clause}"),
                        rusqlite::params_from_iter(bind.iter()),
                        |row| row.get(0),
                    )
                    .map_err(sqlite_error)?,
                )
            } else {
                None
            };

            let mut page_where = where_clause;
            if let Some(cursor) = query.cursor {
                let prefix = if page_where.is_empty() {
                    "WHERE"
                } else {
                    "AND"
                };
                page_where.push_str(&format!(
                    " {prefix} (ts_unix < ?{} OR (ts_unix = ?{} AND id < ?{}))",
                    bind.len() + 1,
                    bind.len() + 2,
                    bind.len() + 3,
                ));
                bind.push(rusqlite::types::Value::Integer(cursor.ts_unix));
                bind.push(rusqlite::types::Value::Integer(cursor.ts_unix));
                bind.push(rusqlite::types::Value::Integer(cursor.id));
            }

            // Defense-in-depth: clamp here too, regardless of whether the
            // caller (`gateway/manager/usage.rs`) already clamped.
            let limit = query.limit.clamp(1, super::query::MAX_CALLS_LIMIT);
            bind.push(rusqlite::types::Value::Integer(
                limit.saturating_add(1) as i64
            ));
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT id, ts_unix, upstream_name, tool_name, capability, operation, \
                     subject_scoped, actor, outcome, elapsed_ms, response_bytes \
                     FROM upstream_calls {page_where} \
                     ORDER BY ts_unix DESC, id DESC LIMIT ?{}",
                    bind.len()
                ))
                .map_err(sqlite_error)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UpstreamCallRecordView {
                        id: row.get(0)?,
                        ts_unix: row.get(1)?,
                        upstream: row.get(2)?,
                        tool: row.get(3)?,
                        capability: row.get(4)?,
                        operation: row.get(5)?,
                        subject_scoped: row.get(6)?,
                        actor: row.get(7)?,
                        outcome: row.get(8)?,
                        elapsed_ms: row.get(9)?,
                        response_bytes: row.get(10)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            let mut rows = rows;
            let has_more = rows.len() > limit;
            rows.truncate(limit);
            let next_cursor = has_more.then(|| {
                let last = rows.last().expect("has_more implies a non-empty page");
                super::query::UsageCursor {
                    ts_unix: last.ts_unix,
                    id: last.id,
                }
            });

            let result = (rows, total, next_cursor);
            drop(stmt);
            tx.commit().map_err(sqlite_error)?;
            Ok(result)
        })
        .await
    }
}

/// Build a `WHERE ...` clause (or empty string) plus its positional bind
/// values for the optional since/until/upstream filters shared by `metrics`
/// and `list_calls`, plus an optional `allowed_upstreams` allowlist (used to
/// enforce route scope for scoped callers — see `gateway/manager/usage.rs`).
fn append_usage_predicate(where_clause: &str, predicate: &str) -> String {
    if where_clause.is_empty() {
        format!("WHERE {predicate}")
    } else {
        format!("{where_clause} AND {predicate}")
    }
}

fn ensure_facet_limit(name: &str, count: usize) -> Result<(), ToolError> {
    if count <= super::query::MAX_METRICS_FACETS as usize {
        Ok(())
    } else {
        Err(ToolError::InvalidParam {
            message: format!(
                "usage {name} facet exceeds {} distinct values; narrow the time window",
                super::query::MAX_METRICS_FACETS
            ),
            param: "include_facets".to_string(),
        })
    }
}

fn ensure_metrics_row_limit(name: &str, count: i64) -> Result<(), ToolError> {
    if count <= super::query::MAX_METRICS_MATCHING_ROWS {
        Ok(())
    } else {
        Err(ToolError::InvalidParam {
            message: format!(
                "{name} matches {count} rows; narrow the time window or filters to at most {} rows",
                super::query::MAX_METRICS_MATCHING_ROWS
            ),
            param: "since_unix".to_string(),
        })
    }
}

fn bounded_matching_count(
    conn: &Connection,
    where_clause: &str,
    bind: &[rusqlite::types::Value],
) -> Result<i64, ToolError> {
    let mut bounded_bind = bind.to_vec();
    bounded_bind.push(rusqlite::types::Value::Integer(
        super::query::MAX_METRICS_MATCHING_ROWS.saturating_add(1),
    ));
    conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM (SELECT 1 FROM upstream_calls {where_clause} LIMIT ?{})",
            bounded_bind.len()
        ),
        rusqlite::params_from_iter(bounded_bind.iter()),
        |row| row.get(0),
    )
    .map_err(sqlite_error)
}

fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '%' => escaped.push_str("\\%"),
            '_' => escaped.push_str("\\_"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn usage_where_clause(
    since_unix: &Option<i64>,
    until_unix: &Option<i64>,
    upstream: &Option<String>,
    tool: &Option<String>,
    actor: &Option<String>,
    outcome: &Option<String>,
    search: &Option<String>,
    allowed_upstreams: &Option<Vec<String>>,
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
    if let Some(tool) = tool {
        clauses.push(format!(
            "(upstream_name || '::' || tool_name) = ?{}",
            bind.len() + 1
        ));
        bind.push(rusqlite::types::Value::Text(tool.clone()));
    }
    if let Some(actor) = actor {
        clauses.push(format!("actor = ?{}", bind.len() + 1));
        bind.push(rusqlite::types::Value::Text(actor.clone()));
    }
    if let Some(outcome) = outcome {
        if outcome == "failed" {
            clauses.push("outcome != 'ok'".to_string());
        } else {
            clauses.push(format!("outcome = ?{}", bind.len() + 1));
            bind.push(rusqlite::types::Value::Text(outcome.clone()));
        }
    }
    if let Some(search) = search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        clauses.push(format!(
            "LOWER(upstream_name || ' ' || tool_name || ' ' || capability || ' ' || operation || ' ' || actor || ' ' || outcome) LIKE ?{} ESCAPE char(92)",
            bind.len() + 1
        ));
        bind.push(rusqlite::types::Value::Text(format!(
            "%{}%",
            escape_like_pattern(&search.to_lowercase())
        )));
    }
    if let Some(allowed) = allowed_upstreams {
        if allowed.is_empty() {
            clauses.push("1 = 0".to_string());
        } else {
            let placeholders: Vec<String> = allowed
                .iter()
                .map(|name| {
                    bind.push(rusqlite::types::Value::Text(name.clone()));
                    format!("?{}", bind.len())
                })
                .collect();
            clauses.push(format!("upstream_name IN ({})", placeholders.join(", ")));
        }
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
            actor: "unattributed".to_string(),
            outcome: "ok".to_string(),
            elapsed_ms: 42,
            response_bytes: Some(512),
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
    async fn open_migrates_v1_usage_rows_without_losing_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "PRAGMA user_version = 1;
                 CREATE TABLE upstream_calls (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts_unix INTEGER NOT NULL,
                    upstream_name TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    actor TEXT NOT NULL DEFAULT 'unattributed',
                    outcome TEXT NOT NULL,
                    elapsed_ms INTEGER NOT NULL
                 );
                 INSERT INTO upstream_calls
                    (ts_unix, upstream_name, tool_name, actor, outcome, elapsed_ms)
                 VALUES (1000, 'github', 'search_repos', 'unattributed', 'ok', 42);",
            )
            .unwrap();
        }

        let store = UsageStore::open(path).await.unwrap();
        let rows = store
            .list_calls(super::super::query::UsageCallsQuery {
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap()
            .0;

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].capability, "tools");
        assert_eq!(rows[0].operation, "tool.call");
        assert!(!rows[0].subject_scoped);
        assert_eq!(rows[0].response_bytes, None);
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

    /// Exercises the loop-until-zero batching logic in `prune_older_than` with
    /// several successive stale rows (well under one batch), proving the loop
    /// terminates and deletes everything below cutoff, not just one batch.
    #[tokio::test]
    async fn prune_older_than_loops_until_all_stale_rows_are_gone() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        for ts in 0..10 {
            store.record_call(sample_record(ts)).await.unwrap();
        }
        store.record_call(sample_record(1_000)).await.unwrap();

        let deleted = store.prune_older_than(500).await.unwrap();
        assert_eq!(deleted, 10);

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
                allowed_upstreams: None,
                ..Default::default()
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
    async fn metrics_respects_allowed_upstreams_scope() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        let mut github = sample_record(1_000);
        github.upstream_name = "github".to_string();
        store.record_call(github).await.unwrap();

        let mut gateway_alpha = sample_record(1_001);
        gateway_alpha.upstream_name = "gateway-alpha".to_string();
        store.record_call(gateway_alpha).await.unwrap();

        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
                allowed_upstreams: Some(vec!["github".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(metrics.total_calls, 1);
        assert_eq!(metrics.top_tools.len(), 1);
        assert_eq!(metrics.top_tools[0].upstream, "github");
    }

    #[tokio::test]
    async fn metrics_timeseries_covers_full_window_when_raw_page_is_capped() {
        use super::super::query::{UsageCallsQuery, UsageMetricsQuery};

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();
        store
            .with_conn(|conn| {
                conn.execute_batch("BEGIN IMMEDIATE").map_err(super::sqlite_error)?;
                for index in 0..500 {
                    conn.execute(
                        "INSERT INTO upstream_calls (ts_unix, upstream_name, tool_name, actor, outcome, elapsed_ms) VALUES (1800, 'github', 'search', 'agent', ?1, 100)",
                        [if index < 100 { "timeout" } else { "ok" }],
                    )
                    .map_err(super::sqlite_error)?;
                }
                for _ in 0..1_200 {
                    conn.execute(
                        "INSERT INTO upstream_calls (ts_unix, upstream_name, tool_name, actor, outcome, elapsed_ms) VALUES (84600, 'github', 'search', 'agent', 'ok', 10)",
                        [],
                    )
                    .map_err(super::sqlite_error)?;
                }
                conn.execute_batch("COMMIT").map_err(super::sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();

        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: Some(0),
                until_unix: Some(86_400),
                bucket_count: 24,
                include_facets: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let (page, total, cursor) = store
            .list_calls(UsageCallsQuery {
                since_unix: Some(0),
                until_unix: Some(86_400),
                limit: 1_000,
                include_total: true,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(page.len(), 1_000);
        assert_eq!(total, Some(1_700));
        assert!(cursor.is_some());
        assert_eq!(metrics.window_total_calls, 1_700);
        assert_eq!(metrics.total_calls, 1_700);
        assert_eq!(metrics.error_calls, 100);
        assert_eq!(metrics.timeseries.len(), 24);
        assert_eq!(metrics.timeseries[0].calls, 500);
        assert_eq!(metrics.timeseries[0].failed, 100);
        assert_eq!(metrics.timeseries[23].calls, 1_200);
        assert_eq!(metrics.p50_elapsed_ms, 10);
        assert_eq!(metrics.p95_elapsed_ms, 100);
        assert_eq!(metrics.peak_per_min, 1_200);
        assert_eq!(metrics.errors[0].kind, "timeout");
        assert_eq!(metrics.errors[0].calls, 100);
        assert_eq!(metrics.facets.tools.len(), 1);
        assert_eq!(metrics.facets.actors, vec!["agent"]);
    }

    #[tokio::test]
    async fn metrics_window_total_and_facets_ignore_operator_filters() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        let mut github_one = sample_record(1_000);
        github_one.actor = "alice".to_string();
        store.record_call(github_one).await.unwrap();

        let mut github_two = sample_record(1_001);
        github_two.actor = "bob".to_string();
        store.record_call(github_two).await.unwrap();

        let mut gitlab = sample_record(1_002);
        gitlab.upstream_name = "gitlab".to_string();
        gitlab.tool_name = "issues".to_string();
        gitlab.actor = "carol".to_string();
        store.record_call(gitlab).await.unwrap();

        let metrics = store
            .metrics(UsageMetricsQuery {
                upstream: Some("github".to_string()),
                include_facets: true,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(metrics.total_calls, 2);
        assert_eq!(metrics.window_total_calls, 3);
        assert_eq!(metrics.facets.upstreams, vec!["github", "gitlab"]);
        assert_eq!(metrics.facets.actors, vec!["alice", "bob", "carol"]);
    }

    #[tokio::test]
    async fn metrics_rejects_incomplete_facet_inventory() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();
        store
            .with_conn(|conn| {
                conn.execute_batch(&format!(
                    "WITH RECURSIVE seq(value) AS (
                        VALUES(0)
                        UNION ALL SELECT value + 1 FROM seq
                        WHERE value < {}
                     )
                     INSERT INTO upstream_calls (
                        ts_unix, upstream_name, tool_name, actor, outcome, elapsed_ms
                     )
                     SELECT 1000, 'github', 'search', printf('actor-%04d', value), 'ok', 1
                     FROM seq;",
                    super::super::query::MAX_METRICS_FACETS
                ))
                .map_err(super::sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();

        let error = store
            .metrics(UsageMetricsQuery {
                include_facets: true,
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert!(error.to_string().contains("actors facet exceeds"));
    }

    #[tokio::test]
    async fn metrics_and_calls_apply_exact_server_side_filters() {
        use super::super::query::{UsageCallsQuery, UsageMetricsQuery};

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        let mut first = sample_record(1_000);
        first.actor = "alice".to_string();
        store.record_call(first).await.unwrap();

        let mut second = sample_record(1_001);
        second.actor = "bob".to_string();
        second.outcome = "timeout".to_string();
        store.record_call(second).await.unwrap();

        let mut third = sample_record(1_002);
        third.upstream_name = "gitlab".to_string();
        third.tool_name = "issues".to_string();
        third.actor = "bob".to_string();
        store.record_call(third).await.unwrap();

        let metrics = store
            .metrics(UsageMetricsQuery {
                tool: Some("github::search_repos".to_string()),
                actor: Some("bob".to_string()),
                outcome: Some("failed".to_string()),
                search: Some("timeout".to_string()),
                include_facets: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let (rows, total, _) = store
            .list_calls(UsageCallsQuery {
                tool: Some("github::search_repos".to_string()),
                actor: Some("bob".to_string()),
                outcome: Some("failed".to_string()),
                search: Some("timeout".to_string()),
                limit: 50,
                include_total: true,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(metrics.window_total_calls, 3);
        assert_eq!(metrics.total_calls, 1);
        assert_eq!(metrics.error_calls, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(total, Some(1));
        assert_eq!(rows[0].actor, "bob");
        assert_eq!(rows[0].outcome, "timeout");
        assert_eq!(metrics.facets.tools.len(), 2);
        assert_eq!(metrics.facets.actors, vec!["alice", "bob"]);
    }

    #[tokio::test]
    async fn usage_search_treats_sql_wildcards_as_literals() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();
        for (ts, tool) in [
            (1_000, "literal%target"),
            (1_001, "literalXtarget"),
            (1_002, "literal_target"),
            (1_003, "literalYtarget"),
        ] {
            let mut record = sample_record(ts);
            record.tool_name = tool.to_string();
            store.record_call(record).await.unwrap();
        }

        let percent = store
            .metrics(UsageMetricsQuery {
                search: Some("%".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let underscore = store
            .metrics(UsageMetricsQuery {
                search: Some("_".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(percent.total_calls, 1);
        assert_eq!(percent.top_tools[0].tool, "literal%target");
        assert_eq!(underscore.total_calls, 1);
        assert_eq!(underscore.top_tools[0].tool, "literal_target");
    }

    #[test]
    fn tool_filter_preserves_delimiter_bearing_upstream_names() {
        let (clause, bind) = super::usage_where_clause(
            &None,
            &None,
            &None,
            &Some("labby::github-chat::search_repos".to_string()),
            &None,
            &None,
            &None,
            &None,
        );
        assert_eq!(clause, "WHERE (upstream_name || '::' || tool_name) = ?1");
        assert_eq!(bind.len(), 1);
    }

    #[test]
    fn aggregate_limits_accept_boundary_and_reject_overflow() {
        assert!(
            super::ensure_metrics_row_limit(
                "query",
                super::super::query::MAX_METRICS_MATCHING_ROWS
            )
            .is_ok()
        );
        assert!(
            super::ensure_metrics_row_limit(
                "query",
                super::super::query::MAX_METRICS_MATCHING_ROWS + 1
            )
            .is_err()
        );
        assert!(
            super::ensure_facet_limit("actors", super::super::query::MAX_METRICS_FACETS as usize)
                .is_ok()
        );
        assert!(
            super::ensure_facet_limit(
                "actors",
                super::super::query::MAX_METRICS_FACETS as usize + 1
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn metrics_iana_timezone_handles_dst_fall_back_exactly() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();
        for ts in [1_793_511_000, 1_793_514_600, 1_793_518_200] {
            store.record_call(sample_record(ts)).await.unwrap();
        }

        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: Some(1_793_510_000),
                until_unix: Some(1_793_519_000),
                timezone: Some("America/New_York".to_string()),
                // A fixed -04:00 offset would incorrectly put the second call
                // in hour 2 after the DST fold; the IANA zone must override it.
                timezone_offset_minutes: -240,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(metrics.hourly[1].calls, 2);
        assert_eq!(metrics.hourly[2].calls, 1);
        assert_eq!(
            metrics
                .hourly
                .iter()
                .map(|bucket| bucket.calls)
                .sum::<i64>(),
            3
        );
    }

    #[tokio::test]
    async fn metrics_rejects_invalid_iana_timezone() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();
        store.record_call(sample_record(1_000)).await.unwrap();

        let error = store
            .metrics(UsageMetricsQuery {
                timezone: Some("Mars/Olympus_Mons".to_string()),
                ..Default::default()
            })
            .await
            .expect_err("unknown IANA timezone must fail closed");
        assert_eq!(error.kind(), "invalid_param");
    }

    #[tokio::test]
    async fn list_calls_uses_stable_keyset_cursor_and_optional_total() {
        use super::super::query::UsageCallsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        for ts in 0..5 {
            store.record_call(sample_record(ts)).await.unwrap();
        }

        let (page, total, cursor) = store
            .list_calls(UsageCallsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
                allowed_upstreams: None,
                limit: 2,
                cursor: None,
                include_total: true,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(page.len(), 2);
        assert_eq!(total, Some(5));
        let cursor = cursor.expect("next cursor");
        // Newest first.
        assert_eq!(page[0].ts_unix, 4);

        let (next, total, next_cursor) = store
            .list_calls(UsageCallsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
                allowed_upstreams: None,
                limit: 2,
                cursor: Some(cursor),
                include_total: false,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            total, None,
            "deep pages must skip a full recount by default"
        );
        assert_eq!(
            next.iter().map(|row| row.ts_unix).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert!(next_cursor.is_some());
    }

    #[tokio::test]
    async fn list_calls_clamps_zero_limit_to_one_row() {
        use super::super::query::UsageCallsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();
        store.record_call(sample_record(1)).await.unwrap();
        store.record_call(sample_record(2)).await.unwrap();

        let (page, total, cursor) = store
            .list_calls(UsageCallsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
                allowed_upstreams: None,
                limit: 0,
                cursor: None,
                include_total: false,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(page.len(), 1);
        assert_eq!(total, None);
        assert!(cursor.is_some());
    }

    #[tokio::test]
    async fn deep_keyset_page_stays_within_large_row_budget() {
        use super::super::query::{UsageCallsQuery, UsageCursor};

        const ROWS: i64 = 100_000;
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();
        store
            .with_conn(|conn| {
                conn.execute_batch("BEGIN IMMEDIATE").map_err(super::sqlite_error)?;
                for ts in 0..ROWS {
                    conn.execute(
                        "INSERT INTO upstream_calls (ts_unix, upstream_name, tool_name, actor, outcome, elapsed_ms) VALUES (?1, 'github', 'search', 'actor', 'ok', 1)",
                        [ts],
                    )
                    .map_err(super::sqlite_error)?;
                }
                conn.execute_batch("COMMIT").map_err(super::sqlite_error)?;
                Ok(())
            })
            .await
            .unwrap();

        let page = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.list_calls(UsageCallsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
                allowed_upstreams: None,
                limit: 100,
                cursor: Some(UsageCursor {
                    ts_unix: 1_000,
                    id: 1_001,
                }),
                include_total: false,
                ..Default::default()
            }),
        )
        .await
        .expect("100k-row deep page exceeded the two-second regression budget")
        .unwrap();

        assert_eq!(page.0.len(), 100);
        assert_eq!(page.1, None);
        assert_eq!(page.0[0].ts_unix, 999);
    }

    /// Regression guard for the write-semaphore backpressure mechanism
    /// (`upstream/pool/usage_record.rs`): locks in the permit count and
    /// proves that once all permits are held, a further `try_acquire`
    /// fails rather than succeeding unboundedly. This is the store-level
    /// half of the backpressure proof; `call_tool` exercises the same
    /// semaphore end-to-end in `upstream/pool/tools_call.rs`.
    #[tokio::test]
    async fn write_semaphore_rejects_acquire_once_permits_are_exhausted() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        let semaphore = store.write_semaphore();
        let mut held_permits = Vec::with_capacity(super::WRITE_SEMAPHORE_PERMITS);
        for _ in 0..super::WRITE_SEMAPHORE_PERMITS {
            held_permits.push(
                semaphore
                    .clone()
                    .try_acquire_owned()
                    .expect("permit available until exhausted"),
            );
        }

        assert!(
            semaphore.try_acquire().is_err(),
            "acquiring beyond WRITE_SEMAPHORE_PERMITS should fail"
        );

        // Releasing one permit frees up capacity again.
        drop(held_permits.pop());
        assert!(
            semaphore.try_acquire().is_ok(),
            "a released permit should be acquirable again"
        );
    }
}
