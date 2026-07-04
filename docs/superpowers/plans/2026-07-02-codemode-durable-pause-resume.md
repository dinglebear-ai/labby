# Code Mode Durable Pause/Resume Implementation Plan (Cloudflare-faithful port)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Code Mode mid-script human-in-the-loop pause/resume for destructive upstream calls, backed by a durable SQLite log that survives `labby.service` restarts — by porting Cloudflare `agents`' proven `CodemodeRuntime` design (`packages/codemode/src/runtime.ts`) into Labby's Rust crate architecture, rather than inventing a home-grown replay mechanism.

**Architecture:** Port Cloudflare's durable-execution model. A run journals every tool call to SQLite; a **decision layer** (`decide()`) returns `replay` / `execute` / `pause` / `diverge` per call. Pausing is enforced by **durable execution status + a monotonic gate**, NOT by an exception propagating: once a run flips to `paused`, every subsequent `decide()` returns `pause` and records nothing, so model JS that swallows the pause sentinel (`Promise.allSettled`, `try/catch`) can drive no further side effects; the host reads the **durable status after the run settles** to decide whether it paused. Resume re-runs the snippet from the top (mandatory — "fresh `javy::Runtime` per Start" invariant) with already-applied calls short-circuited to their recorded results, matched by **emission `seq` + `(tool_id, method, stable-stringify(args))` divergence detection** (a hard, model-actionable error on mismatch). Oversized args/results **fail the run** (never truncate — truncation corrupts replay). `codemode.step(name, fn)` journals nondeterministic/side-effectful work so replay stays deterministic.

**Reference (port source — cite these while implementing):**
- `/home/jmagar/workspace/upstream/cloudflare-agents/packages/codemode/src/runtime.ts` — `CodemodeRuntime` (schema, `begin`/`resume`/`decide`/`recordResult`/`complete`/`fail`/`reject`/`expirePaused`, `stableStringify`).
- `.../packages/codemode/src/proxy-tool.ts` — the host driver: how it calls `decide`→dispatch→`recordResult`, the `{ status: "paused", executionId, pending }` output, the pause sentinel (best-effort halt only), and `codemode.step`.

**Tech Stack:** Rust 2024, `rusqlite` 0.39 (bundled), `r2d2`/`r2d2_sqlite`, `sha2` (content hashing + HMAC via `hmac`), `ulid`, `thiserror`, `tokio::task::spawn_blocking`, `tracing`. No new external dependencies.

## Global Constraints

*(Every task implicitly includes this section.)*

