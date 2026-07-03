//! TRUE full-stack ("Option A") end-to-end tests for Code Mode durable
//! pause/resume.
//!
//! Unlike the protocol-level and decider-level coverage elsewhere, these drive
//! the ENTIRE real production path with NO inline host glue:
//!
//!   REAL: `LabMcpServer::call_tool_codemode_impl` (the real MCP-surface settle
//!         logic — begin-run, then read the DURABLE status after the pass
//!         settles), the real `GatewayManager` / `CodeModeHost`
//!         decide→dispatch→record dance, the production `RunnerPool` spawning
//!         the REAL `labby internal code-mode-runner` subprocess, a real
//!         `SqliteDecider` / `CodeModePauseStore` over a temp-file DB, and a
//!         DESTRUCTIVE tool in the catalog.
//!
//! The enabling seam: the production runner pool resolves its executable via
//! `labby-codemode/src/runner_exe.rs`, honoring `LAB_CODE_MODE_RUNNER_EXE`
//! (absolute, security-checked) BEFORE `current_exe()`. An integration test
//! carries `CARGO_BIN_EXE_labby` (the built debug binary), which passes that
//! check (absolute, executable, not group/world-writable, owned by the current
//! user). nextest runs each test in its own process, so the per-test env var is
//! isolated.
//!
//! Gated on `test-harness` so the file only builds under the feature (CI's
//! `--all-features` enables it, pulling in the `codemode_test_harness` wrappers
//! that expose the crate-private MCP Code Mode surface).
#![cfg(feature = "test-harness")]
// Test target: explicit `panic!` in assertion helpers is expected. The workspace
// lints set `panic = "warn"`, which CI promotes to an error via `-D warnings`.
#![allow(clippy::panic)]

use std::sync::Arc;

use labby::codemode::decider::SqliteDecider;
use labby::codemode::sqlite_pauses::CodeModePauseStore;
use labby::codemode_test_harness::{code_mode_server_with_destructive_tool, drive_codemode};
use labby_codemode::{CodeModeDecider, RunLifecycle};
use serde_json::{Value, json};

/// Point the production runner pool at the built debug `labby` binary. Set once
/// per test process (nextest isolates env per test). `CARGO_BIN_EXE_labby` is an
/// absolute path to the debug binary built for this integration target; it
/// satisfies `runner_exe.rs`'s operator-override security check.
///
/// The workspace forbids `unsafe_code`, so `std::env::set_var` (unsafe in Rust
/// 2024) is unavailable here. We instead write the var to a temp `.env` file and
/// load it through `dotenvy`, which owns its own `set_var` internally — the same
/// unsafe-free pattern `cli/serve.rs` uses for the bootstrap token.
fn use_real_runner_binary() {
    let dir = tempfile::tempdir().expect("tempdir for runner env");
    let env_path = dir.path().join("runner.env");
    std::fs::write(
        &env_path,
        format!("LAB_CODE_MODE_RUNNER_EXE={}\n", env!("CARGO_BIN_EXE_labby")),
    )
    .expect("write runner env file");
    dotenvy::from_path_override(&env_path).expect("load runner env override");
}

/// Open a fresh temp-file durable store + decider. The `TempDir` is returned so
/// it outlives the test (dropping it would delete the DB mid-run).
async fn fresh_decider() -> (Arc<SqliteDecider>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = CodeModePauseStore::open(dir.path().join("codemode_pauses.db"))
        .await
        .expect("open pause store");
    (Arc::new(SqliteDecider::new(store)), dir)
}

