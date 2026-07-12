# Code Mode Notebook-as-Log (Durable Step Journal, v1 Write Half) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist every `codemode.step(name, fn)` boundary of a Code Mode run to a durable append-only journal and expose it as a read-only notebook, without ever pausing or gating a running snippet.

**Architecture:** The `labby-codemode` kernel already emits `step_begin`/`step_result` and routes them to `CodeModeHost` hooks that currently no-op. We (1) thread `execution_id` + a parent-derived `step_ordinal` into the hook context, (2) add an append-only SQLite journal store in `labby-gateway` mirroring `UsageStore`, (3) override `record_step` on `GatewayManager` to buffer entries in memory and flush them in one bulk insert at the run boundary (never a write on the drive loop), and (4) project the journal as a notebook + fix the now-false docs. Replay execution is explicitly out of scope (v2 epic lab-5dtw9).

**Tech Stack:** Rust 2024, tokio, rusqlite (bundled, already a workspace dep), serde_json, tracing. Test runner: `cargo-nextest`.

## Global Constraints

- **HARD CONSTRAINT:** Do NOT reintroduce the destructive-call pause/confirm/resume gate removed in commit `e3575193`. No `confirm`/`resume_token` param, no pause action, no mid-run interruption. The journal is read/replay-only and orthogonal to dispatch. (`docs/dev/CODE_MODE.md:521-527`)
- **Journal KEY = `(execution_id, step_ordinal, name)`.** `step_ordinal` is a parent-derived monotonic count of `step_begin` events, NOT the runner `seq`. The single `next_runner_seq` spine is preserved and untouched; the ordinal is a parent-side index with no wire field.
- **Journaling is FAIL-OPEN on normal runs.** A flush failure logs a warning and the run still returns success. Journaling must never alter control flow on the non-replay path.
- **No new business logic in CLI/MCP adapters.** Not applicable to v1 (no new surface), but honor it in Task 4's read helper: put projection logic in a shared helper, not `output.rs` formatting.
- **v1 overrides `record_step` ONLY.** `decide_step`, `decide_local`, `record_local` stay at their crate defaults (deferred to v2).
- **No `mod.rs` files** — a module `foo` is declared in `foo.rs` sibling to its `foo/` directory.
- **Never add `clap`/`rmcp`/`axum`/`anyhow` to `labby-apis`** (not touched here, but relevant if tempted).
- **Verification target is the all-features workspace build:** `cargo nextest run --all-features` and `just test`. Narrow `-p` runs are for fast iteration only.
- **Secrets never logged.** Redact both step `value` AND step `name` (name is caller-authored JS).

---

### Task 1: Thread `execution_id` + `step_ordinal` into the hook context

**Files:**
- Modify: `crates/labby-codemode/src/host.rs:59-71` (ExecCtx), `:142-149` (record_step signature)
- Modify: `crates/labby-codemode/src/runner_drive.rs:79` (RunnerConfig), `:675`, `:736`, `:1118-1178` (step handlers + DriveState), and `DriveState` definition
- Test: `crates/labby-codemode/src/runner_drive.rs` (inline `#[cfg(test)]` module) and `crates/labby-codemode/src/host.rs` (inline tests)

**Interfaces:**
- Produces:
  - `ExecCtx { seq: u64, execution_id: Option<Arc<str>>, step_ordinal: Option<u64> }` — now `Clone` (not `Copy`).
  - `ExecCtx::none() -> Self` (unchanged contract: write-free, `execution_id: None`).
  - `CodeModeHost::record_step(&self, ctx: ExecCtx, name: &str, value: &Value)` — **new `name` param** (mirrors `decide_step`, which already takes `name`).
  - `RunnerConfig.execution_id: Option<Arc<str>>` — set by the caller (gateway/binary); flows into every `ExecCtx`.

- [ ] **Step 1: Write the failing test for ExecCtx carrying the new fields**

Add to `crates/labby-codemode/src/host.rs` inline `#[cfg(test)]` module:

```rust
#[test]
fn exec_ctx_none_is_write_free() {
    let ctx = ExecCtx::none();
    assert_eq!(ctx.seq, 0);
    assert!(ctx.execution_id.is_none());
    assert!(ctx.step_ordinal.is_none());
}

#[test]
fn exec_ctx_carries_execution_id_and_ordinal() {
    let ctx = ExecCtx {
        seq: 7,
        execution_id: Some(std::sync::Arc::from("exec_abc")),
        step_ordinal: Some(2),
    };
    assert_eq!(ctx.seq, 7);
    assert_eq!(ctx.execution_id.as_deref(), Some("exec_abc"));
    assert_eq!(ctx.step_ordinal, Some(2));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p labby-codemode exec_ctx_ -- --nocapture`
