//! SQLite-backed durable Code Mode pause/resume store.
//!
//! Port of Cloudflare `CodemodeRuntime`'s durable storage
//! (`packages/codemode/src/runtime.ts`): `cm_executions` → `codemode_runs`,
//! `cm_log` → `codemode_call_log`. The connection-pool / pragma / 0600 /
//! path-traversal boilerplate mirrors `crates/labby/src/acp/sqlite_persistence.rs`.
//!
//! # Threat model (what is / is NOT stored)
//!
//! Stored, per run: a sha256 of the submitted code (source is **never** stored),
//! caller identity (`actor_key`), admin flag, route scope, capability filter
//! fingerprint, and lifecycle status. Per call: the tool id, a canonical hash of
//! the RAW args (for divergence detection), the **redacted** args/result (audit +
//! divergence-message only), approval flag, ephemerality, and call ordering.
//!
//! NOT stored: raw args/results (redacted to `[redacted]` before disk),
//! snippet source.
//!
//! Integrity: `status`, `is_admin`, `capability_filter_fingerprint`,
//! `route_scope`, `actor_key` are HMAC-SHA256 signed (`integrity_sig`) and
//! verified before any authorization decision trusts them — this defends the
//! raw-SQLite-file-write tamper path (a flipped `is_admin`/`status` fails
//! verification and the caller fails closed).
//!
//! File perms are 0600 (owner read/write only). At single-operator scale this
//! only stops *other* Unix users; metadata is still readable at the operator's
//! own privilege level (accepted, documented residual confidentiality risk). The
//! redaction dictionary only masks known secret key names — a novel upstream
//! secret field name could slip through (documented limitation).
//!
//! # No truncation
//!
//! Any single journaled value over `MAX_DURABLE_VALUE_BYTES` fails the run
//! (`ValueTooLarge`) rather than being truncated — truncation would feed resumed
//! code corrupted data (`runtime.ts:146-153`).
//!
//! # Known limitations (v1)
//!
//! - **Redaction dictionary:** the on-disk byte-scan test only catches known
//!   secret key names (`token`/`api_key`/`password`/`secret`/`authorization`,
//!   plus value heuristics). A novel upstream secret field name could slip
//!   through — a documented residual confidentiality risk.
//! - **Local providers (`state`/`git`) + resume (C3):** Labby's runner-reserved
//!   local providers are dispatched on a separate path
//!   (`runner_drive::enqueue_local_provider_call`) that does NOT flow through
//!   `host.call_tool`. They are now journaled as **ephemeral** durable entries
//!   through `CodeModeHost::decide_local` / `record_local` (`decide` with
//!   `ephemeral = true`) BEFORE the local dispatch runs, so they participate in
//!   the `seq` spine and are divergence-checked on resume — while an ephemeral
//!   entry RE-EXECUTES on replay rather than replaying a stored result, so the
//!   FS/git side effect re-runs deterministically-enough instead of being
//!   silently double-applied out of the spine. This **lifted** the former
//!   fail-closed non-pause-capable exclusion (plan Wave 4 Task 4.1): runs that
//!   touch `state`/`git` are pause-capable again. The companion
//!   `codemode.step(name, fn)` primitive (non-ephemeral: journal-once,
//!   replay-thereafter) journals arbitrary nondeterministic work under the same
//!   spine — see `docs/dev/CODE_MODE.md`.

use std::path::PathBuf;

use hmac::{Hmac, KeyInit, Mac};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

use labby_codemode::redact_trace_value;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Cap for any single serialized value stored in the durable log (args, a
/// recorded result). Truncating is never an option — replay would feed resumed
/// code corrupted data — so a breach fails the run instead. 1 MiB matches
/// Cloudflare's `MAX_DURABLE_VALUE_BYTES` (`runtime.ts:153`).
pub const MAX_DURABLE_VALUE_BYTES: usize = 1_000_000;

/// Redaction size cap for stored redacted args/results. Large enough that the
/// redaction pass does not itself truncate a legitimately-sized value below the
/// durable cap (redaction is for secret-key masking, not size bounding here).
const REDACT_CAP_BYTES: usize = MAX_DURABLE_VALUE_BYTES;

/// Terminal executions retained per store (newest kept), mirroring Cloudflare's
/// `DEFAULT_MAX_EXECUTIONS` (`runtime.ts:144`).
const DEFAULT_MAX_EXECUTIONS: i64 = 50;

type HmacSha256 = Hmac<Sha256>;

// ── Status + state enums ──────────────────────────────────────────────────────

/// Lifecycle status of a durable run (`cm_executions.status`).
///
/// Note: `parse_status` (not `from_str`) is inherent on purpose — an inherent
/// `from_str` trips `clippy::should_implement_trait` and would fail the
/// all-features clippy gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Paused,
    Completed,
    Error,
    Rejected,
    Expired,
}

impl RunStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Paused => "paused",
            RunStatus::Completed => "completed",
            RunStatus::Error => "error",
            RunStatus::Rejected => "rejected",
            RunStatus::Expired => "expired",
        }
    }

    #[must_use]
    pub fn parse_status(s: &str) -> Option<Self> {
        match s {
            "running" => Some(RunStatus::Running),
            "paused" => Some(RunStatus::Paused),
            "completed" => Some(RunStatus::Completed),
            "error" => Some(RunStatus::Error),
            "rejected" => Some(RunStatus::Rejected),
            "expired" => Some(RunStatus::Expired),
            _ => None,
        }
    }
}

/// State of a single call-log entry (`codemode_call_log.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogState {
    Pending,
    Executing,
    Applied,
    Reverted,
    /// A corrupt/unrecognized state string read back from disk. Fails closed:
    /// the decider treats an `Unknown`-state entry as replay divergence and
    /// refuses to advance the run, mirroring the run-status path where a
    /// tampered row reads back as `verified == false`. Never written by the
    /// store — it exists only so a corrupt on-disk value cannot be laundered
    /// into a valid terminal state (e.g. `Reverted`) that would silently
    /// re-journal and re-dispatch the call.
    Unknown,
}

impl LogState {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            LogState::Pending => "pending",
            LogState::Executing => "executing",
            LogState::Applied => "applied",
            LogState::Reverted => "reverted",
            LogState::Unknown => "unknown",
        }
    }

    /// Parse a persisted state string. An unrecognized value maps to
    /// [`LogState::Unknown`] (fail closed) rather than `None` so `row_to_log_entry`
    /// can surface corruption to the decider instead of silently defaulting to a
    /// valid terminal state.
    #[must_use]
    pub fn parse_state(s: &str) -> Self {
        match s {
            "pending" => LogState::Pending,
            "executing" => LogState::Executing,
            "applied" => LogState::Applied,
            "reverted" => LogState::Reverted,
            _ => LogState::Unknown,
        }
    }
}

