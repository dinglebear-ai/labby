//! `SqliteDecider`: the durable-execution decision layer over
//! [`CodeModePauseStore`].
//!
//! Ports `decide()`/`recordResult()` from Cloudflare's `CodemodeRuntime`
//! (`packages/codemode/src/runtime.ts:411-572`) onto Labby's SQLite pause store.
//! Implements the storage-neutral `labby_codemode::CodeModeDecider` trait so the
//! gateway host can consult it via `Arc<dyn CodeModeDecider>` without depending
//! on SQLite.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use labby_codemode::{CodeModeDecider, DecideOutcome};
use labby_runtime::error::ToolError;
use serde_json::Value;

use super::sqlite_pauses::{
    CodeModePauseStore, CodeModePauseStoreError, LogState, NewLogEntry, RunStatus,
};

/// Default age after which a paused (awaiting-approval) run can be expired
/// (24h, matching Cloudflare's `DEFAULT_PAUSED_TTL_MS`; `runtime.ts:156`).
const DEFAULT_PAUSED_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// Minimum interval between lazy expiry sweeps (throttle; `Wave 4 Task 4.2`).
const EXPIRY_SWEEP_INTERVAL_MS: i64 = 60_000;

/// SQLite-backed durable-execution decider.
#[derive(Clone)]
pub struct SqliteDecider {
    store: CodeModePauseStore,
    /// Last lazy-expiry sweep timestamp (ms). Throttles `maybe_expire`.
    last_sweep_ms: Arc<AtomicI64>,
}

impl SqliteDecider {
    #[must_use]
    pub fn new(store: CodeModePauseStore) -> Self {
        Self {
            store,
            last_sweep_ms: Arc::new(AtomicI64::new(0)),
        }
    }

    /// Access the underlying store (used by the MCP surface for begin / status
    /// reads / resume CAS / reject).
    #[must_use]
    pub fn store(&self) -> &CodeModePauseStore {
        &self.store
    }

    /// Configured pause TTL in ms (`LAB_CODE_MODE_PAUSE_TTL_MS`, default 24h).
    #[must_use]
    pub fn pause_ttl_ms() -> i64 {
        std::env::var("LAB_CODE_MODE_PAUSE_TTL_MS")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_PAUSED_TTL_MS)
    }

    /// Lazy, throttled TTL sweep (`Wave 4 Task 4.2`). No-op unless
    /// `EXPIRY_SWEEP_INTERVAL_MS` has elapsed since the last sweep. Does only
    /// SQLite work (no runner pool, no subprocess).
    pub async fn maybe_expire(&self) {
        let now = now_ms();
        let last = self.last_sweep_ms.load(Ordering::Relaxed);
        if now - last < EXPIRY_SWEEP_INTERVAL_MS {
            return;
        }
        // Claim the sweep slot (best-effort; a lost race just means another
        // caller sweeps instead).
        if self
            .last_sweep_ms
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let cutoff = now - Self::pause_ttl_ms();
        match self.store.expire_paused(cutoff).await {
            Ok(n) if n > 0 => {
                tracing::info!(
                    surface = "mcp",
                    service = "codemode",
                    action = "pause.expire",
                    expired = n,
                    "expired stale paused/running Code Mode runs"
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    surface = "mcp",
                    service = "codemode",
                    action = "pause.expire",
                    kind = "internal_error",
                    error = %err,
                    "lazy Code Mode pause expiry sweep failed"
                );
            }
        }
    }
}

impl CodeModeDecider for SqliteDecider {
    fn decide<'a>(
        &'a self,
        execution_id: &'a str,
        seq: u64,
        tool_id: &'a str,
        args: &'a Value,
        requires_approval: bool,
        ephemeral: bool,
    ) -> labby_codemode::host::BoxDecideFuture<'a, DecideOutcome> {
        Box::pin(async move {
            self.decide_inner(execution_id, seq, tool_id, args, requires_approval, ephemeral)
                .await
        })
    }

    fn record_result<'a>(
        &'a self,
        execution_id: &'a str,
        seq: u64,
        result: &'a Value,
    ) -> labby_codemode::host::BoxDecideFuture<'a, Result<(), ToolError>> {
        Box::pin(async move { self.record_result_inner(execution_id, seq, result).await })
    }
}

