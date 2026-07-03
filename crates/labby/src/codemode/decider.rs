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

use labby_codemode::host::BoxDecideFuture;
use labby_codemode::{
    AuthLoad, BeginRun, CodeModeDecider, DecideOutcome, PendingCall, RunLifecycle, VerifiedAuth,
};
use labby_runtime::error::ToolError;
use serde_json::Value;

use super::sqlite_pauses::{
    CodeModePauseStore, CodeModePauseStoreError, LogState, NewLogEntry, NewRun, RunStatus,
};

/// Map the neutral `RunLifecycle` (host-facing) onto the store's `RunStatus`.
fn lifecycle_to_status(l: RunLifecycle) -> Option<RunStatus> {
    match l {
        RunLifecycle::Running => Some(RunStatus::Running),
        RunLifecycle::Paused => Some(RunStatus::Paused),
        RunLifecycle::Completed => Some(RunStatus::Completed),
        RunLifecycle::Error => Some(RunStatus::Error),
        RunLifecycle::Rejected => Some(RunStatus::Rejected),
        RunLifecycle::Expired => Some(RunStatus::Expired),
        RunLifecycle::Unknown => None,
    }
}

/// Map the store's `RunStatus` onto the neutral `RunLifecycle`.
fn status_to_lifecycle(s: RunStatus) -> RunLifecycle {
    match s {
        RunStatus::Running => RunLifecycle::Running,
        RunStatus::Paused => RunLifecycle::Paused,
        RunStatus::Completed => RunLifecycle::Completed,
        RunStatus::Error => RunLifecycle::Error,
        RunStatus::Rejected => RunLifecycle::Rejected,
        RunStatus::Expired => RunLifecycle::Expired,
    }
}

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

    /// Access the underlying store. Used by the MCP surface's resume/reject
    /// paths (Wave 3); retained here as the concrete-decider escape hatch.
    #[must_use]
    #[allow(dead_code)]
    pub fn store(&self) -> &CodeModePauseStore {
        &self.store
    }

    /// Configured pause TTL in ms (`LAB_CODE_MODE_PAUSE_TTL_MS`, default 24h).
    #[must_use]
    pub fn pause_ttl_ms() -> i64 {
        super::pause_ttl_ms()
    }

    /// Lazy, throttled TTL sweep (`Wave 4 Task 4.2`). No-op unless
    /// `EXPIRY_SWEEP_INTERVAL_MS` has elapsed since the last sweep. Does only
    /// SQLite work (no runner pool, no subprocess). Exposed via the trait's
    /// `maybe_expire`.
    async fn maybe_expire_impl(&self) {
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
    ) -> BoxDecideFuture<'a, DecideOutcome> {
        Box::pin(async move {
            self.decide_inner(
                execution_id,
                seq,
                tool_id,
                args,
                requires_approval,
                ephemeral,
            )
            .await
        })
    }

    fn record_result<'a>(
        &'a self,
        execution_id: &'a str,
        seq: u64,
        result: &'a Value,
    ) -> BoxDecideFuture<'a, Result<(), ToolError>> {
        Box::pin(async move { self.record_result_inner(execution_id, seq, result).await })
    }

    fn begin(&self, run: BeginRun) -> BoxDecideFuture<'_, Result<(), ToolError>> {
        Box::pin(async move {
            self.store
                .begin(NewRun {
                    execution_id: run.execution_id,
                    code_hash: run.code_hash,
                    actor_key: run.actor_key,
                    is_admin: run.is_admin,
                    route_scope: run.route_scope,
                    capability_filter_fingerprint: run.capability_filter_fingerprint,
                    expires_at_ms: run.expires_at_ms,
                })
                .await
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: e.to_string(),
                })
        })
    }

    fn run_status<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, RunLifecycle> {
        Box::pin(async move {
            match self.store.load_run(execution_id).await {
                // A tampered row (verified == false) fails closed as Unknown.
                Ok(Some(run)) if run.verified => status_to_lifecycle(run.status),
                _ => RunLifecycle::Unknown,
            }
        })
    }

    fn run_error<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, Option<String>> {
        Box::pin(async move { self.store.run_error(execution_id).await.ok().flatten() })
    }

    fn list_pending<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, Vec<PendingCall>> {
        Box::pin(async move {
            self.store
                .list_pending(execution_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|e| PendingCall {
                    seq: e.seq as u64,
                    tool_id: e.tool_id,
                })
                .collect()
        })
    }

    fn set_status<'a>(
        &'a self,
        execution_id: &'a str,
        to: RunLifecycle,
        error: Option<&'a str>,
    ) -> BoxDecideFuture<'a, Result<bool, ToolError>> {
        Box::pin(async move {
            let Some(status) = lifecycle_to_status(to) else {
                return Err(ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: "cannot set a run to Unknown status".to_string(),
                });
            };
            self.store
                .set_status(execution_id, status, error)
                .await
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: e.to_string(),
                })
        })
    }

    fn resume_to_running<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, bool> {
        Box::pin(async move {
            self.store
                .resume_to_running(execution_id)
                .await
                .unwrap_or(false)
        })
    }

    fn reject_paused<'a>(
        &'a self,
        execution_id: &'a str,
        error: Option<&'a str>,
    ) -> BoxDecideFuture<'a, Result<bool, ToolError>> {
        Box::pin(async move {
            self.store
                .reject_paused(execution_id, error)
                .await
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: e.to_string(),
                })
        })
    }

    fn run_auth_fields<'a>(&'a self, execution_id: &'a str) -> BoxDecideFuture<'a, AuthLoad> {
        Box::pin(async move {
            let run = match self.store.load_run(execution_id).await {
                // A load error (SQLite/join) is treated as fail-closed: report
                // the run as missing so no gate reads auth off nothing.
                Err(_) | Ok(None) => return AuthLoad::Missing,
                Ok(Some(run)) => run,
            };
            // The auth-bearing tuple is only reachable through `verified_auth()`,
            // which is `None` for a tampered row — so a tampered row can NEVER
            // become a `VerifiedAuth`; it is surfaced as its own variant.
            let Some(auth) = run.verified_auth() else {
                return AuthLoad::Tampered;
            };
            AuthLoad::Ok(VerifiedAuth {
                code_hash: run.code_hash.clone(),
                actor_key: auth.actor_key.map(ToOwned::to_owned),
                is_admin: auth.is_admin,
                capability_filter_fingerprint: auth.capability_filter_fingerprint.to_string(),
                route_scope: auth.route_scope.to_string(),
                status: status_to_lifecycle(run.status),
            })
        })
    }

    fn maybe_expire(&self) -> BoxDecideFuture<'_, ()> {
        Box::pin(async move { self.maybe_expire_impl().await })
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
                    // A corrupt/unrecognized on-disk state (`LogState::Unknown`)
                    // fails closed: refuse to replay a run with a corrupt log
                    // entry rather than laundering it into `Reverted` and
                    // re-dispatching the call. Mirrors the run-status path, where
                    // a tampered row reads back `verified == false` and the run
                    // reads as `RunLifecycle::Unknown` (host.rs).
                    LogState::Unknown => {
                        DecideOutcome::Diverge("corrupt log entry state".to_string())
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
                if let Err(status_err) = self
                    .store
                    .set_status(execution_id, RunStatus::Error, Some(&msg))
                    .await
                {
                    // F6: correctness is fine (we still return Err below); log the
                    // swallowed status-write failure so it is observable.
                    tracing::warn!(
                        surface = "mcp",
                        service = "codemode",
                        action = "record_result",
                        kind = "internal_error",
                        execution_id,
                        error = %status_err,
                        "failed to mark Code Mode run errored after oversize result"
                    );
                }
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
                if let Err(status_err) = self
                    .store
                    .set_status(execution_id, RunStatus::Error, Some(&msg))
                    .await
                {
                    // F6: we still return Fail below (correctness is fine); log
                    // the swallowed status-write failure so it is observable.
                    tracing::warn!(
                        surface = "mcp",
                        service = "codemode",
                        action = "decide",
                        kind = "internal_error",
                        execution_id,
                        error = %status_err,
                        "failed to mark Code Mode run errored after oversize args"
                    );
                }
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

use super::now_ms;

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
        begin_run_scoped(decider, id, "unscoped").await;
    }

    async fn begin_run_scoped(decider: &SqliteDecider, id: &str, route_scope: &str) {
        decider
            .store
            .begin(NewRun {
                execution_id: id.to_string(),
                code_hash: "hash".to_string(),
                actor_key: Some("actor".to_string()),
                is_admin: true,
                route_scope: route_scope.to_string(),
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
            .decide(
                "e1",
                0,
                "svc::read",
                &serde_json::json!({"q": 1}),
                false,
                false,
            )
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
        let _outcome = d
            .decide("e3", 0, "svc::delete", &serde_json::json!({}), true, false)
            .await;
        // A later call at seq 1 gets Pause and journals nothing.
        let out = d
            .decide(
                "e3",
                1,
                "svc::read",
                &serde_json::json!({"x": 2}),
                false,
                false,
            )
            .await;
        assert!(matches!(out, DecideOutcome::Pause));
        assert!(d.store.get_log_entry("e3", 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn replay_of_applied_entry_returns_cached_result() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e4").await;
        let args = serde_json::json!({"q": "x"});
        let _outcome = d.decide("e4", 0, "svc::read", &args, false, false).await;
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
        let _outcome = d
            .decide(
                "e5",
                0,
                "svc::read",
                &serde_json::json!({"q": "x"}),
                false,
                false,
            )
            .await;
        d.record_result("e5", 0, &serde_json::json!({"ok": 1}))
            .await
            .unwrap();
        // Different args at the same seq → hard divergence.
        let out = d
            .decide(
                "e5",
                0,
                "svc::read",
                &serde_json::json!({"q": "DIFFERENT"}),
                false,
                false,
            )
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
            .decide(
                "e6",
                0,
                "svc::do",
                &serde_json::json!({"blob": big}),
                false,
                false,
            )
            .await;
        assert!(matches!(out, DecideOutcome::Fail(_)));
        let run = d.store.load_run("e6").await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Error);
    }

    #[tokio::test]
    async fn record_result_failure_leaves_entry_unapplied_and_errors_run() {
        // F1: if record_result fails (here: oversize result), the log entry must
        // NOT be marked `applied` (else a resume would replay a NULL result), and
        // the run must flip to Error so it fails closed rather than allowing a
        // double-dispatch on resume.
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e-rec").await;
        // Fresh non-destructive call → journals + executes (state = executing).
        let out = d
            .decide(
                "e-rec",
                0,
                "svc::read",
                &serde_json::json!({"q": 1}),
                false,
                false,
            )
            .await;
        assert!(matches!(out, DecideOutcome::Execute));
        // Record an oversize result → record_result returns Err.
        let big = "z".repeat(super::super::sqlite_pauses::MAX_DURABLE_VALUE_BYTES + 10);
        let err = d
            .record_result("e-rec", 0, &serde_json::json!({"blob": big}))
            .await
            .expect_err("oversize result must fail record_result");
        assert!(matches!(err, ToolError::Sdk { .. }));
        // Entry stays not-applied (still `executing`) — no NULL result to replay.
        let entry = d.store.get_log_entry("e-rec", 0).await.unwrap().unwrap();
        assert_eq!(
            entry.state,
            LogState::Executing,
            "failed record must not mark the entry applied"
        );
        assert!(
            entry.redacted_result.is_none(),
            "failed record must not persist a result"
        );
        // Run flipped to Error (fails closed).
        let run = d.store.load_run("e-rec").await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Error);
    }

    #[tokio::test]
    async fn step_journals_non_ephemerally_and_replays_recorded_value() {
        // A `codemode.step` boundary journals non-ephemerally (ephemeral=false):
        // the recorded value REPLAYS on a resume so `fn` is never re-run. This is
        // the durable half of what `handle_step_begin_event` + the gateway
        // `decide_step` hook rely on.
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e-step").await;
        let args = serde_json::json!({ "name": "s" });
        // First pass: fresh non-destructive step → Execute (fn runs).
        let out = d
            .decide("e-step", 0, "codemode::step", &args, false, false)
            .await;
        assert!(matches!(out, DecideOutcome::Execute));
        // Record fn's produced value (the nondeterministic result).
        d.record_result("e-step", 0, &serde_json::json!({ "rand": 0.42 }))
            .await
            .unwrap();
        // Resume: same seq + same step name → replay the cached value, NOT execute.
        let out = d
            .decide("e-step", 0, "codemode::step", &args, false, false)
            .await;
        match out {
            DecideOutcome::Replay(v) => assert_eq!(v, serde_json::json!({ "rand": 0.42 })),
            other => panic!("step replay must return the cached value, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wrapping_nondeterminism_in_step_resumes_cleanly_unwrapped_diverges() {
        // CONTRAST: the same nondeterministic value behaves differently depending
        // on whether it is wrapped in codemode.step.
        //
        // WRAPPED — the step's divergence key is its NAME (`{"name":"s"}`), not
        // fn's output. On resume the name is identical, so decide() replays the
        // recorded value cleanly even though fn would have produced something
        // different this pass.
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "wrap").await;
        let step_args = serde_json::json!({ "name": "s" });
        let _first = d
            .decide("wrap", 0, "codemode::step", &step_args, false, false)
            .await;
        // fn produced 0.42 on the first pass; that value is journaled.
        d.record_result("wrap", 0, &serde_json::json!(0.42))
            .await
            .unwrap();
        let out = d
            .decide("wrap", 0, "codemode::step", &step_args, false, false)
            .await;
        assert!(
            matches!(&out, DecideOutcome::Replay(v) if *v == serde_json::json!(0.42)),
            "wrapped nondeterminism replays cleanly, got {out:?}"
        );

        // UNWRAPPED — the same nondeterministic value passed as a raw tool-call
        // ARG is the divergence key. A drift from 0.42 → 0.43 at the same seq is a
        // hard resume_divergence (fail closed), never a silent stale-result apply.
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "raw").await;
        let _first = d
            .decide(
                "raw",
                0,
                "svc::use",
                &serde_json::json!({ "rand": 0.42 }),
                false,
                false,
            )
            .await;
        d.record_result("raw", 0, &serde_json::json!({ "ok": true }))
            .await
            .unwrap();
        let out = d
            .decide(
                "raw",
                0,
                "svc::use",
                &serde_json::json!({ "rand": 0.43 }),
                false,
                false,
            )
            .await;
        assert!(
            matches!(out, DecideOutcome::Diverge(_)),
            "unwrapped nondeterminism must diverge, got {out:?}"
        );
    }

    #[tokio::test]
    async fn ephemeral_local_provider_reexecutes_on_replay() {
        // Local `state`/`git` providers journal EPHEMERALLY (ephemeral=true): the
        // entry marks applied but a resume RE-EXECUTES it rather than replaying a
        // stored result — so the FS/git side effect re-runs, never double-applied
        // silently nor served from stale cache. This is the durable half of the
        // gateway `decide_local` hook + `enqueue_local_provider_call`.
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e-local").await;
        let args = serde_json::json!({ "path": "a.txt" });
        // First pass: fresh ephemeral call → Execute.
        let out = d
            .decide("e-local", 0, "state::writeFile", &args, false, true)
            .await;
        assert!(matches!(out, DecideOutcome::Execute));
        d.record_result("e-local", 0, &serde_json::json!({ "ok": true }))
            .await
            .unwrap();
        // Resume: same seq + same id/args → Execute AGAIN (re-run), not Replay.
        let out = d
            .decide("e-local", 0, "state::writeFile", &args, false, true)
            .await;
        assert!(
            matches!(out, DecideOutcome::Execute),
            "an applied ephemeral entry must re-execute on replay, got {out:?}"
        );
    }

    #[tokio::test]
    async fn unknown_execution_fails() {
        let (d, _dir) = fresh_decider().await;
        let out = d
            .decide("nope", 0, "svc::read", &serde_json::json!({}), false, false)
            .await;
        assert!(matches!(out, DecideOutcome::Fail(_)));
    }

    // ── Wave 3 lifecycle (resume/reject) trait methods ──

    #[tokio::test]
    async fn run_auth_fields_returns_recorded_authz() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e7").await;
        let fields = match d.run_auth_fields("e7").await {
            AuthLoad::Ok(fields) => fields,
            other => panic!("a fresh verified run must load Ok, got {other:?}"),
        };
        assert_eq!(fields.code_hash, "hash");
        assert_eq!(fields.actor_key.as_deref(), Some("actor"));
        assert!(fields.is_admin);
        assert_eq!(fields.capability_filter_fingerprint, "fp");
        assert_eq!(fields.route_scope, "unscoped");
        assert_eq!(fields.status, RunLifecycle::Running);
    }

    #[tokio::test]
    async fn run_auth_fields_missing_run_is_missing() {
        let (d, _dir) = fresh_decider().await;
        assert!(matches!(d.run_auth_fields("nope").await, AuthLoad::Missing));
    }

    #[tokio::test]
    async fn run_auth_fields_tampered_row_is_tampered_not_ok() {
        // A raw-file edit that flips an auth-bearing field breaks the HMAC, so the
        // row can NEVER be read as a usable VerifiedAuth — the load reports
        // `Tampered` (its own dead-end variant), and the resume/reject gates that
        // pattern-match `AuthLoad::Ok(..)` therefore refuse it structurally.
        let (d, dir) = fresh_decider().await;
        begin_run(&d, "e-tamper").await;
        let path = dir.path().join("codemode_pauses.db");
        let conn = rusqlite::Connection::open(&path).expect("raw open");
        conn.execute(
            "UPDATE codemode_runs SET is_admin = 0 WHERE execution_id = 'e-tamper'",
            [],
        )
        .expect("tamper");
        drop(conn);
        assert!(
            matches!(d.run_auth_fields("e-tamper").await, AuthLoad::Tampered),
            "a tampered row must load as Tampered, never Ok"
        );
    }

    #[tokio::test]
    async fn run_auth_fields_carries_the_recorded_route_scope() {
        // F2: the route scope a run began under must be faithfully threaded into
        // `RunAuthFields` so the resume handler can refuse a cross-route resume
        // (`auth_fields.route_scope != self.route_scope.label()`). Two runs begun
        // under different scopes must each report their own.
        let (d, _dir) = fresh_decider().await;
        begin_run_scoped(&d, "route-a", "protected:media").await;
        begin_run_scoped(&d, "route-b", "protected:ops").await;
        let a = match d.run_auth_fields("route-a").await {
            AuthLoad::Ok(f) => f,
            other => panic!("route-a must load Ok, got {other:?}"),
        };
        let b = match d.run_auth_fields("route-b").await {
            AuthLoad::Ok(f) => f,
            other => panic!("route-b must load Ok, got {other:?}"),
        };
        assert_eq!(a.route_scope, "protected:media");
        assert_eq!(b.route_scope, "protected:ops");
        // The refusal predicate the handler applies: a run paused under A is
        // refused when the live route scope is B.
        assert_ne!(
            a.route_scope, b.route_scope,
            "distinct route scopes must not compare equal — the cross-route resume guard"
        );
    }

    #[tokio::test]
    async fn resume_to_running_cas_wins_once() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e8").await;
        // Pause it (a destructive fresh call).
        let _outcome = d
            .decide("e8", 0, "svc::delete", &serde_json::json!({}), true, false)
            .await;
        assert_eq!(
            d.run_status("e8").await,
            RunLifecycle::Paused,
            "must be paused after a destructive call"
        );
        // First CAS wins; a concurrent second loses (already_resumed).
        assert!(d.resume_to_running("e8").await);
        assert!(!d.resume_to_running("e8").await);
        assert_eq!(d.run_status("e8").await, RunLifecycle::Running);
    }

    #[tokio::test]
    async fn maybe_expire_is_lazy_and_throttled() {
        let (d, _dir) = fresh_decider().await;
        // last_sweep starts at 0 → first call sweeps and stamps a recent time.
        assert_eq!(d.last_sweep_ms.load(Ordering::Relaxed), 0);
        d.maybe_expire().await;
        let after_first = d.last_sweep_ms.load(Ordering::Relaxed);
        assert!(after_first > 0, "first sweep must stamp last_sweep_ms");
        // An immediate second call is inside EXPIRY_SWEEP_INTERVAL_MS → throttled
        // (last_sweep_ms unchanged, no second sweep).
        d.maybe_expire().await;
        assert_eq!(
            d.last_sweep_ms.load(Ordering::Relaxed),
            after_first,
            "second call within the throttle window must not re-sweep"
        );
    }

    #[tokio::test]
    async fn maybe_expire_rejects_stale_paused_run() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e-exp").await;
        let _outcome = d
            .decide(
                "e-exp",
                0,
                "svc::delete",
                &serde_json::json!({}),
                true,
                false,
            )
            .await;
        assert_eq!(d.run_status("e-exp").await, RunLifecycle::Paused);
        // Force expiry directly on the store with a future cutoff (bypass the
        // throttle/TTL) to prove the sweep flips a stale paused run to rejected.
        let cutoff = now_ms() + 1_000_000;
        let n = d.store.expire_paused(cutoff).await.expect("expire");
        assert_eq!(n, 1);
        assert_eq!(d.run_status("e-exp").await, RunLifecycle::Rejected);
    }

    #[tokio::test]
    async fn reject_sets_rejected_only_when_paused() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e9").await;
        let _outcome = d
            .decide("e9", 0, "svc::delete", &serde_json::json!({}), true, false)
            .await;
        // Reject guarded on Paused → succeeds.
        assert!(
            d.set_status("e9", RunLifecycle::Rejected, Some("rejected by user"))
                .await
                .expect("set_status")
        );
        assert_eq!(d.run_status("e9").await, RunLifecycle::Rejected);
        // A resume CAS on a rejected run fails closed.
        assert!(!d.resume_to_running("e9").await);
    }

    // ── F3: reject_paused is guarded to Paused-only ──

    #[tokio::test]
    async fn reject_paused_terminates_only_a_paused_run() {
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e-rej").await;
        // Pause via a destructive call.
        let _outcome = d
            .decide(
                "e-rej",
                0,
                "svc::delete",
                &serde_json::json!({}),
                true,
                false,
            )
            .await;
        assert_eq!(d.run_status("e-rej").await, RunLifecycle::Paused);
        // reject_paused on a paused run → true + Rejected, pending entry reverted.
        assert!(
            d.reject_paused("e-rej", Some("rejected by user"))
                .await
                .expect("reject_paused")
        );
        assert_eq!(d.run_status("e-rej").await, RunLifecycle::Rejected);
        let entry = d.store.get_log_entry("e-rej", 0).await.unwrap().unwrap();
        assert_eq!(entry.state, LogState::Reverted);
    }

    #[tokio::test]
    async fn reject_paused_is_a_noop_on_a_running_run() {
        // The status guard is the security-critical property (F3): a token holder
        // must NOT be able to force-terminate a live Running run.
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e-run").await;
        assert_eq!(d.run_status("e-run").await, RunLifecycle::Running);
        // reject_paused on a running run → false, status unchanged.
        assert!(
            !d.reject_paused("e-run", Some("rejected by user"))
                .await
                .expect("reject_paused")
        );
        assert_eq!(
            d.run_status("e-run").await,
            RunLifecycle::Running,
            "reject_paused must not terminate a running run"
        );
    }

    #[tokio::test]
    async fn reject_paused_is_a_noop_on_an_unknown_run() {
        let (d, _dir) = fresh_decider().await;
        assert!(!d.reject_paused("nope", None).await.expect("reject_paused"));
    }

    // ── Fail-closed on a corrupt log-entry state ──

    #[tokio::test]
    async fn corrupt_log_entry_state_reads_back_unknown_and_diverges_on_decide() {
        // A corrupt/unknown on-disk `state` string must (a) read back as
        // `LogState::Unknown` rather than being laundered into a valid terminal
        // state, and (b) make `decide()` fail closed with `Diverge` on replay —
        // refusing to re-journal and re-dispatch a call whose recorded state is
        // corrupt. Mirrors the run-status path where a tampered row reads back
        // `verified == false`.
        let (d, dir) = fresh_decider().await;
        begin_run(&d, "e-corrupt").await;
        // Journal a fresh non-destructive call at seq 0 (state = executing),
        // then record its result so it becomes a normal `applied` entry.
        let args = serde_json::json!({"q": 1});
        let _outcome = d
            .decide("e-corrupt", 0, "svc::read", &args, false, false)
            .await;
        d.record_result("e-corrupt", 0, &serde_json::json!({"ok": true}))
            .await
            .expect("record");

        // Corrupt the entry's state directly in the raw file (bypassing the store).
        let path = dir.path().join("codemode_pauses.db");
        let conn = rusqlite::Connection::open(&path).expect("raw open");
        conn.execute(
            "UPDATE codemode_call_log SET state = 'bogus' WHERE run_id = 'e-corrupt'",
            [],
        )
        .expect("corrupt state");
        drop(conn);

        // (a) reads back Unknown
        let entry = d
            .store
            .get_log_entry("e-corrupt", 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            entry.state,
            LogState::Unknown,
            "a corrupt state string must read back as Unknown, not a valid terminal state"
        );

        // (b) re-deciding the same seq with matching args diverges (fails closed)
        let out = d
            .decide("e-corrupt", 0, "svc::read", &args, false, false)
            .await;
        assert!(
            matches!(out, DecideOutcome::Diverge(_)),
            "a corrupt log-entry state must diverge on replay, got {out:?}"
        );
    }
}