/// HEADLINE: a snippet that SWALLOWS the pause sentinel (via
/// `Promise.allSettled`) still ends the durable run `Paused` and dispatches
/// NOTHING to the destructive upstream.
///
/// Every layer is real: the real runner subprocess runs the JS, the real
/// `SqliteDecider` journals + gates, and the real `call_tool_codemode_impl`
/// settle logic reads the DURABLE status after the sandbox completes "ok" —
/// overriding the swallowed result. No live upstream peer is needed because the
/// destructive call PAUSES before any dispatch.
#[tokio::test]
async fn full_stack_swallowed_pause_ends_paused_and_dispatches_nothing() {
    use_real_runner_binary();
    let (decider, _dir) = fresh_decider().await;
    let handle = code_mode_server_with_destructive_tool(Arc::clone(&decider)).await;
    let destructive_id = handle.tools.destructive_id();

    // The snippet fans two destructive calls out under Promise.allSettled and
    // returns their statuses — swallowing the pause-sentinel rejections. The
    // sandbox therefore completes "ok", but the durable status is the source of
    // truth read after settle.
    let code = format!(
        r#"async () => {{
            const r = await Promise.allSettled([
              callTool("{id}", {{ id: 5 }}),
              callTool("{id}", {{ id: 6 }})
            ]);
            return r.map(x => x.status);
        }}"#,
        id = destructive_id,
    );

    // A fresh run, execute scope, NO `confirm` → the pause-capable path. This
    // begins a durable run, drives the REAL runner, and reads the durable status
    // after settle.
    let env = drive_codemode(
        &handle,
        json!({ "code": code }),
        &["lab:admin"],
        Some("actor-a"),
    )
    .await;

    // (Real envelope) The MCP surface returns confirmation_required + a
    // resume_token — proving the settle logic paused the run despite the
    // sandbox's swallowed "ok" result.
    let error = env
        .get("error")
        .unwrap_or_else(|| panic!("expected an error envelope, got: {env}"));
    assert_eq!(
        error["kind"], "confirmation_required",
        "a swallowed pause must still surface confirmation_required, got: {env}"
    );
    assert_eq!(error["status"], json!("paused"));
    let resume_token = error["resume_token"]
        .as_str()
        .unwrap_or_else(|| panic!("pause envelope must carry a resume_token, got: {env}"));
    assert!(!resume_token.is_empty(), "resume_token must be non-empty");
    // The pending destructive call(s) are surfaced, starting at seq 0. The C1
    // monotonic gate journals the first destructive call as pending and pauses
    // every later destructive call in the same pass too, so both fan-out calls
    // land as pending — none is dispatched.
    let pending = error["pending"]
        .as_array()
        .unwrap_or_else(|| panic!("pause envelope must carry pending calls, got: {env}"));
    assert!(
        !pending.is_empty(),
        "pause envelope must carry at least the first pending call, got: {env}"
    );
    assert_eq!(pending[0]["seq"], json!(0));
    assert!(
        pending
            .iter()
            .all(|p| p["tool_id"] == Value::String(destructive_id.clone())),
        "every pending call must be the destructive tool, got: {env}"
    );

    // (Real durable DB) The run is Paused — NOT the sandbox's "ok" result.
    let execution_id = resume_token.to_string();
    assert_eq!(
        decider.run_status(&execution_id).await,
        RunLifecycle::Paused,
        "the durable run must be Paused after the swallowed-pause pass settles"
    );

    // (Real durable DB) The destructive upstream was NEVER dispatched: the first
    // call PAUSES before dispatch, and the C1 monotonic gate PAUSES the rest of
    // the pass too, so both fan-out calls are journaled `pending` (awaiting
    // approval) and none reached an upstream. Every pending entry being the
    // destructive tool — with no recorded result — is the durable proof.
    let pending_db = decider.list_pending(&execution_id).await;
    assert!(
        !pending_db.is_empty(),
        "swallowing the pause must journal the destructive call(s) as pending, never dispatch"
    );
    assert_eq!(pending_db[0].seq, 0);
    assert!(
        pending_db.iter().all(|p| p.tool_id == destructive_id),
        "every journaled pending call must be the destructive tool, never dispatched"
    );
}