// ── Row / input types ─────────────────────────────────────────────────────────

/// Fields for inserting a fresh `running` run (port of `runtime.ts:355 begin`).
#[derive(Debug, Clone)]
pub struct NewRun {
    pub execution_id: String,
    pub code_hash: String,
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub route_scope: String,
    pub capability_filter_fingerprint: String,
    pub expires_at_ms: i64,
}

/// The HMAC-verified authorization view of a run. Borrowed from a [`Run`] ONLY
/// through [`Run::verified_auth`], which returns `None` for a tampered row — so
/// a holder of this type is proof the signed tuple verified. The non-auth fields
/// (`status`, `code_hash`, timestamps) stay directly public on [`Run`]; only the
/// auth-bearing tuple is gated here.
#[derive(Debug, Clone, Copy)]
pub struct VerifiedRunAuth<'a> {
    pub actor_key: Option<&'a str>,
    pub is_admin: bool,
    pub route_scope: &'a str,
    pub capability_filter_fingerprint: &'a str,
}

/// A loaded run row. `verified` is the HMAC integrity-check result — callers
/// MUST treat `verified == false` as tampered and fail closed. The auth-bearing
/// fields (`is_admin`, `route_scope`, `actor_key`,
/// `capability_filter_fingerprint`) are reachable through [`Run::verified_auth`]
/// so a caller cannot read them off a tampered row by accident.
#[derive(Debug, Clone)]
pub struct Run {
    pub execution_id: String,
    pub code_hash: String,
    pub status: RunStatus,
    pub actor_key: Option<String>,
    pub is_admin: bool,
    pub route_scope: String,
    pub capability_filter_fingerprint: String,
    // Loaded from the real columns; carried for audit/inspection. Not read on
    // the current decide/lifecycle paths.
    #[allow(dead_code)]
    pub created_at_ms: i64,
    #[allow(dead_code)]
    pub updated_at_ms: i64,
    #[allow(dead_code)]
    pub expires_at_ms: i64,
    /// HMAC verification result over the signed tuple. `false` ⇒ tampered.
    pub verified: bool,
}

impl Run {
    /// The auth-bearing fields, but ONLY if the row passed HMAC verification.
    /// Returns `None` for a tampered row so no gate can read forged
    /// `is_admin`/`route_scope`/`actor_key`/`capability_filter_fingerprint`.
    #[must_use]
    pub fn verified_auth(&self) -> Option<VerifiedRunAuth<'_>> {
        if !self.verified {
            return None;
        }
        Some(VerifiedRunAuth {
            actor_key: self.actor_key.as_deref(),
            is_admin: self.is_admin,
            route_scope: &self.route_scope,
            capability_filter_fingerprint: &self.capability_filter_fingerprint,
        })
    }
}

/// Input for journaling a fresh call. `raw_args` is redacted+hashed inside the
/// store — the store owns the pre-redaction boundary.
///
/// `seq` is `u64` — the protocol/host type — so a caller cannot construct a
/// negative sequence. The single `as i64` narrowing happens only at the SQL bind
/// site inside [`CodeModePauseStore::upsert_log_entry`].
#[derive(Debug, Clone)]
pub struct NewLogEntry {
    pub seq: u64,
    pub tool_id: String,
    pub raw_args: Value,
    pub requires_approval: bool,
    pub ephemeral: bool,
}

/// A loaded call-log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub seq: i64,
    pub tool_id: String,
    pub args_hash: String,
    // Audit-only fields loaded from the real columns; not read on the current
    // decide/replay paths (divergence uses `args_hash`; approval uses `state`).
    #[allow(dead_code)]
    pub redacted_args: String,
    pub redacted_result: Option<Value>,
    #[allow(dead_code)]
    pub requires_approval: bool,
    pub ephemeral: bool,
    pub state: LogState,
}

// ── Errors ────────────────────────────────────────────────────────────────────

/// Errors from the durable pause store.
#[derive(Debug, thiserror::Error)]
pub enum CodeModePauseStoreError {
    /// A journaled value exceeded `MAX_DURABLE_VALUE_BYTES`. The decider turns
    /// this into a terminal run failure (never a truncation).
    #[error("{0}")]
    ValueTooLarge(String),
    /// Underlying SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(String),
    /// r2d2 pool / join / internal error.
    #[error("internal: {0}")]
    Internal(String),
}

impl From<rusqlite::Error> for CodeModePauseStoreError {
    fn from(e: rusqlite::Error) -> Self {
        CodeModePauseStoreError::Sqlite(e.to_string())
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// SQLite-backed durable Code Mode pause/resume store.
///
/// Clone is cheap — all state is behind `Arc` (r2d2 pools clone by `Arc`).
#[derive(Clone)]
pub struct CodeModePauseStore {
    write_pool: Pool<SqliteConnectionManager>,
    read_pool: Pool<SqliteConnectionManager>,
    hmac_key: std::sync::Arc<Vec<u8>>,
}

fn pragma_init(
    query_only: bool,
) -> impl Fn(&mut Connection) -> rusqlite::Result<()> + Send + Sync + 'static {
    move |conn| {
        conn.busy_timeout(std::time::Duration::from_millis(5_000))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "mmap_size", 134_217_728_i64)?;
        conn.pragma_update(None, "cache_size", -65_536_i64)?;
        conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)?;
        if query_only {
            conn.pragma_update(None, "query_only", "true")?;
        }
        Ok(())
    }
}

impl CodeModePauseStore {
    /// Open (or create) the durable pause database at `db_path`.
    ///
    /// `db_path` must not contain `..` components. The file is created with mode
    /// 0600 on first open.
    pub async fn open(db_path: PathBuf) -> Result<Self, CodeModePauseStoreError> {
        if db_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(CodeModePauseStoreError::Internal(format!(
                "codemode pause db path must not contain `..` components: {}",
                db_path.display()
            )));
        }

        let hmac_key = std::sync::Arc::new(codemode_hmac_key().to_vec());
        let path = db_path.clone();

        let (write_pool, read_pool) = tokio::task::spawn_blocking(move || -> Result<_, String> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all: {e}"))?;
            }

            #[cfg(unix)]
            create_db_file_0600(&path);

            let write_manager = SqliteConnectionManager::file(&path).with_init(pragma_init(false));
            let write_pool = Pool::builder()
                .max_size(1)
                .connection_timeout(std::time::Duration::from_secs(30))
                .build(write_manager)
                .map_err(|e| format!("build write pool: {e}"))?;