Expected: FAIL — `ExecCtx` has no field `execution_id` / no field `step_ordinal`.

- [ ] **Step 3: Extend `ExecCtx`**

In `crates/labby-codemode/src/host.rs`, replace the struct + `none()` (lines 59-71):

```rust
/// Per-call execution context threaded from the runner drive layer into
/// [`CodeModeHost`] hooks. Carries the protocol `seq` for this call, the
/// durable-run `execution_id` (None on the write-free/standalone path), and,
/// for a `codemode.step` boundary, the parent-derived `step_ordinal` (a
/// monotonic count of step_begin events — the journal key ordinate, distinct
/// from `seq`).
#[derive(Debug, Clone)]
pub struct ExecCtx {
    pub seq: u64,
    pub execution_id: Option<std::sync::Arc<str>>,
    pub step_ordinal: Option<u64>,
}

impl ExecCtx {
    /// The write-free context used when no durable run is active.
    #[must_use]
    pub fn none() -> Self {
        Self { seq: 0, execution_id: None, step_ordinal: None }
    }
}
```

Note: `none()` loses `const` because `Arc` isn't const-constructible here; that is fine — grep for `ExecCtx::none()` callers; none require const.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p labby-codemode exec_ctx_ -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Update `record_step` trait signature to take `name`**

In `crates/labby-codemode/src/host.rs`, change the `record_step` default (around line 142):

```rust
    /// Record the value a step's `fn` produced (decision was execute) so a
    /// later resume replays it without re-running `fn`. `name` + `ctx.step_ordinal`
    /// + `ctx.execution_id` form the journal key.
    ///
    /// The default impl is a no-op `Ok(())`.
    fn record_step(
        &self,
        ctx: ExecCtx,
        name: &str,
        value: &Value,
    ) -> impl Future<Output = Result<(), ToolError>> + Send {
        let _ = (ctx, name, value);
        async { Ok(()) }
    }
```

- [ ] **Step 6: Add step-ordinal tracking to `DriveState` and thread `execution_id`**

In `crates/labby-codemode/src/runner_drive.rs`: add fields to `DriveState` (find `struct DriveState`) and initialize them where `DriveState` is constructed:

```rust
    // in struct DriveState:
    /// Monotonic count of `step_begin` events seen so far (the journal ordinate).
    next_step_ordinal: u64,
    /// Maps a step's runner `seq` -> (step_ordinal, name), populated at
    /// step_begin and read at step_result (which reuses the step_begin seq).
    step_ordinals: std::collections::HashMap<u64, (u64, String)>,
```

Initialize both (`next_step_ordinal: 0`, `step_ordinals: HashMap::new()`) in the `DriveState` constructor/`Default`.

Add `execution_id: Option<Arc<str>>` to `RunnerConfig` (line 79 area) and make the drive loop carry it into `ExecCtx`. At each of the four `ExecCtx { seq }` sites (`:675`, `:736`, and the two step handlers), build with the config's execution_id (thread `cfg.execution_id.clone()` into scope where needed).

- [ ] **Step 7: Populate ordinal at step_begin, look it up at step_result**

Replace the `ExecCtx { seq }` construction in `handle_step_begin_event` (`:1128`):

```rust
    let step_ordinal = state.next_step_ordinal;
    state.next_step_ordinal += 1;
    state.step_ordinals.insert(seq, (step_ordinal, name.clone()));
    let ctx = ExecCtx {
        seq,
        execution_id: execution_id.clone(),
        step_ordinal: Some(step_ordinal),
    };
```

(`execution_id: Option<Arc<str>>` must be passed into `handle_step_begin_event`/`handle_step_result_event` — add it as a parameter sourced from `RunnerConfig`.)

Replace the `ExecCtx { seq }` construction in `handle_step_result_event` (`:1165`) and the `record_step` call:

```rust
    let (step_ordinal, name) = state
        .step_ordinals
        .get(&seq)
        .cloned()
        .unwrap_or((state.next_step_ordinal, String::new()));
    let ctx = ExecCtx {
        seq,
        execution_id: execution_id.clone(),
        step_ordinal: Some(step_ordinal),
    };
    let record = match broker.host {
        Some(host) => host.record_step(ctx, &name, &value).await,
        None => Ok(()),
    };
```

For the non-step `ExecCtx { seq }` at `:675`/`:736` (tool-call / local paths), set `execution_id: execution_id.clone(), step_ordinal: None`.

- [ ] **Step 8: Write the failing test for ordinal contiguity across interleaved calls**

Add an inline test in `runner_drive.rs`'s test module that drives a scripted `step_begin(seqA), tool_call(seqA+1), step_begin(seqA+2)` sequence against a recording stub host and asserts the two steps get `step_ordinal` 0 and 1 regardless of the seq gap. Use the existing test harness pattern in that file (search for existing `DriveState` tests / stub host). Concretely, assert the stub host recorded `record_step` calls with ordinals `[0, 1]` and names matching, even though seqs were non-contiguous.