- **Port fidelity:** when a Labby decision has a Cloudflare analog, match Cloudflare's behavior and cite the `runtime.ts`/`proxy-tool.ts` line. Deviate only where Labby's architecture forces it (crate split, subprocess runner, redaction/HMAC), and document the deviation.
- **Pausing is decided by durable status, never by an escaping exception.** `decide()` flips `status='paused'`; a monotonic guard returns `pause` for every subsequent call in a non-`running` run; the host inspects durable status after the run settles. The pause sentinel error is a best-effort sandbox halt only (`proxy-tool.ts:222-224, 650`). This is the C1 fix — do not regress to "return a catchable error and hope it propagates."
- **Determinism contract:** all snippet work outside connector calls and `codemode.step` must be deterministic across replays. Divergence (`(tool_id, method, args)` mismatch at a `seq`) is a **hard, model-actionable error** (`resume_divergence`), never a silent stale-result application (`runtime.ts:436-455`).
- **No truncation of journaled values.** Oversized args/results fail the run with a model-actionable message; truncating would feed resumed code corrupted data (`runtime.ts:146-153, 490-499, 535-563`). Cap: reuse a `MAX_DURABLE_VALUE_BYTES` const (start at 1 MiB, matching Cloudflare).
- **Locked v1 scope:** durable pause/resume log with replay + `codemode.step` + reject + lazy TTL expiry. **Deferred (do NOT build):** `rollback()`/compensating-action machinery (Cloudflare's tier-d `actionsToRevert`), a background expiry timer, encryption-at-rest, multi-node durability.
- **Crate boundaries (hard):** `labby-codemode` stays storage-neutral (gains only the `CodeModeDecider` *trait* + a `redact_trace_value` re-export — no SQLite). The SQLite store + the `decide()` port live in the `labby` binary crate (`crates/labby/src/codemode/`), injected down into `GatewayManager` (labby-gateway) as `Arc<dyn CodeModeDecider>`. Never in `labby-apis`.
- **Feature gating:** the `codemode` binary module is gated `#[cfg(feature = "gateway")]` (it calls `labby_codemode::redact_trace_value`, only present under `gateway`).
- **DB path:** `labby_runtime::lab_home().join("codemode_pauses.db")` → `~/.labby/codemode_pauses.db`. Not `acp.db`; not the stale `$HOME/.lab` path.
- **No `mod.rs` files. No `#[async_trait]`** — native `async fn in trait` only.
- **Redaction + integrity:** args/results are redacted (`redact_trace_value`) before disk; `params_hash`/`args`-for-divergence use the **raw** value via a canonical stable-stringify, captured once at the pre-redaction boundary (never re-derived from redacted data). `status`, `is_admin`, and `capability_filter_fingerprint` in the runs table are **HMAC-signed** (mirror `acp/sqlite_persistence.rs`) and verified before any authorization decision trusts them.
- **Fail closed** on every gate; **machine-readable bypass** on every surface (`params.resume_token`/`params.confirm`); **error kinds are a spec change** (atomic `docs/dev/ERRORS.md` update same commit); DB file `0600`; path resolution rejects `..`.
- **Verification:** `cargo clippy --all-features` clean; `cargo nextest run --all-features` green; `just deny` passes.

---

## Crate Seam (how Cloudflare's single Durable Object maps onto Labby's 3 crates)

Cloudflare packs SQLite + `decide()` + the driver into one `CodemodeRuntime` DO. Labby's dependency direction is `labby (binary) → labby-gateway → labby-codemode`, and SQLite must live at the top (binary). The port splits `CodemodeRuntime` across the seam:

| Cloudflare piece | Labby home | Crate |
|---|---|---|
| `cm_executions`/`cm_log` SQLite + row CRUD | `crates/labby/src/codemode/sqlite_pauses.rs` | `labby` (binary) |
| `decide()`/`recordResult()`/`complete()`/`fail()`/`reject()`/`expirePaused()` logic | `crates/labby/src/codemode/decider.rs` (`SqliteDecider`) | `labby` (binary) |
| The `decide→dispatch→record` driver (proxy-tool) | `code_mode_host.rs::call_tool` (already dispatches upstream; add decide/record around it) | `labby-gateway` |
| The abstract contract the host calls | `CodeModeDecider` trait | `labby-codemode` (`host.rs`, next to `CodeModeHost`) |

`GatewayManager` gains `Option<Arc<dyn CodeModeDecider>>`, injected by the binary at construction. `None` ⇒ today's behavior (no pause). `CodeModeHost::call_tool` gains an execution context (`execution_id`, `seq`) threaded from the driver (`runner_drive.rs` already owns the protocol `seq` at the `ToolCall` event, `runner_drive.rs:333`).

**Perf scope:** journaling happens only on the **pause-capable path** — an MCP, execute-capable run that is NOT pre-confirmed with whole-run `confirm:true`. Pre-confirmed runs, CLI runs, and runs with no injected decider take today's write-free path unchanged. This bounds the "one row per call" cost (which Cloudflare pays unconditionally) to exactly the runs that can pause. Accepted, documented trade-off.

---

# Wave 1 — Durable store, ported schema + HMAC (`lab-yp0s2.1`)

> Supersedes the earlier standalone store plan `2026-07-02-codemode-pauses-sqlite-store.md` where they differ (schema is now Cloudflare-faithful; `redacted_result`/state-machine/HMAC added). Reuse that plan's connection-pool boilerplate (r2d2 write max_size=1 / read max_size=4 `query_only`, WAL, `spawn_blocking`, 0600, `..` rejection) verbatim from `crates/labby/src/acp/sqlite_persistence.rs`.

**File Structure:**
- Create `crates/labby/src/codemode.rs` (module entry, declares `sqlite_pauses` + `decider`), gated `#[cfg(feature = "gateway")]`.
- Create `crates/labby/src/codemode/sqlite_pauses.rs` (store: schema, pools, CRUD, HMAC, redaction call-through).
- Modify `crates/labby-codemode/src/trace.rs` + `lib.rs` — make `redact_trace_value` `pub` and re-export (per the superseded plan's Task 1).

## Task 1.1: Ported schema + pools + HMAC scaffolding

**Files:** `crates/labby/src/codemode/sqlite_pauses.rs`

**Schema (port of `runtime.ts:302-336`, adapted):**

```sql
CREATE TABLE IF NOT EXISTS codemode_runs (          -- ≈ cm_executions
    execution_id                  TEXT PRIMARY KEY,   -- ulid + random suffix (V4)
    code_hash                     TEXT NOT NULL,      -- sha256 of submitted code (source NOT stored)
    status                        TEXT NOT NULL,      -- running|paused|completed|error|rejected|expired
    actor_key                     TEXT,
    is_admin                      INTEGER NOT NULL,
    route_scope                   TEXT NOT NULL,
    capability_filter_fingerprint TEXT NOT NULL,
    integrity_sig                 TEXT NOT NULL,      -- HMAC over (status,is_admin,fingerprint,route_scope,actor_key) (V6)
    created_at_ms                 INTEGER NOT NULL,
    updated_at_ms                 INTEGER NOT NULL,
    expires_at_ms                 INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_codemode_runs_status_expires
    ON codemode_runs(status, expires_at_ms);          -- serves expire sweep (perf)

CREATE TABLE IF NOT EXISTS codemode_call_log (      -- ≈ cm_log
    run_id            TEXT NOT NULL REFERENCES codemode_runs(execution_id),
    seq               INTEGER NOT NULL,               -- emission seq from protocol.rs
    tool_id           TEXT NOT NULL,
    args_hash         TEXT NOT NULL,                  -- canonical stable-stringify of RAW args (divergence)
    redacted_args     TEXT NOT NULL,                  -- redacted, for audit/divergence-message only
    redacted_result   TEXT,                           -- redacted recorded result (replay), NULL until recorded
    requires_approval INTEGER NOT NULL DEFAULT 0,
    ephemeral         INTEGER NOT NULL DEFAULT 0,     -- re-execute on replay instead of replaying result
    state             TEXT NOT NULL,                  -- pending|executing|applied|reverted
    applied_at_ms     INTEGER,
    PRIMARY KEY (run_id, seq)                          -- covers load-by-run (drop any redundant run index)
);
```

- [ ] **Step 1:** Copy the pool/pragma/`open`/`from_lab_home`/`blocking_write`/`blocking_read`/`0600`/`reject_path_traversal` boilerplate from `sqlite_persistence.rs`, renaming to `CodeModePauseStore`. Add `conn.pragma_update(None, "wal_autocheckpoint", 1000_i64)?;` to the pragma init (perf fix — the superseded plan omitted it). Path via `labby_runtime::lab_home().join("codemode_pauses.db")`.
- [ ] **Step 2:** Add `const MAX_DURABLE_VALUE_BYTES: usize = 1_000_000;` and an HMAC helper pair `sign_run_fields(...) -> String` / `verify_run_fields(row) -> bool` reusing the `acp_hmac_key()` pattern (env `LAB_CODEMODE_HMAC_SECRET`, ephemeral per-process fallback). Sign the tuple `(status, is_admin, capability_filter_fingerprint, route_scope, actor_key)`.
- [ ] **Step 3:** Write the schema-bootstrap test (fresh DB creates both tables + the index; re-running `migrate` is idempotent). Run `cargo nextest run -p labby codemode::sqlite_pauses::tests::schema --all-features`. Commit.

## Task 1.2: Row types + status enum (clippy-safe)

**Files:** `crates/labby/src/codemode/sqlite_pauses.rs`

- [ ] **Step 1:** Define `RunStatus { Running, Paused, Completed, Error, Rejected, Expired }` with `as_str(&self) -> &'static str` and **`parse_status(s: &str) -> Option<Self>`** (NOT `from_str` — inherent `from_str` trips `clippy::should_implement_trait` and would fail the all-features gate; C7 fix). Define `LogState { Pending, Executing, Applied, Reverted }` similarly.
- [ ] **Step 2:** Define `NewRun { execution_id, code_hash, actor_key: Option<String>, is_admin, route_scope, capability_filter_fingerprint, expires_at_ms }`, `Run { …all columns…, verified: bool }` (where `verified` is the HMAC check result), `NewLogEntry { seq, tool_id, raw_args: Value, requires_approval, ephemeral }`, `LogEntry { seq, tool_id, args_hash, redacted_args, redacted_result: Option<Value>, requires_approval, ephemeral, state: LogState }`.
- [ ] **Step 3:** Unit-test `parse_status`/`as_str` round-trips for all variants. Commit.

## Task 1.3: Store CRUD ported 1:1 from `runtime.ts`

**Files:** `crates/labby/src/codemode/sqlite_pauses.rs`

Each method ports the cited `runtime.ts` method. All use `blocking_write`/`blocking_read`.

**Interfaces (produced):**
```rust
impl CodeModePauseStore {
    // begin() — runtime.ts:355. Inserts a 'running' run row (HMAC-signed). Prunes terminal rows to a cap.
    pub async fn begin(&self, run: NewRun) -> Result<(), CodeModePauseStoreError>;
    // load_run() with HMAC verify (sets Run.verified=false on tamper). runtime.ts #executionRow.
    pub async fn load_run(&self, execution_id: &str) -> Result<Option<Run>, CodeModePauseStoreError>;
    // resume(): CAS paused→running (runtime.ts:383). Returns true iff a paused row transitioned.
    pub async fn resume_to_running(&self, execution_id: &str) -> Result<bool, CodeModePauseStoreError>;
    // set_status with re-signed integrity_sig. Used by complete/fail/reject/expire.
    pub async fn set_status(&self, execution_id: &str, to: RunStatus, error: Option<&str>) -> Result<bool, CodeModePauseStoreError>;
    // log CRUD (runtime.ts #logRow / INSERT OR REPLACE / #setEntryState / recordResult UPDATE):
    pub async fn get_log_entry(&self, run_id: &str, seq: i64) -> Result<Option<LogEntry>, CodeModePauseStoreError>;
    pub async fn upsert_log_entry(&self, run_id: &str, entry: NewLogEntry) -> Result<(), CodeModePauseStoreError>; // INSERT OR REPLACE, redacts+hashes here
    pub async fn set_entry_state(&self, run_id: &str, seq: i64, state: LogState) -> Result<(), CodeModePauseStoreError>;
    pub async fn record_entry_result(&self, run_id: &str, seq: i64, raw_result: Option<Value>) -> Result<(), CodeModePauseStoreError>; // NULL for ephemeral; redacts here
    pub async fn load_call_log(&self, run_id: &str) -> Result<Vec<LogEntry>, CodeModePauseStoreError>; // single ORDER BY seq query (audit/list_pending)
    pub async fn list_pending(&self, execution_id: &str) -> Result<Vec<LogEntry>, CodeModePauseStoreError>; // paused runs only (runtime.ts:640)
    // expire_paused(): paused→rejected, stale-running→error, older than cutoff (runtime.ts:699). Returns count.
    pub async fn expire_paused(&self, older_than_ms: i64) -> Result<usize, CodeModePauseStoreError>;
    pub fn canonical_args_hash(args: &Value) -> String; // sha256(stable_stringify(RAW args)) — one hash fn, raw input
}
```

- [ ] **Step 1 (redaction/hash correctness — perf#1/#3 fix):** In `upsert_log_entry`/`record_entry_result`, compute `args_hash`/`result_hash` from the **RAW** value via `canonical_args_hash` (stable, key-sorted), and store `redacted_args`/`redacted_result` via `redact_trace_value` **separately**. Never hash redacted data; never derive the hash from the display trace. On oversize (`> MAX_DURABLE_VALUE_BYTES`) return a distinct `ValueTooLarge` error (the decider turns it into a run failure, per no-truncation).
- [ ] **Step 2:** Port `stable_stringify` from `runtime.ts:191` (recursively key-sorted JSON; return `None` on non-serializable → caller skips the args check rather than false-diverging).
- [ ] **Step 3:** TDD each method with a failing test first (round-trip, CAS-once for `resume_to_running`, HMAC-tamper → `verified=false`, `expire_paused` flips only stale paused/running rows, `upsert` redaction + on-disk byte-scan for no raw secret). Commit per method group.

## Wave 1 gate
- [ ] `cargo clippy --all-features` clean (no `should_implement_trait`); `cargo nextest run -p labby --all-features` green.
- [ ] On-disk byte-scan: raw `api_key`/`token` values never appear in `codemode_pauses.db`; `[REDACTED]` does.
- [ ] HMAC tamper of `status`/`is_admin` in the raw file makes `load_run` return `verified=false`.

---

# Wave 2 — Decider + kernel/host wiring (`lab-yp0s2.2`)

**Depends on:** Wave 1. This is the port of `decide()`/`recordResult()` and the proxy-tool driver.

**File Structure:**
- `crates/labby-codemode/src/host.rs` — new `CodeModeDecider` trait + `DecideOutcome`; extend `CodeModeHost::call_tool` signature with execution context.
- `crates/labby-codemode/src/runner_drive.rs` — thread `execution_id` + protocol `seq` into the `call_tool` invocation (`runner_drive.rs:333/596-623`).
- `crates/labby/src/codemode/decider.rs` — `SqliteDecider` implementing `CodeModeDecider` over `CodeModePauseStore` (ports `runtime.ts:411-572`).
- `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs` — call `decide → dispatch → record` around the existing upstream dispatch; the destructive gate (`:79-97`) computes `requires_approval`.
- `crates/labby/src/mcp/call_tool_codemode.rs` — `begin()` the run on the pause-capable path; after settle, read durable status → paused/completed/error envelope; inject the decider into `GatewayManager`.
- `docs/dev/{ERRORS,CODE_MODE}.md`.

## Task 2.1: `CodeModeDecider` trait + `DecideOutcome` (labby-codemode, storage-neutral)

**Files:** `crates/labby-codemode/src/host.rs`

```rust
pub enum DecideOutcome {
    Replay(serde_json::Value),   // return cached result, do NOT dispatch (runtime.ts:464)
    Execute,                     // dispatch for real, then call record_result (runtime.ts:527)
    Pause,                       // run flipped to paused; return the pause sentinel (runtime.ts:524)
    Diverge(String),             // hard model-actionable error (runtime.ts:437/448)
    Fail(String),                // oversize/unserializable args → terminal (runtime.ts:494)
}
pub trait CodeModeDecider: Send + Sync {
    fn decide(&self, execution_id: &str, seq: u64, tool_id: &str, args: &serde_json::Value,
              requires_approval: bool, ephemeral: bool)
        -> impl std::future::Future<Output = DecideOutcome> + Send;
    fn record_result(&self, execution_id: &str, seq: u64, result: &serde_json::Value)
        -> impl std::future::Future<Output = Result<(), ToolError>> + Send;
}
```

- [ ] Add the trait + enum; extend `CodeModeHost::call_tool` to accept an `ExecCtx { execution_id: Option<&str>, seq: u64 }` (Option so the no-decider/standalone path passes `None`). Commit.

## Task 2.2: Port `decide()` into `SqliteDecider`

**Files:** `crates/labby/src/codemode/decider.rs`

Port `runtime.ts:411-528` verbatim in structure. Load the run row; **monotonic gate first**:

```rust
async fn decide(&self, execution_id, seq, tool_id, args, requires_approval, ephemeral) -> DecideOutcome {
    let Some(run) = self.store.load_run(execution_id).await.ok().flatten() else { return DecideOutcome::Fail("unknown execution".into()); };
    if !run.verified { return DecideOutcome::Fail("integrity check failed".into()); } // V6
    if run.status != RunStatus::Running { return DecideOutcome::Pause; }              // C1 monotonic gate (runtime.ts:428)

    if let Some(existing) = self.store.get_log_entry(execution_id, seq as i64).await? {
        // divergence: tool_id + stable-stringify(args) must match (runtime.ts:436-455)
        let after = canonical_args_hash(args);
        if existing.tool_id != tool_id || existing.args_hash != after {
            self.store.set_status(execution_id, RunStatus::Error, Some(&divergence_msg)).await?;
            return DecideOutcome::Diverge(divergence_msg); // resume_divergence, fail closed
        }
        match existing.state {
            LogState::Applied if existing.ephemeral => return DecideOutcome::Execute,      // re-run (runtime.ts:461)
            LogState::Applied => return DecideOutcome::Replay(existing.redacted_result…),   // replay cached (runtime.ts:464)
            LogState::Pending => { self.store.set_entry_state(execution_id, seq, Executing).await?; return DecideOutcome::Execute; } // approved (runtime.ts:473)
            LogState::Executing => return DecideOutcome::Execute,                           // crash window (runtime.ts:481)
            LogState::Reverted => {} // fall through as fresh
        }
    }
    // fresh call: journal, oversize → Fail (no truncation, runtime.ts:494)
    match self.store.upsert_log_entry(execution_id, NewLogEntry { seq: seq as i64, tool_id, raw_args, requires_approval, ephemeral }).await {
        Err(ValueTooLarge{msg}) => { self.store.set_status(execution_id, Error, Some(&msg)).await?; return DecideOutcome::Fail(msg); }
        Err(e) => return DecideOutcome::Fail(e.to_string()),
        Ok(()) => {}
    }
    if requires_approval {
        self.store.set_status(execution_id, RunStatus::Paused, None).await?; // flip durable status (runtime.ts:520)
        return DecideOutcome::Pause;
    }
    DecideOutcome::Execute
}
```

**Replay-result note (C4 fix):** `redacted_result` is the recorded result. Because Wave 1 fails (never truncates) on oversize at record time, an `Applied` non-ephemeral entry always has an intact result; there is no truncation-marker-as-value hazard. If `redacted_result` is `NULL` on an `Applied` non-ephemeral entry (shouldn't happen), treat as `Diverge` (fail closed), never return `Null` as if it were the value.

- [ ] TDD: (a) fresh non-destructive call → `Execute` + a `pending`/`executing` row; (b) fresh destructive-unconfirmed → `Pause` + run `paused`; (c) second call after pause → `Pause` (monotonic), nothing logged; (d) replay of an `applied` entry with matching args → `Replay(result)`, no dispatch; (e) mismatched args at a seq → `Diverge`, run `error`; (f) oversize args → `Fail`, run `error`; (g) tampered `is_admin` (HMAC) → `Fail`. Commit.

## Task 2.3: `record_result` + host driver wiring

**Files:** `crates/labby/src/codemode/decider.rs`, `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`

- [ ] **Step 1:** Port `record_result` (`runtime.ts:543-572`): `record_entry_result` → mark `applied`; oversize/unserializable result → fail the run (no truncation).
- [ ] **Step 2:** In `code_mode_host.rs::call_tool`, wrap the existing upstream dispatch with the decide/record dance (port of `proxy-tool.ts:402-428`). Compute `requires_approval = upstream_tool.destructive && !destructive_permitted(surface, caller)` (reuses the existing `:79-97` gate — no soft-pass). If no decider is injected (`self.decider.is_none()`), take today's path unchanged.

```rust
if let Some(decider) = &self.decider {
    if let Some(exec_id) = ctx.execution_id {
        match decider.decide(exec_id, ctx.seq, id, &params, requires_approval, /*ephemeral*/ false).await {
            DecideOutcome::Replay(v) => return Ok(ToolCallOutcome { value: v, ui: None }), // no dispatch
            DecideOutcome::Pause     => return Err(pause_sentinel_error()),                 // best-effort halt; status already paused
            DecideOutcome::Diverge(m)=> return Err(ToolError::Sdk { sdk_kind: "resume_divergence".into(), message: m }),
            DecideOutcome::Fail(m)   => return Err(ToolError::Sdk { sdk_kind: "internal_error".into(), message: m }),
            DecideOutcome::Execute   => { /* fall through to real dispatch, then record */ }
        }
        let outcome = /* existing real dispatch */;
        decider.record_result(exec_id, ctx.seq, &outcome.value).await.ok();
        return Ok(outcome);
    }
}
/* existing dispatch path unchanged */
```

- [ ] **Step 3:** Thread `execution_id` + `seq` into `call_tool` from `runner_drive.rs` (the `ToolCall { seq, id, params }` event at `:333` already has both — pass them into `enqueue_tool_call` → `call_tool_id` → `host.call_tool`). TDD with a gateway test double: destructive-unconfirmed pauses; a matching replay short-circuits (host upstream NOT hit). Commit.

## Task 2.4: `begin` the run + read durable status after settle (the C1 payoff)

**Files:** `crates/labby/src/mcp/call_tool_codemode.rs`

- [ ] **Step 1:** Inject `Arc<dyn CodeModeDecider>` into `GatewayManager` at construction (binary side); build one `CodeModePauseStore`/`SqliteDecider` cached in gateway state (never per request — perf).
- [ ] **Step 2:** On the pause-capable path (MCP, execute-capable, not pre-confirmed with whole-run `confirm:true`, no `resume_token`), `store.begin(NewRun { execution_id, code_hash: sha256(code), actor_key, is_admin, route_scope, capability_filter_fingerprint, expires_at_ms })` before driving. `execution_id = format!("exec_{ts}_{ulid}")` with a random component (V4; mirror `runtime.ts:360`).
- [ ] **Step 3 (the C1 fix):** After the driver returns (Completed OR ExecutionError — a swallowed pause completes "ok"), **read `store.load_run(execution_id).status`**:
  - `Paused` → return the `confirmation_required` envelope carrying `resume_token` + `list_pending` summary (port of `proxy-tool.ts:108-112` `{ status: "paused", pending }`), regardless of the sandbox's own result.
  - `Error` → return the recorded error.
  - else → normal completed result; `store.set_status(Completed)`.
- [ ] **Step 4:** TDD the swallow case: a snippet that wraps the destructive call in `Promise.allSettled`/`try-catch` and returns normally still yields a `paused` envelope with a `resume_token`, and the upstream destructive tool is never dispatched (assert via test-double call count). This is the test that proves C1 is fixed. Commit.

## Task 2.5: Error kinds in docs (same commit)

**Files:** `docs/dev/ERRORS.md`, `docs/dev/CODE_MODE.md`

- [ ] Add `resume_divergence` (and confirm `confirmation_required` reuse for the paused envelope) to ERRORS.md in the existing entry format. Supersede CODE_MODE.md:423-429 with the pause/resume contract + the determinism note ("all code outside connector calls and `codemode.step` must be deterministic across replays") mirroring `proxy-tool.ts:575-578`. Commit.

## Wave 2 gate
- [ ] The swallow-the-pause test (Task 2.4 Step 4) passes — a `Promise.allSettled` snippet pauses durably and dispatches nothing.
- [ ] Non-paused happy path unchanged; `cargo nextest run --all-features` green; clippy clean.

---

# Wave 3 — Resume/reject MCP surface + resume authorization (`lab-yp0s2.3`)

**Depends on:** Wave 2. **Scope:** MCP-only (Code Mode has no CLI/HTTP surface today — documented, not built).

## Task 3.1: Resume dispatch with full authorization (V1/V3 fixes)

**Files:** `crates/labby/src/mcp/call_tool_codemode.rs`

On `params.resume_token` + `params.confirm: true` + resubmitted `code`:

- [ ] **Step 1 — checks BEFORE the CAS (fail-closed ordering):**
  1. `run = load_run(token)`; missing → `unknown_execution`; `!run.verified` → fail closed (V6); `status != Paused` → `already_resumed`/terminal.
  2. **Code identity:** `sha256(resubmitted code) == run.code_hash` else `resume_divergence` (source not persisted — caller resubmits identical code).
  3. **Actor identity (V3):** `run.actor_key == live_actor_key` (with `None` matching only `None`, never bridging trusted-local↔scoped) — mirror `CodeModeSourceStore::resolve` (`types.rs:648`).
  4. **Live authorization (V1 — the critical fix):** recompute `code_mode_capabilities_for_scopes(&live_auth.scopes)` at resume time; require `live.can_execute && (run.is_admin == live.is_admin)` AND `run.capability_filter_fingerprint == live_fingerprint`. The fingerprint alone (namespaces/tools only) is NOT an authz check — the recomputed live capabilities are. If live scope narrowed/revoked since pause → fail closed even though the fingerprint matches.
- [ ] **Step 2 — CAS + re-run:** `store.resume_to_running(token)` (CAS `Paused→Running`, `runtime.ts:383`); loser gets `already_resumed`. Then re-drive the snippet with the same `execution_id` injected — `decide()` replays applied entries, transitions the previously-`pending` (now approved) destructive call to `Executing`→`Execute` (dispatches for real), and pauses again at the next unconfirmed destructive call. After settle, read durable status → paused-again / completed / error (same as Task 2.4 Step 3).
- [ ] **Step 3 — intent log:** fire a pre-dispatch INFO intent log before the confirmed destructive call re-dispatches (OBSERVABILITY pre-execution-intent requirement).
- [ ] **Step 4 — TDD the four+ cases:** (a) identical code + confirm → prior applied calls replayed (no re-dispatch), confirmed call fires once; (b) two concurrent resumes → exactly one dispatches, other `already_resumed`; (c) live caps revoked since pause (same fingerprint) → fails closed with auth error; (d) different resubmitted code → `resume_divergence`; (e) different actor_key → forbidden. Commit.

## Task 3.2: Reject action

**Files:** `crates/labby/src/mcp/call_tool_codemode.rs`

- [ ] On `params.resume_token` + `params.confirm: false`: `store.set_status(token, Rejected, Some("rejected by user"))` guarded on current `Paused` (port `runtime.ts:668`); return a reject ack. A subsequent resume with the same token fails closed. Reject is NOT destructive (`ActionSpec.destructive=false`). TDD + commit.

## Task 3.3: Document the MCP pause/resume/reject contract + CLI/HTTP N/A

**Files:** `docs/dev/CODE_MODE.md`, `docs/dev/ERRORS.md`, verify `docs/CONVENTIONS.md`

- [ ] Worked MCP JSON example (pause → `confirmation_required`+`resume_token` → resume/reject). One sentence: Code Mode is MCP-only today; a future CLI/HTTP surface must carry `resume_token`/`confirm` as params. Add `already_resumed` + `unknown_execution` (reuse) to ERRORS.md. Commit.

## Wave 3 gate
- [ ] V1 test named explicitly: `resume_fails_when_admin_or_scope_revoked_even_with_matching_fingerprint` passes.
- [ ] Whole-run `confirm:true` contract unregressed for callers who never use `resume_token`. Clippy/nextest green.

---

# Wave 4 — `codemode.step`, lazy expiry, observability, hardening (`lab-yp0s2.4`)

**Depends on:** Waves 1–3.

## Task 4.1: `codemode.step(name, fn)` determinism primitive (C3 fix)

**Files:** `crates/labby-codemode/src/preamble.rs` (JS bridge), `crates/labby-codemode/src/protocol.rs` + `runner.rs`/`runner_drive.rs` (a `StepCall` event), `crates/labby-gateway/.../code_mode_host.rs` (route step through `decide`/`record` as a pseudo-connector), `docs/dev/CODE_MODE.md`.

Port `proxy-tool.ts:226-237, 518-532`: `codemode.step(name, fn)` runs `fn` once, journals its result via `decide`(ephemeral=false)/`record_result`, and replays it on resume so nondeterministic/side-effectful work (random, time, raw fetch, reads that drift) doesn't cause divergence.

- [ ] **Step 1:** Add a `step` bridge global to the preamble that emits a `StepCall { seq, name }` protocol event and awaits a result (mirror the `ToolCall` promise machinery in `runner.rs:410`). Runner-side assigns `seq` from the same counter as tool calls (one ordinal space — the C2 correctness requirement).
- [ ] **Step 2:** Parent-side, route `StepCall` through `decider.decide(exec_id, seq, "codemode::step:<name>", args=Null, requires_approval=false)`: `Replay` → return journaled value without running `fn`; `Execute` → run `fn` in-sandbox, `record_result`. (For v1, `fn` runs in-sandbox; the host only journals its returned value.)
- [ ] **Step 3:** Mark Labby's local `state`/`git` providers **non-resumable**: a run that called a local provider cannot be resumed (fail closed with a clear error) OR its calls are journaled as `ephemeral` re-runs — pick fail-closed for v1 (simpler, safe), documented (C3: local providers are stateful and can't be safely replayed). TDD: a snippet with a nondeterministic value wrapped in `step` resumes cleanly; the same value unwrapped → `resume_divergence`. Commit.

## Task 4.2: Lazy, throttled TTL expiry

**Files:** `crates/labby/src/mcp/call_tool_codemode.rs`

- [ ] Port `expirePaused` invocation as a **lazy, throttled** sweep: on pause/resume/reject dispatch, if `now - last_sweep_ms > 60_000` (an `AtomicI64` on the decider), call `store.expire_paused(now - pause_ttl_ms())`. No background timer. `pause_ttl_ms()` reads `LAB_CODE_MODE_PAUSE_TTL_MS` (default 24h, matching `DEFAULT_PAUSED_TTL_MS`; `runtime.ts:156`). The sweep does only SQLite work (no `RunnerPool`, no subprocess). TDD lazy-expiry + throttle. Commit.

## Task 4.3: Observability + threat-model + hardening

**Files:** `crates/labby/src/mcp/call_tool_codemode.rs`, `crates/labby/src/codemode/sqlite_pauses.rs` (doc comment), `docs/dev/CODE_MODE.md`

- [ ] **Step 1:** Every pause/resume/reject/expire event emits standard dispatch fields (`surface`, `service`, `action`, `elapsed_ms`; `kind` on errors); destructive re-dispatch logs intent-before + outcome-after (OBSERVABILITY.md:244-286). TDD via the repo's tracing-capture harness.
- [ ] **Step 2:** Module threat-model doc comment: what IS stored (redacted args/results, tool ids, call ordering, caller identity, route scope, HMAC sig), what is NOT (raw args/results, snippet source), file perms 0600, and the two residual risks stated honestly — (a) **confidentiality:** metadata reveals operational intelligence to anyone with fs read access *at the operator's privilege level* (0600 only stops *other* Unix users; accepted at single-operator scale); (b) **integrity:** HMAC on `status`/`is_admin`/`fingerprint` defends the raw-file-write tamper path (V6) — a verification failure fails closed. Note the redaction-dictionary caveat: the byte-scan test only catches known secret key names; a novel upstream secret field could slip through (documented limitation).
- [ ] **Step 3:** `bd show lab-y08q1` (+children) — confirm prior Code Mode security/lifecycle bugs aren't reintroduced; note in the PR. Commit.

## Wave 4 gate
- [ ] `codemode.step` resumability test passes; local providers fail closed on resume.
- [ ] OBSERVABILITY required-fields self-audit passes; `just deny` passes; clippy/nextest green all-features.

---

## Failure-Mode Coverage (post-port)

| Codepath | Failure mode | Rescued? | Test | Source |
|---|---|---|---|---|
| Pause | Snippet swallows pause sentinel (`allSettled`/`try-catch`) | Y — durable status + monotonic gate; host reads durable status | 2.4 Step 4 | C1 / `runtime.ts:428` |
| Replay match | Nondeterministic call order / edited snippet | Y — `(tool_id,args)` divergence → hard `resume_divergence` | 2.2(e) | C2 / `runtime.ts:436` |
| Replay value | Oversize result would truncate → corrupt replay | Y — record fails the run, never truncates | 2.3 | C4 / `runtime.ts:146` |
| Replay drift | Nondeterministic pre-destructive work | Y — `codemode.step` journals it; unwrapped → divergence | 4.1 | C3 / `proxy-tool.ts:232` |
| Local providers | `state`/`git` double-apply on replay | Y — non-resumable (fail closed) | 4.1 Step 3 | C3 |
| Resume authz | Caller scope revoked since pause (fingerprint unchanged) | Y — live `is_admin`/`can_execute` recompute | 3.1(c) | V1 |
| Resume identity | Different actor resumes another's token | Y — `actor_key` gate | 3.1(e) | V3 |
| Tamper | Raw-file flip of `status`/`is_admin` | Y — HMAC verify → fail closed | 2.2(g) | V6 |
| Concurrency | Two resumes race | Y — CAS `Paused→Running` once | 3.1(b) | eng-review |
| Expiry | Abandoned paused run lingers | Accepted (lazy+throttled); no correctness impact | 4.2 | — |

## Self-Review Notes

- **Every FATAL/critical eng-review finding maps to a task:** C1→2.2/2.4, C2→2.2/4.1 (one seq space + divergence), C3→4.1, C4→1.3/2.3 (no-truncation), C5→dropped (no model-facing result field; results live only in the store), C7→1.2 (`parse_status`), V1→3.1, V3→3.1, V4→2.4 (random id suffix), V6→1.1/1.3/2.2 (HMAC), Perf#1/#3→1.3 (raw canonical hash, capture pre-redaction), Perf index/WAL→1.1, Perf throttle→4.2.
- **Fidelity:** each ported behavior cites its `runtime.ts`/`proxy-tool.ts` source; deviations (crate split, redaction, HMAC, `code_hash` instead of stored source, non-resumable local providers) are documented and forced by Labby's architecture/constraints.
- **Deferred, as locked:** `rollback()`/`actionsToRevert` compensating actions; background expiry timer; encryption-at-rest; multi-node.
- **Perf trade-off accepted & documented:** journaling per call on the pause-capable path only (what Cloudflare does unconditionally); write-free hot path preserved for pre-confirmed/CLI/no-decider runs.
