# Service Layer First Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce the shared `crates/lab/src/services` layer and migrate the first vertical slice so ByteStash no longer depends on MCP-owned dispatch, while also extracting the shared CLI param parser and HTTP API dispatch wrapper.

**Architecture:** This slice establishes the target dependency direction without trying to migrate every service at once. First extract the obvious shared helpers, then add the new `services` layer with a neutral error/result contract, then move ByteStash onto it end to end, and finally point the MCP and HTTP API adapters at the shared implementation.

**Tech Stack:** Rust 2024, `clap`, `serde_json`, `axum`, `tracing`, `lab-apis`, existing MCP envelope and HTTP API layers

---

## File Structure

This slice should introduce and touch the following files.

**Create:**
- `crates/lab/src/services.rs`
- `crates/lab/src/services/context.rs`
- `crates/lab/src/services/params.rs`
- `crates/lab/src/services/bytestash.rs`
- `crates/lab/src/api/services/helpers.rs`

**Modify:**
- `crates/lab/src/cli/bytestash.rs`
- `crates/lab/src/cli/unifi.rs`
- `crates/lab/src/mcp/services/bytestash.rs`
- `crates/lab/src/cli/serve.rs` (must remove `mcp::services::bytestash::dispatch` reference — see Task 4)
- `crates/lab/src/api/services/bytestash.rs`
- `crates/lab/src/api/services.rs`
- `crates/lab/src/CLAUDE.md` only if implementation details require a follow-up clarification
- `docs/coverage/bytestash.md`
- `docs/SERVICE_LAYER_MIGRATION.md` only if the implementation differs from the planned target

**Test targets:**
- `crates/lab/src/mcp/services/bytestash.rs`
- `crates/lab/src/cli/bytestash.rs`
- `crates/lab/src/cli/unifi.rs`
- `crates/lab/src/api/services/bytestash.rs`
- `crates/lab/src/api/services/helpers.rs`

## Constraints

- Do not move upstream request-building or response parsing out of `lab-apis`.
- Do not let CLI call MCP dispatch after this slice for ByteStash.
- Do not let HTTP API call MCP dispatch after this slice for ByteStash.
- Keep result shapes behaviorally stable for existing MCP and HTTP API callers.
- Keep ByteStash CLI action-style for this slice. Typed CLI standardization is intentionally deferred.
- Prefer `serde_json::Value` as the initial shared result type to reduce migration churn.

### Task 1: Extract Shared Action-Style CLI Param Parsing

**Files:**
- Create: `crates/lab/src/services/params.rs`
- Modify: `crates/lab/src/cli/bytestash.rs`
- Modify: `crates/lab/src/cli/unifi.rs`
- Test: `crates/lab/src/services/params.rs` or an existing nearby test module

- [ ] **Step 1: Write failing unit tests for the shared parser**

Add focused tests for:

- `key=value` parsing into a JSON object
- boolean coercion
- integer coercion
- float coercion
- invalid input without `=`

Suggested cases:

```rust
#[test]
fn parse_kv_params_coerces_scalars() {
    let value = parse_kv_params(vec![
        "enabled=true".to_string(),
        "count=7".to_string(),
        "ratio=1.5".to_string(),
        "name=alice".to_string(),
    ]).unwrap();

    assert_eq!(value["enabled"], true);
    assert_eq!(value["count"], 7);
    assert_eq!(value["ratio"], 1.5);
    assert_eq!(value["name"], "alice");
}

#[test]
fn parse_kv_params_rejects_missing_equals() {
    let err = parse_kv_params(vec!["broken".to_string()]).unwrap_err();
    assert!(err.to_string().contains("expected key=value"));
}
```

- [ ] **Step 2: Run the targeted test to verify it fails**

Run:

```bash
cargo test -p lab parse_kv_params -- --nocapture
```

Expected:

- test failure because the shared helper does not exist yet

- [ ] **Step 3: Implement the shared helper**

In `crates/lab/src/services/params.rs`, add:

- `parse_kv_params(params: Vec<String>) -> anyhow::Result<Value>`
- a private `coerce_value(raw: &str) -> Value`

Keep the behavior identical to the current ByteStash and UniFi implementations.

- [ ] **Step 4: Switch ByteStash and UniFi CLI to the shared helper**

Replace the local duplicated `parse_params` and `coerce_value` functions in:

- `crates/lab/src/cli/bytestash.rs`
- `crates/lab/src/cli/unifi.rs`

Each file should call the shared helper instead.

- [ ] **Step 5: Run the targeted tests to verify the helper and both CLIs pass**

Run:

```bash
cargo test -p lab parse_kv_params -- --nocapture
cargo test -p lab bytestash -- --nocapture
cargo test -p lab unifi -- --nocapture
```

Expected:

- helper tests pass
- no regressions in affected CLI tests

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/services.rs crates/lab/src/services/params.rs crates/lab/src/cli/bytestash.rs crates/lab/src/cli/unifi.rs
git commit -m "refactor: extract shared action cli param parser"
```

### Task 2: Add Shared HTTP API Dispatch Wrapper

> **Engineering review decision:** `handle_action` is implemented in this slice because it is the correct enforcement point for the destructive confirmation gate (a HIGH security finding — the gate is specified in `api/CLAUDE.md` but was not implemented). Radarr and UniFi HTTP handler migration is **deferred** to a follow-up slice; only ByteStash migrates here.

**Files:**
- Create: `crates/lab/src/api/services/helpers.rs`
- Modify: `crates/lab/src/api/services.rs`
- Modify: `crates/lab/src/api/services/bytestash.rs`
- Test: `crates/lab/src/api/services/helpers.rs`

- [ ] **Step 1: Write failing tests for the shared HTTP API wrapper**

Add tests covering:

- success path returns `Json<Value>`
- error path preserves `ToolError::kind()`
- destructive action without `confirm: true` returns `ToolError::ConfirmationRequired`
- destructive action with `confirm: true` in params proceeds to dispatch
- dispatch logging wrapper compiles with a closure-based dispatch call

Keep the tests narrow and local to the helper module.

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test -p lab api::services::helpers -- --nocapture
```

Expected:

- failure because the helper module does not exist yet

- [ ] **Step 3: Implement the shared helper**

Create a helper with this shape:

```rust
pub async fn handle_action<F, Fut>(
    service: &'static str,
    req: ActionRequest,
    actions: &[ActionSpec],  // used to check ActionSpec.destructive
    dispatch: F,
) -> Result<Json<Value>, ToolError>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, ToolError>>,
```

It should own:

- destructive confirmation gate: if the matched `ActionSpec.destructive == true`, require `params["confirm"] == true`. Return `ToolError::ConfirmationRequired` otherwise.
- timer start/stop (timer must wrap the full dispatch call including all error paths)
- dispatch logging — **IMPORTANT: log `action` and `elapsed_ms` only; never log `params` as values may contain credentials** (see `docs/OBSERVABILITY.md`)
- JSON response wrapping

It should not own:

- axum routing
- request extraction
- service-specific execution

- [ ] **Step 4: Migrate the ByteStash HTTP API handler to the helper**

Update `crates/lab/src/api/services/bytestash.rs` so it becomes a small adapter that passes `service`, `req`, `ACTIONS`, and a dispatch closure to the shared helper.

Radarr and UniFi HTTP handler migration is intentionally deferred — they will migrate when those services move to the shared `services/` layer in a follow-up slice.

- [ ] **Step 5: Run the targeted tests and a focused binary test pass**

Run:

```bash
cargo test -p lab api::services::helpers -- --nocapture
cargo test -p lab api::services::bytestash -- --nocapture
```

Expected:

- helper tests pass
- no regressions in the affected HTTP API area

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/api/services/helpers.rs crates/lab/src/api/services.rs crates/lab/src/api/services/bytestash.rs
git commit -m "feat: shared http api dispatch wrapper with confirmation gate"
```

### Task 3: Introduce The Shared `services` Layer Skeleton

> **Engineering review decisions:**
> - `DispatchError` is **dropped**. `services/bytestash.rs` returns `Result<Value, ToolError>` directly. Both `services/` and the surface adapters live in the same `lab` crate — there is no structural reason to introduce a parallel error vocabulary and a `DispatchError → ToolError` mapping layer. The mapping adds a catch-all arm trap (any unmatched variant silently becomes `internal_error`) with no architectural gain.
> - `DispatchContext` is **minimal**: `surface: &'static str` + `instance: Option<String>` only. `request_id` requires axum middleware plumbing out of scope for this slice; `operation` is redundant with the `action` parameter already in every dispatch signature. Extend when a second service migrates and the pattern is proven.
> - `services/errors.rs` is therefore **not created**.