```rust
#[tokio::test]
async fn step_ordinal_is_contiguous_across_interleaved_tool_calls() {
    // Arrange: a RecordingHost capturing (execution_id, step_ordinal, name) per record_step.
    // Drive: step_begin(seq=5,"a") -> tool_call(seq=6) -> step_result(seq=5) ->
    //        step_begin(seq=7,"b") -> step_result(seq=7)
    // Assert: recorded ordinals == [0, 1], names == ["a","b"], execution_id == Some("exec_test").
}
```

(Use/extend the file's existing scripted-runner test utilities; do not invent a new harness if one exists.)

- [ ] **Step 9: Run tests to verify pass; verify single-spine invariant untouched**

Run: `cargo nextest run -p labby-codemode`
Expected: PASS including any existing `next_runner_seq` / single-spine invariant tests. If a `record_step` caller elsewhere breaks on the new `name` param, fix the call site (only the two `handle_step_result_event` paths and any test stubs should call it).

- [ ] **Step 10: Clippy + commit**

```bash
cargo clippy -p labby-codemode --all-features -- -D warnings
git add crates/labby-codemode/src/host.rs crates/labby-codemode/src/runner_drive.rs
git commit -m "feat(codemode): thread execution_id + step_ordinal into hook context (lab-d6ke7.1)"
```

---

### Task 2: Append-only step-journal SQLite store in `labby-gateway`

**Files:**
- Create: `crates/labby-gateway/src/codemode_journal.rs` (module decl + `StepJournalRow`)
- Create: `crates/labby-gateway/src/codemode_journal/store.rs` (`StepJournalStore`)
- Modify: `crates/labby-gateway/src/lib.rs` (add `pub mod codemode_journal;`)
- Test: `crates/labby-gateway/src/codemode_journal/store.rs` inline `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `StepJournalRow { execution_id: String, step_ordinal: u64, seq_base: u64 /* the step_begin runner seq, for call-to-cell attribution */, name: String, value: String /* redacted JSON text */, ok: bool, elapsed_ms: u128, recorded_at: i64, actor_key: Option<String>, route_scope: String, capability_filter_fingerprint: Option<String>, replayed_from: Option<String> }`
  - `StepJournalStore::open(path: PathBuf) -> Result<Self, ToolError>` (async)
  - `StepJournalStore::flush(&self, rows: Vec<StepJournalRow>) -> Result<(), ToolError>` (async; ONE transaction, multi-row insert)
  - `StepJournalStore::load(&self, execution_id: &str) -> Result<Vec<StepJournalRow>, ToolError>` (async; ONE `SELECT ... ORDER BY step_ordinal`)
  - `StepJournalStore::prune_older_than(&self, cutoff_unix: i64) -> Result<usize, ToolError>` (async; batched)
  - `redact_journal_text(raw: &Value, cap_bytes: usize) -> String` — early-aborting bounded serialize + secret-shaped redaction.

- [ ] **Step 1: Write the failing test for buffer→flush→load round-trip**

Create `crates/labby-gateway/src/codemode_journal/store.rs` with an inline test first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn row(exec: &str, ord: u64, name: &str) -> StepJournalRow {
        StepJournalRow {
            execution_id: exec.into(), step_ordinal: ord, seq_base: ord * 3, name: name.into(),
            value: "\"v\"".into(), ok: true, elapsed_ms: 1, recorded_at: 100,
            actor_key: Some("actor1".into()), route_scope: "default".into(),
            capability_filter_fingerprint: None, replayed_from: None,
        }
    }

    #[tokio::test]
    async fn flush_then_load_returns_rows_in_ordinal_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = StepJournalStore::open(dir.path().join("journal.db")).await.unwrap();
        store.flush(vec![row("e1", 1, "b"), row("e1", 0, "a")]).await.unwrap();
        let got = store.load("e1").await.unwrap();
        assert_eq!(got.iter().map(|r| r.step_ordinal).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(got[0].name, "a");
        assert!(store.load("missing").await.unwrap().is_empty());
    }
}
```

Confirm `tempfile` is a dev-dependency of `labby-gateway` (it is used by other stores); if not, add it to `[dev-dependencies]`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p labby-gateway codemode_journal`
Expected: FAIL — module/type does not exist.

- [ ] **Step 3: Define the row type and module**

Create `crates/labby-gateway/src/codemode_journal.rs`:

```rust
//! Append-only durable journal of `codemode.step` boundaries. Read/replay-only:
//! this store never gates or pauses a run (see docs/dev/CODE_MODE.md:521-527).
//! Owner-identity columns (actor_key/route_scope/capability_filter_fingerprint)
//! and `replayed_from` are persisted for the v2 replay-auth path (epic
//! lab-5dtw9) even though v1 never reads them.

pub mod store;

pub use store::StepJournalStore;

/// One persisted `codemode.step` boundary. `value` is redacted, bounded JSON text.
#[derive(Debug, Clone, PartialEq)]
pub struct StepJournalRow {
    pub execution_id: String,
    pub step_ordinal: u64,
    pub name: String,
    pub value: String,
    pub ok: bool,
    pub elapsed_ms: u128,
    pub recorded_at: i64,
    pub actor_key: Option<String>,
    pub route_scope: String,
    pub capability_filter_fingerprint: Option<String>,
    pub replayed_from: Option<String>,
}
```

Add `pub mod codemode_journal;` to `crates/labby-gateway/src/lib.rs` (alphabetical with siblings).

- [ ] **Step 4: Implement the store by mirroring `UsageStore`**

In `crates/labby-gateway/src/codemode_journal/store.rs`, mirror `crates/labby-gateway/src/usage/store.rs` exactly for the pool/pragma/perms scaffolding (`SQLITE_BUSY_TIMEOUT_MS = 5_000`, `SQLITE_POOL_SIZE = 4`, `SCHEMA_VERSION = 1`, `PRUNE_BATCH_SIZE = 5_000`, `open_connections`, `ensure_restrictive_permissions`, `open_connection` with WAL + `synchronous=NORMAL` + `PRAGMA user_version`). Copy those private helpers verbatim (they are file-private in `usage/store.rs`; duplicating the ~40 lines is correct — do not make `usage`'s private helpers pub). Schema:

```rust
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
```

`flush` — one transaction, one prepared statement reused per row, all values bound via `params![]` (NEVER `format!` into SQL):

```rust
pub async fn flush(&self, rows: Vec<StepJournalRow>) -> Result<(), ToolError> {
    if rows.is_empty() { return Ok(()); }
    let conn = self.conn(); // Arc<Mutex<Connection>> round-robin like UsageStore
    tokio::task::spawn_blocking(move || {
        let mut guard = conn.lock().expect("journal conn mutex poisoned");
        let tx = guard.transaction().map_err(map_sqlite)?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO step_journal \
                 (execution_id, step_ordinal, seq_base, name, value, ok, elapsed_ms, recorded_at, \
                  actor_key, route_scope, capability_filter_fingerprint, replayed_from) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            ).map_err(map_sqlite)?;
            for r in &rows {
                stmt.execute(params![
                    r.execution_id, r.step_ordinal as i64, r.seq_base as i64, r.name, r.value,
                    r.ok as i64, r.elapsed_ms as i64, r.recorded_at,
                    r.actor_key, r.route_scope, r.capability_filter_fingerprint, r.replayed_from,
                ]).map_err(map_sqlite)?;
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(())
    }).await.map_err(|e| storage_error(format!("journal flush task failed: {e}")))?
}
```

`load` — one `SELECT ... WHERE execution_id = ?1 ORDER BY step_ordinal ASC`, mapping rows back to `StepJournalRow`. `prune_older_than` — copy `UsageStore::prune_older_than`'s batched `DELETE ... WHERE recorded_at < ?1 LIMIT PRUNE_BATCH_SIZE` loop. Reuse `map_sqlite`/`storage_error`/`ToolError` helpers analogous to `usage/store.rs` (define local copies matching its `storage_error`).

- [ ] **Step 5: Run round-trip test to verify pass**

Run: `cargo nextest run -p labby-gateway codemode_journal`
Expected: PASS.

- [ ] **Step 6: Write the failing test for redaction + bounded serialize**

```rust
#[test]
fn redact_journal_text_bounds_and_redacts() {
    // Oversize: a huge array must not be fully materialized; result is capped/sentinel.
    let big = serde_json::json!(vec!["x".repeat(1024); 1024]);
    let out = redact_journal_text(&big, 4096);
    assert!(out.len() <= 4096 + 64, "must be bounded near cap, got {}", out.len());
    // Secret-shaped: a value that looks like a token is masked.
    let secret = serde_json::json!({"authorization": "Bearer sk-abcdef1234567890"});
    let red = redact_journal_text(&secret, 4096);
    assert!(!red.contains("sk-abcdef1234567890"), "token must be redacted: {red}");
}
```

- [ ] **Step 7: Run to verify it fails**

Run: `cargo nextest run -p labby-gateway redact_journal_text`
Expected: FAIL — `redact_journal_text` not defined.

- [ ] **Step 8: Implement `redact_journal_text`**

In `codemode_journal/store.rs`. Reuse the existing secret-shaped redactor — search `labby-codemode`/`labby-runtime` for `redact_secret_like_segments` (referenced in `crates/labby-codemode/src/truncate.rs:42`) and call it after a bounded serialize. Bounded serialize = serialize to a `Vec<u8>` writer that returns early once it exceeds `cap_bytes`, then emit a sentinel:

```rust
pub fn redact_journal_text(raw: &Value, cap_bytes: usize) -> String {
    // 1. Serialize with an early-abort writer bounded at cap_bytes.
    let mut buf = Vec::with_capacity(cap_bytes.min(4096));
    let bounded = {
        let mut ser = serde_json::Serializer::new(BoundedWriter::new(&mut buf, cap_bytes));
        raw.serialize(&mut ser).is_ok()
    };
    if !bounded {
        return format!("{{\"__journal_truncated\":true,\"cap_bytes\":{cap_bytes}}}");
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    // 2. Redact secret-shaped segments (reuse the crate's existing redactor).
    redact_secret_like_segments(&text)
}
```

Implement `BoundedWriter` (an `io::Write` that errors once `written > cap`). If `redact_secret_like_segments` is not `pub`-reachable from `labby-gateway`, add a minimal local port or make the source fn `pub(crate)` and re-export — prefer reuse; only port if it would require a cross-crate API change larger than the redactor itself.

- [ ] **Step 9: Run tests + perms assertion**

Add a test asserting the db file is `0600` after `open` (mirror `usage/store.rs`'s perms test if present). Then:

Run: `cargo nextest run -p labby-gateway codemode_journal`
Expected: PASS (round-trip, redaction, perms).

- [ ] **Step 10: Clippy + commit**

```bash
cargo clippy -p labby-gateway --all-features -- -D warnings
git add crates/labby-gateway/src/codemode_journal.rs crates/labby-gateway/src/codemode_journal/store.rs crates/labby-gateway/src/lib.rs
git commit -m "feat(gateway): append-only codemode step-journal store (lab-d6ke7.2)"
```

---

### Task 3: `record_step` override on `GatewayManager` + buffered flush at the run boundary

**Files:**
- Modify: `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs:29-244` (add `record_step` impl; add per-execution buffer access)
- Modify: `crates/labby-gateway/src/gateway/manager.rs` (or wherever `GatewayManager`'s fields live — add the buffer + `StepJournalStore`) and the gateway `code_mode` `execute()` wrapper (flush at boundary)
- Test: inline `#[cfg(test)]` in `code_mode_host.rs`

**Interfaces:**
- Consumes: `ExecCtx` (Task 1), `StepJournalStore`/`StepJournalRow`/`redact_journal_text` (Task 2).
- Produces:
  - `GatewayManager` gains `step_journal: Option<Arc<StepJournalStore>>` and `step_buffers: Arc<Mutex<HashMap<String /*execution_id*/, Vec<StepJournalRow>>>>`.
  - `GatewayManager::flush_step_journal(&self, execution_id: &str, owner: &JournalOwner)` — called once at run completion.
  - `JournalOwner { actor_key: Option<String>, route_scope: String, capability_filter_fingerprint: Option<String> }` — the run's caller identity, captured from the existing per-run caller context.

- [ ] **Step 1: Write the failing test — record_step buffers, flush persists, happy path unchanged**

Add inline test in `code_mode_host.rs` using an in-memory/temp `StepJournalStore`:

```rust
#[tokio::test]
async fn record_step_buffers_then_flush_persists() {
    let mgr = test_manager_with_journal().await; // helper: GatewayManager wired to a temp StepJournalStore
    let exec = std::sync::Arc::<str>::from("exec_t1");
    let ctx = ExecCtx { seq: 3, execution_id: Some(exec.clone()), step_ordinal: Some(0) };
    mgr.record_step(ctx, "fetch", &serde_json::json!({"id": 7})).await.unwrap();
    // Nothing on disk yet (buffered):
    assert!(mgr.step_journal().unwrap().load("exec_t1").await.unwrap().is_empty());
    // Flush at boundary:
    mgr.flush_step_journal("exec_t1", &JournalOwner {
        actor_key: Some("a".into()), route_scope: "default".into(), capability_filter_fingerprint: None,
    }).await;
    let rows = mgr.step_journal().unwrap().load("exec_t1").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "fetch");
    assert_eq!(rows[0].step_ordinal, 0);
    assert_eq!(rows[0].actor_key.as_deref(), Some("a"));
}

#[tokio::test]
async fn record_step_none_execution_id_is_noop() {
    let mgr = test_manager_with_journal().await;
    let ctx = ExecCtx { seq: 1, execution_id: None, step_ordinal: Some(0) };
    mgr.record_step(ctx, "x", &serde_json::json!(1)).await.unwrap();
    // No buffer entry created for a None execution_id.
    assert!(mgr.step_buffer_is_empty()); // test-only accessor
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p labby-gateway record_step_`
Expected: FAIL — `record_step` not overridden / helpers absent.

- [ ] **Step 3: Add the buffer + store fields to `GatewayManager`**

Add to the `GatewayManager` struct: `step_journal: Option<Arc<StepJournalStore>>` and `step_buffers: Arc<Mutex<std::collections::HashMap<String, Vec<StepJournalRow>>>>`. Initialize `step_buffers` to empty; wire `step_journal` from gateway config/data-dir at manager construction (open `StepJournalStore` at `<gateway_data_dir>/codemode_journal.db`, mirroring where `UsageStore` is opened). If journaling is unconfigured, `step_journal = None` (pure no-op path).

Define `JournalOwner` in `code_mode_host.rs` (or a small sibling), and a test-only `test_manager_with_journal()` + `step_buffer_is_empty()`/`step_journal()` accessors behind `#[cfg(test)]`.

- [ ] **Step 4: Implement `record_step` (buffer only, nanoseconds, no I/O)**

In `impl CodeModeHost for GatewayManager`:

```rust
async fn record_step(&self, ctx: ExecCtx, name: &str, value: &Value) -> Result<(), ToolError> {
    // Fail-open + write-free unless a durable run is active AND journaling is configured.
    let (Some(execution_id), Some(ordinal), Some(_store)) =
        (ctx.execution_id.as_ref(), ctx.step_ordinal, self.step_journal.as_ref())
    else {
        return Ok(());
    };
    let row = StepJournalRow {
        execution_id: execution_id.to_string(),
        step_ordinal: ordinal,
        seq_base: ctx.seq,
        name: redact_secret_like_segments(name),
        value: redact_journal_text(value, JOURNAL_VALUE_CAP_BYTES),
        ok: true,
        elapsed_ms: 0, // populated at flush if available; 0 acceptable in v1
        recorded_at: unix_now(),
        actor_key: None, // owner identity is stamped at flush from the run context
        route_scope: String::new(),
        capability_filter_fingerprint: None,
        replayed_from: None,
    };
    self.step_buffers
        .lock()
        .expect("step_buffers mutex poisoned")
        .entry(execution_id.to_string())
        .or_default()
        .push(row);
    Ok(())
}
```

Define `JOURNAL_VALUE_CAP_BYTES` (e.g. `64 * 1024`, matching the history byte-cap spirit) and `unix_now()`. Note `record_step` returns `Ok(())` on every path — it can never fail the run (fail-open; buffer push is infallible barring a poisoned mutex).

- [ ] **Step 5: Implement `flush_step_journal` (one bulk insert, fail-open) and call it at the run boundary**

```rust
pub async fn flush_step_journal(&self, execution_id: &str, owner: &JournalOwner) {
    let Some(store) = self.step_journal.as_ref() else { return; };
    let mut rows = {
        let mut buffers = self.step_buffers.lock().expect("step_buffers mutex poisoned");
        buffers.remove(execution_id).unwrap_or_default()
    };
    if rows.is_empty() { return; }
    for r in &mut rows {
        r.actor_key = owner.actor_key.clone();
        r.route_scope = owner.route_scope.clone();
        r.capability_filter_fingerprint = owner.capability_filter_fingerprint.clone();
    }
    if let Err(err) = store.flush(rows).await {
        // FAIL-OPEN: journaling is orthogonal to dispatch; a lost journal only
        // costs future replay completeness, never the run's success.
        tracing::warn!(
            surface = "gateway", service = "codemode", execution_id,
            kind = %err.kind(), "step journal flush failed (fail-open)"
        );
    }
}
```

Call `flush_step_journal(&execution_id, &owner)` in the gateway `code_mode` `execute()` wrapper (the fn that brackets the run and returns the response), AFTER `execute()` returns and BEFORE returning the response — regardless of success/failure of the run. Source `execution_id` + `owner` from the same per-run caller context the binary already builds (the one that later stamps `CodeModeHistoryEntry.execution_id`).

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo nextest run -p labby-gateway record_step_`
Expected: PASS (buffer, flush, None no-op).

- [ ] **Step 7: Write + run the fail-open flush test**

```rust
#[tokio::test]
async fn flush_failure_is_fail_open() {
    // Wire a store whose flush errors (e.g. closed/invalid path), buffer one row,
    // call flush_step_journal, assert it does NOT panic/propagate and buffer is drained.
    let mgr = test_manager_with_failing_journal().await;
    let ctx = ExecCtx { seq: 1, execution_id: Some("e".into()), step_ordinal: Some(0) };
    mgr.record_step(ctx, "s", &serde_json::json!(1)).await.unwrap();
    mgr.flush_step_journal("e", &JournalOwner::default()).await; // must not panic
    assert!(mgr.step_buffer_is_empty());
}
```

Run: `cargo nextest run -p labby-gateway flush_failure_is_fail_open`
Expected: PASS.

- [ ] **Step 8: Golden — happy-path run response is byte-identical with journaling on**

Add/extend an existing gateway code-mode execution test: run a snippet with two `codemode.step` calls through the full path with journaling configured, and assert the returned `CodeModeResponse` (result/calls/logs) is identical to the same run with `step_journal = None`. Assert no `spawn_blocking`/DB work happens during step handling (structural: `record_step` contains no `.await` on the store — enforce by code review + the buffer-only impl).

Run: `cargo nextest run -p labby-gateway --all-features code_mode`
Expected: PASS.

- [ ] **Step 9: Clippy + commit**

```bash
cargo clippy -p labby-gateway --all-features -- -D warnings
git add crates/labby-gateway/src/gateway/
git commit -m "feat(gateway): record_step buffer + run-boundary flush, fail-open (lab-d6ke7.3)"
```

---

### Task 4: Notebook projection + CODE_MODE.md doc rewrite + observability

**Files:**
- Create: `crates/labby-gateway/src/codemode_journal/notebook.rs` (projection helper) + declare `pub mod notebook;` in `codemode_journal.rs`
- Modify: `docs/dev/CODE_MODE.md:521-527` (reword) and `crates/labby-codemode/src/runner_drive.rs:1110-1116` (stale comment)
- Modify: `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs` (tracing on record_step buffer + flush already added in Task 3 — add the buffer-event trace here)
- Modify: `crates/labby/src/output.rs` OR a dispatch read action (see below) for rendering
- Test: inline `#[cfg(test)]` in `notebook.rs`

**Interfaces:**
- Consumes: `StepJournalRow` (Task 2), `CodeModeExecutedCall`/`CodeModeHistoryEntry` (existing, `crates/labby-codemode/src/types.rs:426-451`).
- Produces:
  - `Notebook { cells: Vec<NotebookCell>, truncated: bool }`, `NotebookCell { ordinal: Option<u64>, name: Option<String>, value: Option<String>, calls: Vec<CallSummary>, elapsed_ms: u128, executed: bool }` (a prologue cell has `ordinal: None`).
  - `project_notebook(rows: &[StepJournalRow], calls: &[CodeModeExecutedCall], max_cells: usize, max_bytes: usize) -> Notebook` — pure fn; groups calls into the step-cell whose seq span contains the call's seq; caps `cells`.

- [ ] **Step 1: Write the failing test for cell projection + size cap**

Create `crates/labby-gateway/src/codemode_journal/notebook.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projects_calls_into_step_cells_by_seq_span() {
        // rows: step ordinal 0 "a" (seq base 5), ordinal 1 "b" (seq base 8)
        // calls at seq 6 (in a's span), seq 9 (in b's span), seq 2 (prologue)
        let nb = project_notebook(&rows_fixture(), &calls_fixture(), 100, 1_000_000);
        assert_eq!(nb.cells[0].ordinal, None);       // prologue holds seq-2 call
        assert_eq!(nb.cells[1].name.as_deref(), Some("a"));
        assert_eq!(nb.cells[1].calls.len(), 1);
        assert_eq!(nb.cells[2].name.as_deref(), Some("b"));
        assert!(nb.cells.iter().all(|c| c.executed)); // v1: everything executed
    }
    #[test]
    fn caps_cell_count() {
        let many = (0..500).map(|i| row_at(i)).collect::<Vec<_>>();
        let nb = project_notebook(&many, &[], 50, 1_000_000);
        assert!(nb.cells.len() <= 50 && nb.truncated);
    }
}
```

Note: call-to-cell attribution uses each row's `seq_base` (the step_begin runner seq, persisted by Task 2's schema and set from `ctx.seq` in Task 3). A call attaches to the last cell whose `seq_base <= call.seq`; calls before the first cell's `seq_base` form the prologue.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo nextest run -p labby-gateway notebook`
Expected: FAIL — `project_notebook` undefined.

- [ ] **Step 3: Implement `project_notebook`**

Pure function: sort rows by `step_ordinal`; build one `NotebookCell` per row (`executed: true` in v1); a leading prologue cell collects calls whose seq precedes the first step's `seq_base`; each call attaches to the last cell whose `seq_base <= call.seq`; stop adding cells at `max_cells` and set `truncated = true`; also stop if accumulated serialized bytes exceed `max_bytes`. No DB access — operates on the already-loaded slices.

- [ ] **Step 4: Run to verify pass**

Run: `cargo nextest run -p labby-gateway notebook`
Expected: PASS.

- [ ] **Step 5: Add the record_step buffer tracing event**

In `code_mode_host.rs` `record_step`, before returning `Ok(())` on the journaling path, emit:

```rust
tracing::debug!(
    surface = "gateway", service = "codemode",
    execution_id = %execution_id, step_ordinal = ordinal,
    "codemode.step journaled"  // name/value NOT logged (redacted content, and unnecessary)
);
```

Confirm the Task-3 flush warning already redacts (it logs only `execution_id` + `kind`, never name/value). Add a redaction assertion test: journal a step whose name/value are secret-shaped, capture nothing secret is emitted (assert the row's stored `name`/`value` are redacted — the log deliberately omits both).

- [ ] **Step 6: Reword the docs (NOT deferrable)**

In `docs/dev/CODE_MODE.md` around lines 521-527, the text currently asserts *"There is no durable execution log, no `resume_token`, and no `confirm` parameter."* Replace the first clause:

```markdown
Code Mode persists a **durable, read/replay-only step journal** of every
`codemode.step(name, fn)` boundary (append-only, owner-scoped, redacted at
rest). It has **no** `resume_token` and **no** `confirm` parameter on the
`codemode` MCP tool, and **no** pause/resume/reject mechanism: the journal is
orthogonal to dispatch and never interrupts, gates, or confirms a running
snippet. This preserves the permanent decision to remove the destructive-call
pause gate — the journal is a record, not a gate.
```

Then update the stale comment in `crates/labby-codemode/src/runner_drive.rs:1110-1116` that frames a seq shift as `resume_divergence` — reword it to note the seq spine is used for intra-run attribution only, and that cross-run replay (v2) keys on `step_ordinal`, not seq.

- [ ] **Step 7: Wire a human/JSON render (minimal)**

Add a `--json`-able rendering of `Notebook` in `crates/labby/src/output.rs` (or a small read action under `crates/labby/src/dispatch/codemode/` if one exists) that serializes `Notebook`. Keep it thin — the projection logic stays in `notebook.rs`. If there is no existing codemode read surface to hang this on in v1, expose `project_notebook` as a `pub` helper + a `Serialize` impl on `Notebook` and add a unit test that it serializes; a full CLI/MCP surface for it is optional v1 polish, not required.

- [ ] **Step 8: Full-workspace verification**

Run: `cargo nextest run --all-features`
Expected: PASS (whole workspace).
Run: `just lint`
Expected: clippy clean + fmt clean.
Run doc checks if present: `just test` (includes doc-freshness) — confirm `CODE_MODE.md` no longer claims "no durable execution log" and still contains the no-pause language.

- [ ] **Step 9: Commit**

```bash
git add crates/labby-gateway/src/codemode_journal/notebook.rs crates/labby-gateway/src/codemode_journal.rs docs/dev/CODE_MODE.md crates/labby-codemode/src/runner_drive.rs crates/labby/src/output.rs
git commit -m "feat(codemode): notebook projection + doc rewrite + journal observability (lab-d6ke7.5)"
```

---

## Self-Review

**Spec coverage (v1 epic lab-d6ke7 + .1/.2/.3/.5):**
- .1 execution_id + step_ordinal threading → Task 1 ✓ (ExecCtx fields, DriveState ordinal, record_step `name` param, None no-op).
- .2 append-only store, M2 flush, dedicated row, forward-compat owner columns, redaction, prune, params-bind, 0600 → Task 2 ✓.
- .3 record_step-only override, in-mem buffer, boundary flush, fail-open, per-execution-keyed (not a shared scalar) → Task 3 ✓. (`decide_step`/`decide_local`/`record_local` left at defaults ✓.)
- .5 projection (size-capped, all-executed), doc reword + stale comment, observability redacting name+value → Task 4 ✓.
- Deferred: replay (`decide_step`→Replay, cursor surface, replay-auth) — not present in any task ✓ (belongs to v2 lab-5dtw9).

**Placeholder scan:** No "TODO/handle edge cases" left; each code step shows real Rust. One deliberate implementation decision (persist `seq_base` for call attribution) is called out in Task 4 Step 1 with a concrete resolution.

**Type consistency:** `ExecCtx { seq, execution_id, step_ordinal }` used identically in Tasks 1/3. `record_step(ctx, name, value)` signature consistent (Task 1 defines, Task 3 implements). `StepJournalRow` fields identical across Tasks 2/3/4. `StepJournalStore::{open,flush,load,prune_older_than}` consistent. `JournalOwner` fields consistent Tasks 3/4.

**Call-attribution key:** `seq_base` is in `StepJournalRow` + the `step_journal` schema from Task 2 (`SCHEMA_VERSION = 1`, column present), set from `ctx.seq` in Task 3's `record_step`, and consumed by `project_notebook` in Task 4 — consistent across all three tasks, no later schema bump needed.