impl SqliteDecider {
    async fn decide_inner(
        &self,
        execution_id: &str,
        seq: u64,
        tool_id: &str,
        args: &Value,
        requires_approval: bool,
        ephemeral: bool,
    ) -> DecideOutcome {
        let seq_i = seq as i64;

        // Load the run; unknown / tampered → fail closed (V6).
        let run = match self.store.load_run(execution_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return DecideOutcome::Fail("unknown execution".into()),
            Err(err) => return DecideOutcome::Fail(err.to_string()),
        };
        if !run.verified {
            return DecideOutcome::Fail("integrity check failed".into());
        }

        // C1 monotonic gate: once a run is not `running` (paused or terminal),
        // every subsequent call gets a pause decision and nothing is recorded —
        // so model code that swallows the pause sentinel can drive no further
        // side effects (runtime.ts:428).
        if run.status != RunStatus::Running {
            return DecideOutcome::Pause;
        }

        // Replay path: an existing log entry at this seq.
        match self.store.get_log_entry(execution_id, seq_i).await {
            Ok(Some(existing)) => {
                // Divergence: tool_id + canonical args hash must match
                // (runtime.ts:436-455). A mismatch is a hard, model-actionable
                // error — never a silent stale-result application.
                let after = CodeModePauseStore::canonical_args_hash(args);
                if existing.tool_id != tool_id || existing.args_hash != after {
                    let msg = format!(
                        "Codemode replay divergence at step {seq}: expected {}, got {tool_id}. \
                         Code must be deterministic up to tool calls and steps. Wrap \
                         nondeterministic work in codemode.step(name, fn).",
                        existing.tool_id
                    );
                    if let Err(err) = self
                        .store
                        .set_status(execution_id, RunStatus::Error, Some(&msg))
                        .await
                    {
                        return DecideOutcome::Fail(err.to_string());
                    }
                    return DecideOutcome::Diverge(msg);
                }
                match existing.state {
                    // Ephemeral applied entries re-execute on replay
                    // (runtime.ts:461).
                    LogState::Applied if existing.ephemeral => DecideOutcome::Execute,
                    // Non-ephemeral applied → replay cached result
                    // (runtime.ts:464). C4: record never truncates, so an
                    // Applied non-ephemeral entry always has an intact result;
                    // a NULL here is a corruption signal → fail closed.
                    LogState::Applied => match existing.redacted_result {
                        Some(value) => DecideOutcome::Replay(value),
                        None => DecideOutcome::Diverge(format!(
                            "Codemode replay divergence at step {seq}: applied entry has no \
                             recorded result to replay."
                        )),
                    },
                    // Approved since the last run: transition pending →
                    // executing and execute (runtime.ts:473).
                    LogState::Pending => {
                        if let Err(err) = self
                            .store
                            .set_entry_state(execution_id, seq_i, LogState::Executing)
                            .await
                        {
                            return DecideOutcome::Fail(err.to_string());
                        }
                        DecideOutcome::Execute
                    }
                    // Crashed mid-call on a prior pass — re-execute
                    // (runtime.ts:481).
                    LogState::Executing => DecideOutcome::Execute,
                    // Reverted → fall through and re-journal as a fresh call.
                    LogState::Reverted => {
                        self.journal_fresh(
                            execution_id,
                            seq_i,
                            tool_id,
                            args,
                            requires_approval,
                            ephemeral,
                        )
                        .await
                    }
                }
            }
            Ok(None) => {
                self.journal_fresh(
                    execution_id,
                    seq_i,
                    tool_id,
                    args,
                    requires_approval,
                    ephemeral,
                )
                .await
            }
            Err(err) => DecideOutcome::Fail(err.to_string()),
        }
    }

    async fn record_result_inner(
        &self,
        execution_id: &str,
        seq: u64,
        result: &Value,
    ) -> Result<(), ToolError> {
        // Port of runtime.ts:543 recordResult. Oversize/unserializable result
        // fails the run (no truncation) rather than corrupting replay.
        match self
            .store
            .record_entry_result(execution_id, seq as i64, Some(result.clone()))
            .await
        {
            Ok(()) => Ok(()),
            Err(CodeModePauseStoreError::ValueTooLarge(msg)) => {
                // Record on the execution (terminal), then surface the message.
                self.store
                    .set_status(execution_id, RunStatus::Error, Some(&msg))
                    .await
                    .ok();
                Err(ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: msg,
                })
            }
            Err(err) => Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: err.to_string(),
            }),
        }
    }
}