            {
                let conn = write_pool
                    .get()
                    .map_err(|e| format!("get write conn: {e}"))?;
                migrate(&conn).map_err(|e| format!("migrate: {e}"))?;
            }

            let read_manager = SqliteConnectionManager::file(&path).with_init(pragma_init(true));
            let read_pool = Pool::builder()
                .max_size(4)
                .connection_timeout(std::time::Duration::from_secs(30))
                .build(read_manager)
                .map_err(|e| format!("build read pool: {e}"))?;

            Ok((write_pool, read_pool))
        })
        .await
        .map_err(|e| CodeModePauseStoreError::Internal(format!("db open join: {e}")))?
        .map_err(CodeModePauseStoreError::Internal)?;

        Ok(Self {
            write_pool,
            read_pool,
            hmac_key,
        })
    }

    /// Open using `~/.labby/codemode_pauses.db` (via `labby_runtime::lab_home`).
    pub async fn from_lab_home() -> Result<Self, CodeModePauseStoreError> {
        let path = labby_runtime::lab_home().join("codemode_pauses.db");
        Self::open(path).await
    }

    // ── Internal blocking helpers ─────────────────────────────────────────────

    async fn blocking_write<T, F>(
        &self,
        label: &'static str,
        f: F,
    ) -> Result<T, CodeModePauseStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, CodeModePauseStoreError> + Send + 'static,
    {
        let pool = self.write_pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| CodeModePauseStoreError::Internal(format!("{label} pool get: {e}")))?;
            f(&conn)
        })
        .await
        .map_err(|e| CodeModePauseStoreError::Internal(format!("{label} join: {e}")))?
    }

    async fn blocking_read<T, F>(
        &self,
        label: &'static str,
        f: F,
    ) -> Result<T, CodeModePauseStoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, CodeModePauseStoreError> + Send + 'static,
    {
        let pool = self.read_pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| CodeModePauseStoreError::Internal(format!("{label} pool get: {e}")))?;
            f(&conn)
        })
        .await
        .map_err(|e| CodeModePauseStoreError::Internal(format!("{label} join: {e}")))?
    }

    // ── HMAC helpers ──────────────────────────────────────────────────────────

    fn sign_run_fields(
        &self,
        status: RunStatus,
        is_admin: bool,
        capability_filter_fingerprint: &str,
        route_scope: &str,
        actor_key: Option<&str>,
    ) -> String {
        let message = run_integrity_message(
            status,
            is_admin,
            capability_filter_fingerprint,
            route_scope,
            actor_key,
        );
        hmac_tag(&self.hmac_key, &message)
    }

    fn verify_run_fields(
        &self,
        status: RunStatus,
        is_admin: bool,
        capability_filter_fingerprint: &str,
        route_scope: &str,
        actor_key: Option<&str>,
        sig_hex: &str,
    ) -> bool {
        let message = run_integrity_message(
            status,
            is_admin,
            capability_filter_fingerprint,
            route_scope,
            actor_key,
        );
        let expected = match hex::decode(sig_hex) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };
        let mut mac =
            HmacSha256::new_from_slice(&self.hmac_key).expect("HMAC accepts any key length");
        mac.update(message.as_bytes());
        mac.verify_slice(&expected).is_ok()
    }

    // ── Public CRUD ───────────────────────────────────────────────────────────

    /// Insert a fresh `running` run (HMAC-signed) and prune terminal rows down
    /// to `DEFAULT_MAX_EXECUTIONS`. Port of `runtime.ts:355 begin`.
    pub async fn begin(&self, run: NewRun) -> Result<(), CodeModePauseStoreError> {
        let now = now_ms();
        let sig = self.sign_run_fields(
            RunStatus::Running,
            run.is_admin,
            &run.capability_filter_fingerprint,
            &run.route_scope,
            run.actor_key.as_deref(),
        );
        self.blocking_write("begin", move |conn| {
            conn.execute(
                "INSERT INTO codemode_runs
                    (execution_id, code_hash, status, actor_key, is_admin,
                     route_scope, capability_filter_fingerprint, integrity_sig,
                     created_at_ms, updated_at_ms, expires_at_ms)
                 VALUES (?1, ?2, 'running', ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
                params![
                    run.execution_id,
                    run.code_hash,
                    run.actor_key,
                    run.is_admin as i64,
                    run.route_scope,
                    run.capability_filter_fingerprint,
                    sig,
                    now,
                    run.expires_at_ms,
                ],
            )?;
            prune_terminal(conn, DEFAULT_MAX_EXECUTIONS, &run.execution_id)?;
            Ok(())
        })
        .await
    }

    /// Load a run row, verifying its HMAC integrity signature. Port of
    /// `runtime.ts #executionRow` + the V6 integrity check.
    pub async fn load_run(
        &self,
        execution_id: &str,
    ) -> Result<Option<Run>, CodeModePauseStoreError> {
        let store = self.clone();
        let id = execution_id.to_string();
        self.blocking_read("load_run", move |conn| load_run_row(conn, &store, &id))
            .await
    }

    /// CAS `paused → running` (port of `runtime.ts:383 resume`). Re-signs the
    /// integrity tuple for the new status. Returns `true` iff a paused row
    /// transitioned.
    pub async fn resume_to_running(
        &self,
        execution_id: &str,
    ) -> Result<bool, CodeModePauseStoreError> {
        // Load first to recompute the signature for the target status.
        let Some(run) = self.load_run(execution_id).await? else {
            return Ok(false);
        };
        if !run.verified || run.status != RunStatus::Paused {
            return Ok(false);
        }
        let sig = self.sign_run_fields(
            RunStatus::Running,
            run.is_admin,
            &run.capability_filter_fingerprint,
            &run.route_scope,
            run.actor_key.as_deref(),
        );
        let now = now_ms();
        let id = execution_id.to_string();
        self.blocking_write("resume_to_running", move |conn| {
            let n = conn.execute(
                "UPDATE codemode_runs
                    SET status = 'running', integrity_sig = ?1, updated_at_ms = ?2
                  WHERE execution_id = ?3 AND status = 'paused'",
                params![sig, now, id],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// Set a run's status with a re-signed `integrity_sig`. Used by
    /// complete/fail/reject/expire. Returns `true` iff a row was updated.
    pub async fn set_status(
        &self,
        execution_id: &str,
        to: RunStatus,
        error: Option<&str>,
    ) -> Result<bool, CodeModePauseStoreError> {
        let Some(run) = self.load_run(execution_id).await? else {
            return Ok(false);
        };
        // Re-sign for the target status. We trust the currently-stored
        // authorization fields only to carry them forward; if the row was
        // tampered (`verified == false`) a re-sign would launder the tamper, so
        // refuse.
        if !run.verified {
            return Ok(false);
        }
        let sig = self.sign_run_fields(
            to,
            run.is_admin,
            &run.capability_filter_fingerprint,
            &run.route_scope,
            run.actor_key.as_deref(),
        );
        let now = now_ms();
        let id = execution_id.to_string();
        let to_str = to.as_str();
        let err_owned = error.map(str::to_string);
        self.blocking_write("set_status", move |conn| {
            let n = conn.execute(
                "UPDATE codemode_runs
                    SET status = ?1, integrity_sig = ?2, updated_at_ms = ?3,
                        error = ?4
                  WHERE execution_id = ?5",
                params![to_str, sig, now, err_owned, id],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// Reject a run, guarded so it only fires on a still-`paused` run (F3). Port
    /// of `runtime.ts:668 reject`: the transition is conditional on the run
    /// observing `status='paused'`, so a stale/duplicate reject (already
    /// resumed, running, or terminal) is a no-op and cannot force-terminate a
    /// live Running run. Re-signs the integrity tuple for the `rejected` status.
    /// Also reverts any still-`pending` log entries. Returns `true` iff a paused
    /// row transitioned.
    pub async fn reject_paused(
        &self,
        execution_id: &str,
        error: Option<&str>,
    ) -> Result<bool, CodeModePauseStoreError> {
        let Some(run) = self.load_run(execution_id).await? else {
            return Ok(false);
        };
        // Refuse to re-sign a tampered row (would launder the tamper) or a
        // non-paused run.
        if !run.verified || run.status != RunStatus::Paused {
            return Ok(false);
        }
        let sig = self.sign_run_fields(
            RunStatus::Rejected,
            run.is_admin,
            &run.capability_filter_fingerprint,
            &run.route_scope,
            run.actor_key.as_deref(),
        );
        let now = now_ms();
        let id = execution_id.to_string();
        let err_owned = error.map(str::to_string);
        self.blocking_write("reject_paused", move |conn| {
            let n = conn.execute(
                "UPDATE codemode_runs
                    SET status = 'rejected', integrity_sig = ?1, updated_at_ms = ?2,
                        error = ?3
                  WHERE execution_id = ?4 AND status = 'paused'",
                params![sig, now, err_owned, id],
            )?;
            if n == 1 {
                conn.execute(
                    "UPDATE codemode_call_log SET state = 'reverted'
                      WHERE run_id = ?1 AND state = 'pending'",
                    params![id],
                )?;
            }
            Ok(n == 1)
        })
        .await
    }

    /// Load a single call-log entry by `(run_id, seq)`. Port of
    /// `runtime.ts #logRow`.
    pub async fn get_log_entry(
        &self,
        run_id: &str,
        seq: i64,
    ) -> Result<Option<LogEntry>, CodeModePauseStoreError> {
        let run = run_id.to_string();
        self.blocking_read("get_log_entry", move |conn| {
            conn.query_row(
                "SELECT seq, tool_id, args_hash, redacted_args, redacted_result,
                        requires_approval, ephemeral, state
                   FROM codemode_call_log
                  WHERE run_id = ?1 AND seq = ?2",
                params![run, seq],
                row_to_log_entry,
            )
            .optional()
            .map_err(CodeModePauseStoreError::from)
        })
        .await
    }

    /// Journal a fresh call (`INSERT OR REPLACE`). Redacts + hashes here from the
    /// RAW args. Port of `runtime.ts:502-516`. On oversize returns
    /// `ValueTooLarge` (never truncates).
    pub async fn upsert_log_entry(
        &self,
        run_id: &str,
        entry: NewLogEntry,
    ) -> Result<(), CodeModePauseStoreError> {
        // Hash the RAW args (divergence spine); redact separately for audit.
        let args_hash = Self::canonical_args_hash(&entry.raw_args);
        let redacted = redact_trace_value(&entry.raw_args, REDACT_CAP_BYTES);
        let redacted_args = serde_json::to_string(&redacted).map_err(|e| {
            CodeModePauseStoreError::Internal(format!("serialize redacted args: {e}"))
        })?;
        // No-truncation gate uses the RAW serialized size.
        let raw_len = serde_json::to_vec(&entry.raw_args)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        if raw_len > MAX_DURABLE_VALUE_BYTES {
            return Err(CodeModePauseStoreError::ValueTooLarge(too_large_message(
                &format!("Arguments to {}", entry.tool_id),
                raw_len,
            )));
        }
        let state = if entry.requires_approval {
            LogState::Pending
        } else {
            LogState::Executing
        };
        let run = run_id.to_string();
        // The one and only `as i64` narrowing of `seq`: SQLite has no u64 column
        // type, but `NewLogEntry.seq` is `u64` so callers can't hand us a
        // negative sequence.
        let seq_i = entry.seq as i64;
        self.blocking_write("upsert_log_entry", move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO codemode_call_log
                    (run_id, seq, tool_id, args_hash, redacted_args,
                     redacted_result, requires_approval, ephemeral, state,
                     applied_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, NULL)",
                params![
                    run,
                    seq_i,
                    entry.tool_id,
                    args_hash,
                    redacted_args,
                    entry.requires_approval as i64,
                    entry.ephemeral as i64,
                    state.as_str(),
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Transition a call-log entry to a new state. Port of
    /// `runtime.ts #setEntryState`.
    pub async fn set_entry_state(
        &self,
        run_id: &str,
        seq: i64,
        state: LogState,
    ) -> Result<(), CodeModePauseStoreError> {
        let run = run_id.to_string();
        let state_str = state.as_str();
        self.blocking_write("set_entry_state", move |conn| {
            conn.execute(
                "UPDATE codemode_call_log SET state = ?1 WHERE run_id = ?2 AND seq = ?3",
                params![state_str, run, seq],
            )?;
            Ok(())
        })
        .await
    }

    /// Record the real result of an executed call and mark it `applied`. Port of
    /// `runtime.ts:543 recordResult`. Ephemeral entries store NULL (they
    /// re-execute on replay). Oversize result → `ValueTooLarge` (no truncation).
    pub async fn record_entry_result(
        &self,
        run_id: &str,
        seq: i64,
        raw_result: Option<Value>,
    ) -> Result<(), CodeModePauseStoreError> {
        // Look up ephemerality + tool id for message context.
        let existing = self.get_log_entry(run_id, seq).await?;
        let (ephemeral, tool_id) = match existing {
            Some(e) => (e.ephemeral, e.tool_id),
            None => {
                return Err(CodeModePauseStoreError::Internal(format!(
                    "no log entry at seq {seq}"
                )));
            }
        };

        let stored_result: Option<String> = if ephemeral {
            None
        } else {
            match raw_result {
                None => None,
                Some(ref v) => {
                    let raw_len = serde_json::to_vec(v).map(|b| b.len()).unwrap_or(usize::MAX);
                    if raw_len > MAX_DURABLE_VALUE_BYTES {
                        return Err(CodeModePauseStoreError::ValueTooLarge(too_large_message(
                            &format!("The result of {tool_id}"),
                            raw_len,
                        )));
                    }
                    let redacted = redact_trace_value(v, REDACT_CAP_BYTES);
                    Some(serde_json::to_string(&redacted).map_err(|e| {
                        CodeModePauseStoreError::Internal(format!("serialize redacted result: {e}"))
                    })?)
                }
            }
        };

        let now = now_ms();
        let run = run_id.to_string();
        self.blocking_write("record_entry_result", move |conn| {
            conn.execute(
                "UPDATE codemode_call_log
                    SET redacted_result = ?1, state = 'applied', applied_at_ms = ?2
                  WHERE run_id = ?3 AND seq = ?4",
                params![stored_result, now, run, seq],
            )?;
            Ok(())
        })
        .await
    }

    /// Load the full call log for a run ordered by `seq` (audit / list_pending).
    pub async fn load_call_log(
        &self,
        run_id: &str,
    ) -> Result<Vec<LogEntry>, CodeModePauseStoreError> {
        let run = run_id.to_string();
        self.blocking_read("load_call_log", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT seq, tool_id, args_hash, redacted_args, redacted_result,
                        requires_approval, ephemeral, state
                   FROM codemode_call_log
                  WHERE run_id = ?1
                  ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![run], row_to_log_entry)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(CodeModePauseStoreError::from)
        })
        .await
    }

    /// List pending entries for a run — only when the run is `paused`. Port of
    /// `runtime.ts:640 listPending` (single-run scope).
    pub async fn list_pending(
        &self,
        execution_id: &str,
    ) -> Result<Vec<LogEntry>, CodeModePauseStoreError> {
        let run = match self.load_run(execution_id).await? {
            Some(r) if r.verified && r.status == RunStatus::Paused => r,
            _ => return Ok(Vec::new()),
        };
        let log = self.load_call_log(&run.execution_id).await?;
        Ok(log
            .into_iter()
            .filter(|e| e.state == LogState::Pending)
            .collect())
    }

    /// Expire non-terminal runs whose last state change is older than
    /// `older_than_ms` (an absolute cutoff timestamp): `paused → rejected`,
    /// stale `running → error`. Port of `runtime.ts:699 expirePaused`. Returns
    /// the count expired.
    pub async fn expire_paused(
        &self,
        older_than_ms: i64,
    ) -> Result<usize, CodeModePauseStoreError> {
        let store = self.clone();
        self.blocking_write("expire_paused", move |conn| {
            let mut stmt = conn.prepare(
                "SELECT execution_id, status, is_admin, route_scope,
                        capability_filter_fingerprint, actor_key
                   FROM codemode_runs
                  WHERE status IN ('paused', 'running') AND updated_at_ms < ?1",
            )?;
            let rows: Vec<(String, String, bool, String, String, Option<String>)> = stmt
                .query_map(params![older_than_ms], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)? != 0,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);

            let now = now_ms();
            let mut expired = 0usize;
            for (id, status, is_admin, route_scope, fingerprint, actor_key) in rows {
                let (to, err) = if status == "paused" {
                    (RunStatus::Rejected, "Expired awaiting approval")
                } else {
                    (
                        RunStatus::Error,
                        "Expired while running — the host never completed the pass",
                    )
                };
                let sig = store.sign_run_fields(
                    to,
                    is_admin,
                    &fingerprint,
                    &route_scope,
                    actor_key.as_deref(),
                );
                let n = conn.execute(
                    "UPDATE codemode_runs
                        SET status = ?1, integrity_sig = ?2, error = ?3,
                            updated_at_ms = ?4
                      WHERE execution_id = ?5 AND status = ?6",
                    params![to.as_str(), sig, err, now, id, status],
                )?;
                if n == 0 {
                    continue;
                }
                conn.execute(
                    "UPDATE codemode_call_log SET state = 'reverted'
                      WHERE run_id = ?1 AND state = 'pending'",
                    params![id],
                )?;
                expired += 1;
            }
            Ok(expired)
        })
        .await
    }

    /// The recorded error message for a run (the nullable `error` column),
    /// used for the `Error`-status audit / envelope. `None` when absent.
    pub async fn run_error(
        &self,
        execution_id: &str,
    ) -> Result<Option<String>, CodeModePauseStoreError> {
        let id = execution_id.to_string();
        self.blocking_read("run_error", move |conn| {
            conn.query_row(
                "SELECT error FROM codemode_runs WHERE execution_id = ?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(CodeModePauseStoreError::from)
        })
        .await
    }

    /// sha256(stable_stringify(RAW args)) — the divergence hash. One hash fn,
    /// RAW input. Falls back to a stable serde string when `stable_stringify`
    /// yields `None` (non-serializable) so the hash is still deterministic.
    #[must_use]
    pub fn canonical_args_hash(args: &Value) -> String {
        let canonical = stable_stringify(args).unwrap_or_else(|| args.to_string());
        let digest = Sha256::digest(canonical.as_bytes());
        hex::encode(digest)
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

use super::now_ms;

fn too_large_message(what: &str, size: usize) -> String {
    format!(
        "{what} is too large to record durably ({size} bytes > \
         {MAX_DURABLE_VALUE_BYTES} byte limit). Write large data to a file or \
         workspace instead and pass/return a small reference (such as a path)."
    )
}

fn run_integrity_message(
    status: RunStatus,
    is_admin: bool,
    capability_filter_fingerprint: &str,
    route_scope: &str,
    actor_key: Option<&str>,
) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        status.as_str(),
        is_admin,
        capability_filter_fingerprint,
        route_scope,
        actor_key.unwrap_or("")
    )
}

fn hmac_tag(key: &[u8], message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Deterministic JSON of a value: object keys sorted recursively. Port of
/// `runtime.ts:191 stableStringify`. Returns `None` on serialization failure so
/// the caller skips the args check rather than false-diverging.
#[must_use]
pub fn stable_stringify(value: &Value) -> Option<String> {
    fn canonicalize(v: &Value) -> Value {
        match v {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), canonicalize(&map[k]));
                }
                Value::Object(out)
            }
            Value::Array(arr) => Value::Array(arr.iter().map(canonicalize).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&canonicalize(value)).ok()
}

fn row_to_log_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogEntry> {
    let redacted_result_str: Option<String> = row.get("redacted_result")?;
    let redacted_result = redacted_result_str
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let state_str: String = row.get("state")?;
    // A corrupt/unknown on-disk state is surfaced as `LogState::Unknown` (fail
    // closed) — NOT laundered into a valid terminal state. The decider refuses
    // to replay a run whose log entry reads back Unknown.
    let state = LogState::parse_state(&state_str);
    Ok(LogEntry {
        seq: row.get("seq")?,
        tool_id: row.get("tool_id")?,
        args_hash: row.get("args_hash")?,
        redacted_args: row.get("redacted_args")?,
        redacted_result,
        requires_approval: row.get::<_, i64>("requires_approval")? != 0,
        ephemeral: row.get::<_, i64>("ephemeral")? != 0,
        state,
    })
}

fn load_run_row(
    conn: &Connection,
    store: &CodeModePauseStore,
    execution_id: &str,
) -> Result<Option<Run>, CodeModePauseStoreError> {
    let row = conn
        .query_row(
            "SELECT execution_id, code_hash, status, actor_key, is_admin,
                    route_scope, capability_filter_fingerprint, integrity_sig,
                    created_at_ms, updated_at_ms, expires_at_ms
               FROM codemode_runs
              WHERE execution_id = ?1",
            params![execution_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)? != 0,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, i64>(9)?,
                    r.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?;

    let Some((
        execution_id,
        code_hash,
        status_str,
        actor_key,
        is_admin,
        route_scope,
        capability_filter_fingerprint,
        integrity_sig,
        created_at_ms,
        updated_at_ms,
        expires_at_ms,
    )) = row
    else {
        return Ok(None);
    };

    // An unknown status string is itself a tamper/corruption signal — fail
    // closed by reporting the row as unverified with a safe placeholder status.
    let status = RunStatus::parse_status(&status_str).unwrap_or(RunStatus::Error);
    let verified = RunStatus::parse_status(&status_str).is_some()
        && store.verify_run_fields(
            status,
            is_admin,
            &capability_filter_fingerprint,
            &route_scope,
            actor_key.as_deref(),
            &integrity_sig,
        );

    Ok(Some(Run {
        execution_id,
        code_hash,
        status,
        actor_key,
        is_admin,
        route_scope,
        capability_filter_fingerprint,
        created_at_ms,
        updated_at_ms,
        expires_at_ms,
        verified,
    }))
}

fn prune_terminal(conn: &Connection, keep: i64, protect_id: &str) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT execution_id FROM codemode_runs
          WHERE status IN ('completed', 'error', 'rejected', 'expired')
            AND execution_id != ?1
          ORDER BY created_at_ms DESC, execution_id DESC",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![protect_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    if ids.len() as i64 <= keep {
        return Ok(());
    }
    for id in ids.into_iter().skip(keep as usize) {
        conn.execute(
            "DELETE FROM codemode_call_log WHERE run_id = ?1",
            params![id],
        )?;
        conn.execute(
            "DELETE FROM codemode_runs WHERE execution_id = ?1",
            params![id],
        )?;
    }
    Ok(())
}

// ── Schema ────────────────────────────────────────────────────────────────────

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS codemode_runs (
    execution_id                  TEXT PRIMARY KEY,
    code_hash                     TEXT NOT NULL,
    status                        TEXT NOT NULL,
    actor_key                     TEXT,
    is_admin                      INTEGER NOT NULL,
    route_scope                   TEXT NOT NULL,
    capability_filter_fingerprint TEXT NOT NULL,
    integrity_sig                 TEXT NOT NULL,
    error                         TEXT,
    created_at_ms                 INTEGER NOT NULL,
    updated_at_ms                 INTEGER NOT NULL,
    expires_at_ms                 INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_codemode_runs_status_expires
    ON codemode_runs(status, expires_at_ms);

CREATE TABLE IF NOT EXISTS codemode_call_log (
    run_id            TEXT NOT NULL REFERENCES codemode_runs(execution_id),
    seq               INTEGER NOT NULL,
    tool_id           TEXT NOT NULL,
    args_hash         TEXT NOT NULL,
    redacted_args     TEXT NOT NULL,
    redacted_result   TEXT,
    requires_approval INTEGER NOT NULL DEFAULT 0,
    ephemeral         INTEGER NOT NULL DEFAULT 0,
    state             TEXT NOT NULL,
    applied_at_ms     INTEGER,
    PRIMARY KEY (run_id, seq)
);
";

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA_SQL)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    Ok(())
}

// ── HMAC key ──────────────────────────────────────────────────────────────────

/// Process-wide HMAC key for run-field integrity signing.
///
/// Loaded from `LABBY_CODEMODE_HMAC_SECRET`; falls back to an ephemeral key seeded
/// from PID + startup timestamp (rotates per process — cross-restart
/// verification requires setting the env var). Mirrors `acp_hmac_key`.
fn codemode_hmac_key() -> &'static [u8] {
    use std::sync::OnceLock;
    static KEY: OnceLock<Vec<u8>> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(secret) = std::env::var("LABBY_CODEMODE_HMAC_SECRET") {
            if !secret.is_empty() {
                return secret.into_bytes();
            }
        }
        tracing::warn!(
            surface = "mcp",
            service = "codemode",
            action = "hmac_key_init",
            kind = "ephemeral_key",
            "LABBY_CODEMODE_HMAC_SECRET is not set; using an ephemeral HMAC key. \
             Set LABBY_CODEMODE_HMAC_SECRET in ~/.labby/.env for cross-restart protection."
        );
        let pid = std::process::id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let input = format!("lab-codemode-hmac-ephemeral:{pid}:{now}");
        Sha256::digest(input.as_bytes()).to_vec()
    })
}

// ── Unix 0600 file creation ───────────────────────────────────────────────────

#[cfg(unix)]
fn create_db_file_0600(path: &PathBuf) {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .ok();
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store_key() -> Vec<u8> {
        b"test-fixed-codemode-hmac-key".to_vec()
    }

    /// Build a store over an in-memory-equivalent temp file with a FIXED hmac
    /// key (so tamper tests are deterministic regardless of the process key).
    async fn temp_store() -> (CodeModePauseStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemode_pauses.db");
        let mut store = CodeModePauseStore::open(path).await.expect("open");
        store.hmac_key = std::sync::Arc::new(test_store_key());
        (store, dir)
    }

    fn sample_new_run(id: &str) -> NewRun {
        NewRun {
            execution_id: id.to_string(),
            code_hash: "hash-abc".to_string(),
            actor_key: Some("actor-1".to_string()),
            is_admin: true,
            route_scope: "unscoped".to_string(),
            capability_filter_fingerprint: "fp-1".to_string(),
            expires_at_ms: now_ms() + 86_400_000,
        }
    }

    #[tokio::test]
    async fn schema_bootstrap_creates_tables_and_index_idempotently() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemode_pauses.db");
        // Open twice — migrate must be idempotent.
        let _s1 = CodeModePauseStore::open(path.clone()).await.expect("open1");
        let s2 = CodeModePauseStore::open(path.clone()).await.expect("open2");

        // Inspect via a raw connection.
        let conn = Connection::open(&path).expect("raw open");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='table' AND name IN ('codemode_runs','codemode_call_log')",
                [],
                |r| r.get(0),
            )
            .expect("count tables");
        assert_eq!(count, 2, "both tables must exist");
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='index' AND name='idx_codemode_runs_status_expires'",
                [],
                |r| r.get(0),
            )
            .expect("count index");
        assert_eq!(idx, 1, "status/expires index must exist");
        drop(s2);
    }

    #[test]
    fn parse_status_roundtrips_all_variants() {
        for s in [
            RunStatus::Running,
            RunStatus::Paused,
            RunStatus::Completed,
            RunStatus::Error,
            RunStatus::Rejected,
            RunStatus::Expired,
        ] {
            assert_eq!(RunStatus::parse_status(s.as_str()), Some(s));
        }
        assert_eq!(RunStatus::parse_status("bogus"), None);
    }

    #[test]
    fn parse_log_state_roundtrips_all_variants() {
        for s in [
            LogState::Pending,
            LogState::Executing,
            LogState::Applied,
            LogState::Reverted,
        ] {
            assert_eq!(LogState::parse_state(s.as_str()), s);
        }
        // A corrupt/unknown state fails closed to `Unknown` rather than being
        // laundered into a valid terminal state.
        assert_eq!(LogState::parse_state("bogus"), LogState::Unknown);
        assert_eq!(LogState::Unknown.as_str(), "unknown");
    }

    #[test]
    fn stable_stringify_sorts_keys_recursively() {
        let a = serde_json::json!({"b": 1, "a": {"y": 2, "x": 3}});
        let b = serde_json::json!({"a": {"x": 3, "y": 2}, "b": 1});
        assert_eq!(stable_stringify(&a), stable_stringify(&b));
        assert_eq!(
            CodeModePauseStore::canonical_args_hash(&a),
            CodeModePauseStore::canonical_args_hash(&b)
        );
    }

    #[tokio::test]
    async fn begin_and_load_run_verifies_hmac() {
        let (store, _dir) = temp_store().await;
        store.begin(sample_new_run("exec-1")).await.expect("begin");
        let run = store.load_run("exec-1").await.expect("load").expect("some");
        assert_eq!(run.status, RunStatus::Running);
        assert!(run.verified, "fresh row must verify");
        assert_eq!(run.actor_key.as_deref(), Some("actor-1"));
        assert!(run.is_admin);
    }

    #[tokio::test]
    async fn resume_to_running_cas_once() {
        let (store, _dir) = temp_store().await;
        store
            .begin(sample_new_run("exec-cas"))
            .await
            .expect("begin");
        // Not paused yet → CAS is a no-op.
        assert!(!store.resume_to_running("exec-cas").await.expect("cas"));
        store
            .set_status("exec-cas", RunStatus::Paused, None)
            .await
            .expect("pause");
        // First CAS wins, second loses.
        assert!(store.resume_to_running("exec-cas").await.expect("cas1"));
        assert!(!store.resume_to_running("exec-cas").await.expect("cas2"));
        let run = store
            .load_run("exec-cas")
            .await
            .expect("load")
            .expect("some");
        assert_eq!(run.status, RunStatus::Running);
        assert!(run.verified);
    }

    #[tokio::test]
    async fn hmac_tamper_of_is_admin_makes_verified_false() {
        let (store, dir) = temp_store().await;
        store
            .begin(sample_new_run("exec-tamper"))
            .await
            .expect("begin");
        // Flip is_admin directly in the raw file (bypassing the signer).
        let path = dir.path().join("codemode_pauses.db");
        let conn = Connection::open(&path).expect("raw open");
        conn.execute(
            "UPDATE codemode_runs SET is_admin = 0 WHERE execution_id = 'exec-tamper'",
            [],
        )
        .expect("tamper");
        drop(conn);
        let run = store
            .load_run("exec-tamper")
            .await
            .expect("load")
            .expect("some");
        assert!(
            !run.verified,
            "flipped is_admin must fail HMAC verification"
        );
    }

    #[tokio::test]
    async fn hmac_tamper_of_status_makes_verified_false() {
        let (store, dir) = temp_store().await;
        store.begin(sample_new_run("exec-st")).await.expect("begin");
        let path = dir.path().join("codemode_pauses.db");
        let conn = Connection::open(&path).expect("raw open");
        conn.execute(
            "UPDATE codemode_runs SET status = 'completed' WHERE execution_id = 'exec-st'",
            [],
        )
        .expect("tamper");
        drop(conn);
        let run = store
            .load_run("exec-st")
            .await
            .expect("load")
            .expect("some");
        assert!(!run.verified, "flipped status must fail HMAC verification");
    }

    #[tokio::test]
    async fn hmac_tamper_of_route_scope_makes_verified_false() {
        // `route_scope` is part of the HMAC-signed tuple (defends the F2
        // cross-route resume guard): a raw-file edit that moves a run into a
        // different route scope must fail verification so the resume/reject
        // handlers fail closed.
        let (store, dir) = temp_store().await;
        store.begin(sample_new_run("exec-rs")).await.expect("begin");
        let path = dir.path().join("codemode_pauses.db");
        let conn = Connection::open(&path).expect("raw open");
        conn.execute(
            "UPDATE codemode_runs SET route_scope = 'protected:evil' \
             WHERE execution_id = 'exec-rs'",
            [],
        )
        .expect("tamper");
        drop(conn);
        let run = store
            .load_run("exec-rs")
            .await
            .expect("load")
            .expect("some");
        assert!(
            !run.verified,
            "flipped route_scope must fail HMAC verification"
        );
    }

    #[tokio::test]
    async fn hmac_tamper_of_capability_fingerprint_makes_verified_false() {
        // `capability_filter_fingerprint` is part of the HMAC-signed tuple
        // (defends the V1 live-authorization resume gate): a raw-file edit that
        // rewrites the recorded fingerprint must fail verification so a resume
        // cannot be authorized against a forged capability set.
        let (store, dir) = temp_store().await;
        store.begin(sample_new_run("exec-fp")).await.expect("begin");
        let path = dir.path().join("codemode_pauses.db");
        let conn = Connection::open(&path).expect("raw open");
        conn.execute(
            "UPDATE codemode_runs SET capability_filter_fingerprint = 'forged' \
             WHERE execution_id = 'exec-fp'",
            [],
        )
        .expect("tamper");
        drop(conn);
        let run = store
            .load_run("exec-fp")
            .await
            .expect("load")
            .expect("some");
        assert!(
            !run.verified,
            "flipped capability_filter_fingerprint must fail HMAC verification"
        );
    }

    #[tokio::test]
    async fn upsert_redacts_args_and_no_raw_secret_on_disk() {
        let (store, dir) = temp_store().await;
        store
            .begin(sample_new_run("exec-red"))
            .await
            .expect("begin");
        store
            .upsert_log_entry(
                "exec-red",
                NewLogEntry {
                    seq: 0,
                    tool_id: "svc::do".to_string(),
                    raw_args: serde_json::json!({"api_key": "SUPERSECRET123", "n": 1}),
                    requires_approval: false,
                    ephemeral: false,
                },
            )
            .await
            .expect("upsert");

        // On-disk byte scan: raw secret must not appear anywhere in the SQLite
        // family (main db + WAL + shm); the `[redacted]` marker must.
        let base = dir.path().join("codemode_pauses.db");
        let mut haystack = String::new();
        for suffix in ["", "-wal", "-shm"] {
            let p = if suffix.is_empty() {
                base.clone()
            } else {
                PathBuf::from(format!("{}{suffix}", base.display()))
            };
            if let Ok(bytes) = std::fs::read(&p) {
                haystack.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        assert!(
            !haystack.contains("SUPERSECRET123"),
            "raw secret must never be on disk"
        );
        assert!(
            haystack.contains("[redacted]"),
            "redaction marker must be present on disk"
        );

        // The entry is loadable and args_hash is over the RAW value.
        let entry = store
            .get_log_entry("exec-red", 0)
            .await
            .expect("get")
            .expect("some");
        assert_eq!(entry.state, LogState::Executing);
        assert_eq!(
            entry.args_hash,
            CodeModePauseStore::canonical_args_hash(
                &serde_json::json!({"api_key": "SUPERSECRET123", "n": 1})
            ),
            "hash must be over RAW args, not redacted"
        );
    }

    #[tokio::test]
    async fn upsert_oversize_args_returns_value_too_large() {
        let (store, _dir) = temp_store().await;
        store
            .begin(sample_new_run("exec-big"))
            .await
            .expect("begin");
        let big = "x".repeat(MAX_DURABLE_VALUE_BYTES + 10);
        let err = store
            .upsert_log_entry(
                "exec-big",
                NewLogEntry {
                    seq: 0,
                    tool_id: "svc::do".to_string(),
                    raw_args: serde_json::json!({"blob": big}),
                    requires_approval: false,
                    ephemeral: false,
                },
            )
            .await
            .expect_err("must reject oversize");
        assert!(matches!(err, CodeModePauseStoreError::ValueTooLarge(_)));
    }

    #[tokio::test]
    async fn record_entry_result_applies_and_replays_result() {
        let (store, _dir) = temp_store().await;
        store.begin(sample_new_run("exec-rr")).await.expect("begin");
        store
            .upsert_log_entry(
                "exec-rr",
                NewLogEntry {
                    seq: 0,
                    tool_id: "svc::read".to_string(),
                    raw_args: serde_json::json!({"q": "x"}),
                    requires_approval: false,
                    ephemeral: false,
                },
            )
            .await
            .expect("upsert");
        store
            .record_entry_result("exec-rr", 0, Some(serde_json::json!({"ok": true})))
            .await
            .expect("record");
        let entry = store
            .get_log_entry("exec-rr", 0)
            .await
            .expect("get")
            .expect("some");
        assert_eq!(entry.state, LogState::Applied);
        assert_eq!(entry.redacted_result, Some(serde_json::json!({"ok": true})));
    }

    #[tokio::test]
    async fn ephemeral_result_is_not_stored() {
        let (store, _dir) = temp_store().await;
        store
            .begin(sample_new_run("exec-eph"))
            .await
            .expect("begin");
        store
            .upsert_log_entry(
                "exec-eph",
                NewLogEntry {
                    seq: 0,
                    tool_id: "svc::read".to_string(),
                    raw_args: serde_json::json!({"q": "x"}),
                    requires_approval: false,
                    ephemeral: true,
                },
            )
            .await
            .expect("upsert");
        store
            .record_entry_result("exec-eph", 0, Some(serde_json::json!({"ok": true})))
            .await
            .expect("record");
        let entry = store
            .get_log_entry("exec-eph", 0)
            .await
            .expect("get")
            .expect("some");
        assert_eq!(entry.state, LogState::Applied);
        assert_eq!(entry.redacted_result, None, "ephemeral stores no result");
    }

    #[tokio::test]
    async fn list_pending_only_for_paused_runs() {
        let (store, _dir) = temp_store().await;
        store.begin(sample_new_run("exec-lp")).await.expect("begin");
        store
            .upsert_log_entry(
                "exec-lp",
                NewLogEntry {
                    seq: 0,
                    tool_id: "svc::delete".to_string(),
                    raw_args: serde_json::json!({}),
                    requires_approval: true,
                    ephemeral: false,
                },
            )
            .await
            .expect("upsert");
        // Running run → nothing pending surfaced.
        assert!(store.list_pending("exec-lp").await.expect("lp").is_empty());
        store
            .set_status("exec-lp", RunStatus::Paused, None)
            .await
            .expect("pause");
        let pending = store.list_pending("exec-lp").await.expect("lp2");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tool_id, "svc::delete");
    }

    #[tokio::test]
    async fn expire_paused_flips_only_stale_rows() {
        let (store, _dir) = temp_store().await;
        // A stale paused run.
        store
            .begin(sample_new_run("exec-old"))
            .await
            .expect("begin");
        store
            .set_status("exec-old", RunStatus::Paused, None)
            .await
            .expect("pause");
        // A fresh paused run.
        store
            .begin(sample_new_run("exec-new"))
            .await
            .expect("begin");
        store
            .set_status("exec-new", RunStatus::Paused, None)
            .await
            .expect("pause");

        // Cutoff between them: expire rows updated before `now + 1` (which is
        // effectively "everything so far"); use a cutoff far in the past to
        // expire nothing, then a cutoff in the future to expire both.
        let none = store.expire_paused(0).await.expect("expire none");
        assert_eq!(none, 0, "nothing older than epoch");

        let future = now_ms() + 1_000_000;
        let both = store.expire_paused(future).await.expect("expire both");
        assert_eq!(both, 2, "both stale paused runs expire → rejected");
        let old = store
            .load_run("exec-old")
            .await
            .expect("load")
            .expect("some");
        assert_eq!(old.status, RunStatus::Rejected);
        assert!(old.verified, "expired row re-signed and still verifies");
    }
}