**Files:**
- Create: `crates/lab/src/services.rs`
- Create: `crates/lab/src/services/context.rs`
- Modify: `crates/lab/src/services/params.rs`
- Test: `crates/lab/src/services/context.rs`

- [ ] **Step 1: Write failing tests for `DispatchContext`**

Cover:

- constructs with surface + instance
- constructs with surface only (`instance = None`)
- field values are accessible

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test -p lab dispatch_context -- --nocapture
```

Expected:

- failure because the type does not exist yet

- [ ] **Step 3: Implement `DispatchContext`**

In `crates/lab/src/services/context.rs`, add:

```rust
pub struct DispatchContext {
    pub surface: &'static str,    // "cli" | "mcp" | "api"
    pub instance: Option<String>, // for multi-instance routing; None = default instance
}
```

Do **not** add `request_id`, `operation`, or any other fields — they are deferred.

Do **not** create `services/errors.rs` or a `DispatchError` type. Service dispatch functions use `ToolError` from `crate::mcp::envelope` directly.

- [ ] **Step 4: Expose the new module tree**

In `crates/lab/src/services.rs`, export:

- `context`
- `params`
- `bytestash`

Keep declarations aligned with the files created in this slice. Do not stub empty modules.

- [ ] **Step 5: Run the targeted tests**

Run:

```bash
cargo test -p lab dispatch_context -- --nocapture
```

Expected:

- tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/services.rs crates/lab/src/services/context.rs crates/lab/src/services/params.rs
git commit -m "feat: add shared service dispatch context"
```

### Task 4: Move ByteStash Into `services`

**Files:**
- Create: `crates/lab/src/services/bytestash.rs`
- Modify: `crates/lab/src/mcp/services/bytestash.rs`
- Modify: `crates/lab/src/cli/bytestash.rs`
- Modify: `crates/lab/src/api/services/bytestash.rs`
- Test: `crates/lab/src/services/bytestash.rs`

- [ ] **Step 1: Write failing tests for shared ByteStash dispatch**

Cover:

- `help`
- one read-only success path such as `auth.config` or `snippets.list`
- missing param for an id-based action
- unknown action