/// RESUME happy-path: after a first pass PAUSES at a destructive call, resuming
/// with `resume_token` + `confirm: true` + identical code leaves the run no
/// longer `Paused` and RE-DISPATCHES the approved destructive call to the
/// upstream.
///
/// Both passes use the SAME swallowing snippet (a hard requirement — the
/// resubmitted code must hash-match the paused run). Swallowing via
/// `Promise.allSettled` matters on BOTH ends: on the first pass it lets the
/// swallowed pause reach the real read-durable-status-after-settle logic (which
/// returns `confirmation_required` + a `resume_token`); on resume it lets the
/// approved destructive call actually re-dispatch to the peerless fixture
/// upstream (whose dispatch fails, but is swallowed) so the run can settle
/// `Completed` instead of erroring out early.
///
/// Every layer is real. The re-dispatch itself is the payoff: on the first pass
/// the destructive call is journaled `pending` and NEVER reaches an upstream; on
/// resume the approved call leaves `pending` (it moved to executing/applied),
/// which — combined with the run settling out of `Paused` — proves it was
/// actually re-dispatched.
#[tokio::test]
async fn full_stack_resume_leaves_paused_and_redispatches_approved_call() {
    use_real_runner_binary();
    let (decider, _dir) = fresh_decider().await;
    let handle = code_mode_server_with_destructive_tool(Arc::clone(&decider)).await;
    let destructive_id = handle.tools.destructive_id();

    // One destructive call, swallowed under allSettled so both the first-pass
    // pause and the resume re-dispatch settle through the real MCP surface
    // instead of erroring out as an uncaught rejection.
    let code = format!(
        r#"async () => {{
            const r = await Promise.allSettled([
              callTool("{id}", {{ id: 2 }})
            ]);
            return r.map(x => x.status);
        }}"#,
        id = destructive_id,
    );

    // ── FIRST PASS: pause at the destructive call ───────────────────────────
    let env = drive_codemode(
        &handle,
        json!({ "code": code }),
        &["lab:admin"],
        Some("actor-a"),
    )
    .await;
    let error = env
        .get("error")
        .unwrap_or_else(|| panic!("first pass must pause, got: {env}"));
    assert_eq!(
        error["kind"], "confirmation_required",
        "first pass must pause at the destructive call, got: {env}"
    );
    let resume_token = error["resume_token"]
        .as_str()
        .expect("pause envelope must carry a resume_token")
        .to_string();

    // The durable run is Paused with the delete pending at seq 0 — and NOT
    // dispatched (it is still awaiting approval).
    assert_eq!(
        decider.run_status(&resume_token).await,
        RunLifecycle::Paused,
        "the run must be Paused after the first pass"
    );
    let pending_before = decider.list_pending(&resume_token).await;
    assert_eq!(
        pending_before.len(),
        1,
        "the destructive call must be pending (never dispatched) after the first pass"
    );
    assert_eq!(pending_before[0].seq, 0);
    assert_eq!(pending_before[0].tool_id, destructive_id);

    // ── RESUME: identical code + confirm:true + the resume_token ────────────
    let resume_env = drive_codemode(
        &handle,
        json!({ "code": code, "confirm": true, "resume_token": resume_token }),
        &["lab:admin"],
        Some("actor-a"),
    )
    .await;

    // The resume must NOT re-pause: the approved (previously pending) call is
    // re-dispatched, not paused again. So the envelope is a normal completed
    // result (the swallowed dispatch failure keeps the sandbox "ok"), never
    // another confirmation_required.
    if let Some(resume_error) = resume_env.get("error") {
        assert_ne!(
            resume_error["kind"], "confirmation_required",
            "resume must NOT re-pause the approved call — it must re-dispatch it, got: {resume_env}"
        );
    }

    // (Real durable DB) The run is no longer Paused. `resume_to_running` flipped
    // it to Running; with no new pause on resume the settle logic marks it
    // Completed.
    let status = decider.run_status(&resume_token).await;
    assert_ne!(
        status,
        RunLifecycle::Paused,
        "a resumed run must not be left Paused; got {status:?}"
    );
    assert_eq!(
        status,
        RunLifecycle::Completed,
        "with no new pause on resume, the settled run transitions to Completed, got {status:?}"
    );

    // (Real durable DB) RE-DISPATCH PROOF: the destructive call is no longer
    // pending. On the first pass it sat `pending` (awaiting approval, never
    // dispatched); on resume the decider moved it out of `pending` via
    // pending→executing and re-dispatched it to the (peerless) upstream. An empty
    // pending list on a run that just settled out of Paused is the durable proof
    // the approved call was actually re-dispatched rather than replayed or
    // re-paused.
    let pending_after = decider.list_pending(&resume_token).await;
    assert!(
        pending_after.is_empty(),
        "the approved destructive call must leave `pending` on resume (re-dispatched), got: \
         {pending_after:?}"
    );
}
