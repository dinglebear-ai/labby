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
    BeginRun, CodeModeDecider, DecideOutcome, PendingCall, RunAuthFields, RunLifecycle,
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
        std::env::var("LAB_CODE_MODE_PAUSE_TTL_MS")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_PAUSED_TTL_MS)
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
            self.decide_inner(execution_id, seq, tool_id, args, requires_approval, ephemeral)
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
        Box::pin(async move {
            self.store
                .run_error(execution_id)
                .await
                .ok()
                .flatten()
        })
    }

    fn list_pending<'a>(
        &'a self,
        execution_id: &'a str,
    ) -> BoxDecideFuture<'a, Vec<PendingCall>> {
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

    fn run_auth_fields<'a>(
        &'a self,
        execution_id: &'a str,
    ) -> BoxDecideFuture<'a, Option<RunAuthFields>> {
        Box::pin(async move {
            self.store
                .load_run(execution_id)
                .await
                .ok()
                .flatten()
                .map(|run| RunAuthFields {
                    code_hash: run.code_hash,
                    actor_key: run.actor_key,
                    is_admin: run.is_admin,
                    capability_filter_fingerprint: run.capability_filter_fingerprint,
                    route_scope: run.route_scope,
                    verified: run.verified,
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
    async fn record_result_failure_leaves_entry_unapplied_and_errors_run() {
        // F1: if record_result fails (here: oversize result), the log entry must
        // NOT be marked `applied` (else a resume would replay a NULL result), and
        // the run must flip to Error so it fails closed rather than allowing a
        // double-dispatch on resume.
        let (d, _dir) = fresh_decider().await;
        begin_run(&d, "e-rec").await;
        // Fresh non-destructive call → journals + executes (state = executing).
        let out = d
            .decide("e-rec", 0, "svc::read", &serde_json::json!({"q": 1}), false, false)
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
        let fields = d.run_auth_fields("e7").await.expect("some");
        assert!(fields.verified);
        assert_eq!(fields.code_hash, "hash");
        assert_eq!(fields.actor_key.as_deref(), Some("actor"));
        assert!(fields.is_admin);
        assert_eq!(fields.capability_filter_fingerprint, "fp");
        assert_eq!(fields.route_scope, "unscoped");
        assert_eq!(fields.status, RunLifecycle::Running);
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
        let a = d.run_auth_fields("route-a").await.expect("some");
        let b = d.run_auth_fields("route-b").await.expect("some");
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
        let _ = d
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
        let _ = d
            .decide("e-exp", 0, "svc::delete", &serde_json::json!({}), true, false)
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
        let _ = d
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
}