Start with tests that focus on dispatch helpers and error shape, not live service behavior.

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test -p lab services::bytestash -- --nocapture
```

Expected:

- failure because the service layer module does not exist yet

- [ ] **Step 3: Create `crates/lab/src/services/bytestash.rs`**

Move from MCP-owned dispatch into the new shared layer:

- `ACTIONS` — this is now the single authoritative source; MCP re-exports from here
- `client_from_env` — apply the empty-string guard: `.ok().filter(|v| !v.is_empty())` on **both** env var reads (`BYTESTASH_URL` and `BYTESTASH_TOKEN`). A blank env var must be treated the same as an absent one.
- param extraction helpers (`require_str`, `body_from_params`, etc.)
- payload/body normalization
- action matching
- SDK calls

**Return `Result<Value, ToolError>` directly.** Do not introduce a separate `DispatchError` type (see Task 3 decision note).

**Fix `auth.register` destructive flag:** In the `ACTIONS` catalog, `auth.register` must be `destructive: true`. It creates a new user account and should trigger the confirmation gate on the HTTP surface.

**AppState note:** `client_from_env` reads env vars per-dispatch and constructs a new `reqwest::Client` (connection pool) each time. This is acceptable for the initial migration. A follow-up task should move `ByteStashClient` construction to `AppState` (initialized once at startup) to enable connection reuse across requests.

- [ ] **Step 4: Make MCP wrap the shared ByteStash service layer**

Update `crates/lab/src/mcp/services/bytestash.rs` so it becomes an adapter that:

- re-exports `ACTIONS` from `crate::services::bytestash::ACTIONS` (single authoritative source — no copy)
- exposes MCP `help` and `schema` from shared metadata
- calls `services::bytestash::dispatch(...)` directly — no error type conversion needed since `dispatch` already returns `ToolError`

It must stop owning the shared operation implementation.

- [ ] **Step 5: Make the ByteStash CLI wrap the shared service layer**

Update `crates/lab/src/cli/bytestash.rs` so it:

- parses params via `services::params::parse_kv_params`
- builds a `DispatchContext { surface: "cli", instance: None }`
- calls `services::bytestash::dispatch(...)`
- renders the returned JSON

It must stop calling `crate::mcp::services::bytestash::dispatch(...)`.

- [ ] **Step 6: Make the ByteStash HTTP API wrap the shared service layer**

Update `crates/lab/src/api/services/bytestash.rs` so it:

- uses `handle_action` from `crate::api::services::helpers`
- passes `services::bytestash::ACTIONS` for the confirmation gate check
- calls `services::bytestash::dispatch(...)` in the dispatch closure

It must stop calling `crate::mcp::services::bytestash::dispatch(...)`.

- [ ] **Step 7: Update `crates/lab/src/cli/serve.rs`**

`cli/serve.rs` contains a dispatch fan-out that calls `mcp::services::bytestash::dispatch` directly. After this step, `mcp/services/bytestash.rs` is a thin adapter over `services::bytestash` — its `dispatch` function still exists as a public entry point that MCP uses. **No change to `serve.rs` is required** if `mcp::services::bytestash::dispatch` is kept as a forwarding shim. Verify this compiles:

```bash
cargo check -p lab
```

If `serve.rs` does import `mcp::services::bytestash::dispatch` and that symbol is removed, update `serve.rs` to call `services::bytestash::dispatch` with `DispatchContext { surface: "api", instance: None }` instead.

- [ ] **Step 8: Run focused tests for the migrated ByteStash slice**

Run:

```bash
cargo test -p lab services::bytestash -- --nocapture
cargo test -p lab bytestash -- --nocapture
```

Expected:

- dispatch tests pass
- MCP adapter still behaves correctly
- CLI and HTTP API compile and pass their focused tests

- [ ] **Step 9: Run a broader crate check**

Run:

```bash
cargo test -p lab
just check
```

Expected:

- crate tests pass
- workspace check passes

- [ ] **Step 10: Update docs**

Update:

- `docs/coverage/bytestash.md`
- `docs/SERVICE_LAYER_MIGRATION.md` if the implementation shape differs from plan

The coverage doc should note that ByteStash now routes through the shared `services` layer instead of MCP-owned dispatch.

- [ ] **Step 11: Commit**

```bash
git add crates/lab/src/services/bytestash.rs crates/lab/src/mcp/services/bytestash.rs crates/lab/src/cli/bytestash.rs crates/lab/src/api/services/bytestash.rs crates/lab/src/cli/serve.rs docs/coverage/bytestash.md
git commit -m "refactor: migrate bytestash to shared services layer"
```

### Task 5: Post-Slice Verification And Follow-Up Decisions

**Files:**
- Modify: `docs/SERVICE_LAYER_MIGRATION.md` only if needed
- Modify: `docs/reports/2026-04-08-service-onboarding-review.md` only if needed

- [ ] **Step 1: Verify the forbidden dependencies are gone for ByteStash**

Run:

```bash
rtk rg -n "mcp::services::bytestash::dispatch|crate::mcp::services::bytestash::dispatch" crates/lab/src
```

Expected:

- no CLI or HTTP API caller still depends on the MCP ByteStash dispatcher
- the only match (if any) should be inside `crates/lab/src/mcp/services/bytestash.rs` itself (the forwarding shim), not in `cli/`, `api/`, or `cli/serve.rs`

Also verify `serve.rs` specifically:

```bash
rtk rg -n "bytestash" crates/lab/src/cli/serve.rs
```

Expected:

- any `bytestash` reference in `serve.rs` calls `services::bytestash::dispatch`, not `mcp::services::bytestash::dispatch`

- [ ] **Step 2: Verify the new dependency path exists**

Run:

```bash
rtk rg -n "services::bytestash::dispatch" crates/lab/src
```

Expected:

- MCP adapter uses it
- CLI uses it
- HTTP API uses it

- [ ] **Step 3: Record any design adjustments before migrating UniFi**

If the first slice reveals a better shared error/result shape or helper layout:

- update `docs/DISPATCH.md`
- update `docs/SERVICE_LAYER_MIGRATION.md`

Do this before copying the pattern to UniFi.

- [ ] **Step 4: Commit**

```bash
git add docs/DISPATCH.md docs/SERVICE_LAYER_MIGRATION.md docs/reports/2026-04-08-service-onboarding-review.md
git commit -m "docs: refine service layer migration after first slice"
```
