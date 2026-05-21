# Radarr Dispatch Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Radarr's shared operation catalog, validation, client resolution, and machine-facing execution into `crates/lab/src/dispatch/` so CLI, MCP, and API all route through the same shared backend.

**Architecture:** Introduce a new directory-first `crates/lab/src/dispatch/radarr/` module that mirrors the current MCP resource-group split while relocating semantic ownership out of `mcp/services/`. Keep CLI, MCP, and API as thin adapters over `dispatch::radarr`, and preserve the existing typed CLI UX by mapping typed subcommands onto shared dispatch actions.

**Tech Stack:** Rust, Tokio, serde_json, clap, axum, tracing, lab-apis Radarr client.

---

## File Structure

- Create: `crates/lab/src/dispatch/radarr.rs`
- Create: `crates/lab/src/dispatch/radarr/catalog.rs`
- Create: `crates/lab/src/dispatch/radarr/client.rs`
- Create: `crates/lab/src/dispatch/radarr/dispatch.rs`
- Create: `crates/lab/src/dispatch/radarr/system.rs`
- Create: `crates/lab/src/dispatch/radarr/movies.rs`
- Create: `crates/lab/src/dispatch/radarr/queue.rs`
- Create: `crates/lab/src/dispatch/radarr/calendar.rs`
- Create: `crates/lab/src/dispatch/radarr/commands.rs`
- Create: `crates/lab/src/dispatch/radarr/history.rs`
- Create: `crates/lab/src/dispatch/radarr/config.rs`
- Modify: `crates/lab/src/dispatch.rs`
- Modify: `crates/lab/src/mcp/services/radarr.rs`
- Modify: `crates/lab/src/api/services/radarr.rs`
- Modify: `crates/lab/src/cli/radarr.rs`
- Modify: `crates/lab/src/cli/health.rs`
- Modify: `docs/coverage/radarr.md`
- Modify: `docs/SERVICE_LAYER_MIGRATION.md`

### Task 1: Create the shared Radarr dispatch module

**Files:**
- Create: `crates/lab/src/dispatch/radarr.rs`
- Create: `crates/lab/src/dispatch/radarr/catalog.rs`
- Create: `crates/lab/src/dispatch/radarr/client.rs`
- Create: `crates/lab/src/dispatch/radarr/dispatch.rs`
- Create: `crates/lab/src/dispatch/radarr/system.rs`
- Create: `crates/lab/src/dispatch/radarr/movies.rs`
- Create: `crates/lab/src/dispatch/radarr/queue.rs`
- Create: `crates/lab/src/dispatch/radarr/calendar.rs`
- Create: `crates/lab/src/dispatch/radarr/commands.rs`
- Create: `crates/lab/src/dispatch/radarr/history.rs`
- Create: `crates/lab/src/dispatch/radarr/config.rs`
- Test: `crates/lab/src/dispatch/radarr.rs`

- [ ] **Step 1: Copy the existing MCP resource-group split into `dispatch/radarr/`**

Move the Radarr action groups out of `crates/lab/src/mcp/services/radarr/` into the new `dispatch/radarr/` tree, keeping the current domain boundaries (`system`, `movies`, `queue`, `calendar`, `commands`, `history`, `config`).

- [ ] **Step 2: Build the thin public entrypoint**

Create `crates/lab/src/dispatch/radarr.rs` that declares the submodules and re-exports:
- `pub use catalog::actions;`
- `pub use client::client_from_env;`
- `pub use dispatch::dispatch;`

- [ ] **Step 3: Centralize catalog assembly**

Create `catalog.rs` that concatenates the action slices from each domain module and adds `help` as the first action.

- [ ] **Step 4: Centralize client construction**

Create `client.rs` and move `client_from_env` plus any shared client helper logic there.

- [ ] **Step 5: Centralize top-level dispatch**

Create `dispatch.rs` and move the top-level `help` response and action-prefix routing there. Unknown actions must still return `ToolError::UnknownAction` with the full valid action list.

- [ ] **Step 6: Preserve existing tests and add missing shape checks**