impl SqliteDecider {
    /// Journal a fresh call and decide execute-vs-pause (runtime.ts:486-527).
    /// Oversize/unserializable args fail the run terminally (no truncation).
    async fn journal_fresh(
        &self,
        execution_id: &str,
        seq: i64,
        tool_id: &str,
        args: &Value,
        requires_approval: bool,
        ephemeral: bool,
    ) -> DecideOutcome {
        match self
            .store
            .upsert_log_entry(
                execution_id,
                NewLogEntry {
                    seq,
                    tool_id: tool_id.to_string(),
                    raw_args: args.clone(),
                    requires_approval,
                    ephemeral,
                },
            )
            .await
        {
            Ok(()) => {}
            Err(CodeModePauseStoreError::ValueTooLarge(msg)) => {
                self.store
                    .set_status(execution_id, RunStatus::Error, Some(&msg))
                    .await
                    .ok();
                return DecideOutcome::Fail(msg);
            }
            Err(err) => return DecideOutcome::Fail(err.to_string()),
        }

        if requires_approval {
            // Flip durable status → paused (runtime.ts:520). The store already
            // journaled the entry as `pending`.
            if let Err(err) = self
                .store
                .set_status(execution_id, RunStatus::Paused, None)
                .await
            {
                return DecideOutcome::Fail(err.to_string());
            }
            return DecideOutcome::Pause;
        }
        DecideOutcome::Execute
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_decider() -> (SqliteDecider, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("codemode_pauses.db");
        let store = CodeModePauseStore::open(path).await.expect("open");
        (SqliteDecider::new(store), dir)
    }

    async fn begin_run(decider: &SqliteDecider, id: &str) {
        decider
            .store
            .begin(super::super::sqlite_pauses::NewRun {
                execution_id: id.to_string(),
                code_hash: "hash".to_string(),
                actor_key: Some("actor".to_string()),
                is_admin: true,
                route_scope: "unscoped".to_string(),
                capability_filter_fingerprint: "fp".to_string(),
                expires_at_ms: now_ms() + 86_400_000,
            })
            .await
            .expect("begin");
    }

    #[tokio::test]
    async fn fresh_nondestructive_call_executes_and_logs() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e1").await;
        let out = d
            .decide("e1", 0, "svc::read", &serde_json::json!({"q": 1}), false, false)
            .await;
        assert!(matches!(out, DecideOutcome::Execute));
        let entry = d.store.get_log_entry("e1", 0).await.unwrap().unwrap();
        assert_eq!(entry.state, LogState::Executing);
    }

    #[tokio::test]
    async fn fresh_destructive_unconfirmed_pauses_run() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e2").await;
        let out = d
            .decide("e2", 0, "svc::delete", &serde_json::json!({}), true, false)
            .await;
        assert!(matches!(out, DecideOutcome::Pause));
        let run = d.store.load_run("e2").await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Paused);
        let entry = d.store.get_log_entry("e2", 0).await.unwrap().unwrap();
        assert_eq!(entry.state, LogState::Pending);
    }

    #[tokio::test]
    async fn second_call_after_pause_is_monotonic_pause_and_logs_nothing() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e3").await;
        // First destructive call pauses.
        let _ = d
            .decide("e3", 0, "svc::delete", &serde_json::json!({}), true, false)
            .await;
        // A later call at seq 1 gets Pause and journals nothing.
        let out = d
            .decide("e3", 1, "svc::read", &serde_json::json!({"x": 2}), false, false)
            .await;
        assert!(matches!(out, DecideOutcome::Pause));
        assert!(d.store.get_log_entry("e3", 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn replay_of_applied_entry_returns_cached_result() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e4").await;
        let args = serde_json::json!({"q": "x"});
        let _ = d.decide("e4", 0, "svc::read", &args, false, false).await;
        d.record_result("e4", 0, &serde_json::json!({"ok": true}))
            .await
            .unwrap();
        // Re-decide same seq with matching args → replay cached, no dispatch.
        let out = d.decide("e4", 0, "svc::read", &args, false, false).await;
        match out {
            DecideOutcome::Replay(v) => assert_eq!(v, serde_json::json!({"ok": true})),
            other => panic!("expected Replay, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mismatched_args_at_seq_diverges_and_errors_run() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e5").await;
        let _ = d
            .decide("e5", 0, "svc::read", &serde_json::json!({"q": "x"}), false, false)
            .await;
        d.record_result("e5", 0, &serde_json::json!({"ok": 1}))
            .await
            .unwrap();
        // Different args at the same seq → hard divergence.
        let out = d
            .decide("e5", 0, "svc::read", &serde_json::json!({"q": "DIFFERENT"}), false, false)
            .await;
        assert!(matches!(out, DecideOutcome::Diverge(_)));
        let run = d.store.load_run("e5").await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Error);
    }

    #[tokio::test]
    async fn oversize_args_fails_run() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e6").await;
        let big = "z".repeat(super::super::sqlite_pauses::MAX_DURABLE_VALUE_BYTES + 10);
        let out = d
            .decide("e6", 0, "svc::do", &serde_json::json!({"blob": big}), false, false)
            .await;
        assert!(matches!(out, DecideOutcome::Fail(_)));
        let run = d.store.load_run("e6").await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Error);
    }

    #[tokio::test]
    async fn unknown_execution_fails() {
        let (d, _dir) = fresh_decider().await;
        let out = d
            .decide("nope", 0, "svc::read", &serde_json::json!({}), false, false)
            .await;
        assert!(matches!(out, DecideOutcome::Fail(_)));
    }
}