Keep or move the current Radarr tests so they now assert against `dispatch::radarr::{actions, dispatch}` and verify action count, duplicate detection, and `ToolError` conversion behavior.

- [ ] **Step 7: Run focused tests for the new shared module**

Run: `cargo test --manifest-path crates/lab/Cargo.toml dispatch::radarr --features radarr -- --nocapture`
Expected: PASS

### Task 2: Rewire MCP, API, and CLI to the shared dispatch module

**Files:**
- Modify: `crates/lab/src/dispatch.rs`
- Modify: `crates/lab/src/mcp/services/radarr.rs`
- Modify: `crates/lab/src/api/services/radarr.rs`
- Modify: `crates/lab/src/cli/radarr.rs`
- Modify: `crates/lab/src/cli/health.rs`
- Test: `crates/lab/src/mcp/services/radarr.rs`

- [ ] **Step 1: Export `dispatch::radarr` from the shared layer root**

Add `#[cfg(feature = "radarr")] pub mod radarr;` to `crates/lab/src/dispatch.rs`.

- [ ] **Step 2: Reduce the MCP module to a thin adapter**

Update `crates/lab/src/mcp/services/radarr.rs` so it only forwards:
- `pub use crate::dispatch::radarr::actions;`
- `pub use crate::dispatch::radarr::client_from_env;`
- `pub async fn dispatch(...)` delegating to `crate::dispatch::radarr::dispatch(...)`

- [ ] **Step 3: Rewire the API adapter**

Update `crates/lab/src/api/services/radarr.rs` to call `crate::dispatch::radarr::{actions, dispatch}` through `handle_action` instead of the MCP module.

- [ ] **Step 4: Rewire the typed CLI onto the shared dispatch path**

Update `crates/lab/src/cli/radarr.rs` so typed subcommands construct the canonical machine-facing action name and params, then call `crate::dispatch::radarr::dispatch(...)` rather than using a client directly. Preserve the current CLI UX and confirmations.

- [ ] **Step 5: Rewire health checks to the shared client helper**

Update any Radarr-specific health path imports, especially in `crates/lab/src/cli/health.rs`, to use `crate::dispatch::radarr::client_from_env`.

- [ ] **Step 6: Run adapter-focused verification**

Run:
- `cargo test --manifest-path crates/lab/Cargo.toml mcp::services::radarr --features radarr -- --nocapture`
- `cargo test --manifest-path crates/lab/Cargo.toml cli::radarr --features radarr -- --nocapture`
- `cargo test --manifest-path crates/lab/Cargo.toml api::services::helpers --features radarr -- --nocapture`
Expected: PASS

### Task 3: Update docs and verify no stale coupling remains

**Files:**
- Modify: `docs/coverage/radarr.md`
- Modify: `docs/SERVICE_LAYER_MIGRATION.md`

- [ ] **Step 1: Update coverage references**

Point the Radarr coverage doc at `crates/lab/src/dispatch/radarr.rs` and the new `dispatch/radarr/` module tree.

- [ ] **Step 2: Update migration checklist status if the slice is complete**

Mark the Radarr checklist items complete in `docs/SERVICE_LAYER_MIGRATION.md` only if CLI, MCP, and API all route through `dispatch::radarr` and verification passes.

- [ ] **Step 3: Verify no `api -> mcp` or `cli -> mcp` coupling remains for Radarr**

Run:
`rtk rg -n 'mcp::services::radarr::dispatch|mcp::services::radarr::client_from_env' crates/lab/src -g '*.rs'`
Expected: no matches outside the MCP forwarding shim itself

- [ ] **Step 4: Run final compile check**

Run: `cargo check --manifest-path crates/lab/Cargo.toml --features radarr`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch.rs crates/lab/src/dispatch/radarr.rs crates/lab/src/dispatch/radarr crates/lab/src/mcp/services/radarr.rs crates/lab/src/api/services/radarr.rs crates/lab/src/cli/radarr.rs crates/lab/src/cli/health.rs docs/coverage/radarr.md docs/SERVICE_LAYER_MIGRATION.md
git commit -m "refactor: migrate radarr to dispatch layer"
```
