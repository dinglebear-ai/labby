# MCP Registry Normalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Normalize MCP service registration so the registry consistently uses the shared `dispatch` layer directly, removes stale `mcp/services` wrapper modules, and preserves any MCP-specific behavior or tests intentionally.

**Architecture:** The MCP registry should be the single place that chooses how each tool is exposed, while `crate::dispatch::<service>` owns service behavior and action catalogs. `crates/lab/src/mcp/services/` should only keep modules that add real MCP-specific logic or intentionally hold MCP-facing tests; pure forwarding wrappers should be removed from live compilation and, where useful, replaced by test modules or moved tests closer to the owning dispatch code.

**Tech Stack:** Rust 2024, `tracing`, shared dispatch layer in `crates/lab/src/dispatch`, MCP registry in `crates/lab/src/registry.rs`, cargo workspace build with `--all-features`

---

## File Structure

### Active registry wiring

- Modify: `crates/lab/src/registry.rs`
  - Normalize every migrated MCP registration to use `crate::dispatch::<service>::dispatch` and `crate::dispatch::<service>::ACTIONS` or `actions()`.
  - Keep explicit exceptions only where MCP-specific behavior is real and intentional, such as `deploy`, `gateway`, `logs`, `extract`, and `lab_admin`.

### MCP module table

- Modify: `crates/lab/src/mcp/services.rs`
  - Remove `pub mod <service>;` entries for services whose MCP module is only a stale or redundant wrapper.
  - Keep module entries only for services with live MCP-specific behavior or intentionally compiled MCP test modules.

### Redundant MCP wrapper modules to retire or convert

- Review/remove or convert to test-only:
  - `crates/lab/src/mcp/services/prowlarr.rs`
  - `crates/lab/src/mcp/services/plex.rs`
  - `crates/lab/src/mcp/services/sabnzbd.rs`
  - `crates/lab/src/mcp/services/linkding.rs`
  - `crates/lab/src/mcp/services/mcpregistry.rs`
  - `crates/lab/src/mcp/services/bytestash.rs`
  - `crates/lab/src/mcp/services/paperless.rs`
  - `crates/lab/src/mcp/services/unifi.rs`

### Explicitly preserved MCP-specific modules

- Keep as live MCP modules unless architecture changes:
  - `crates/lab/src/mcp/services/extract.rs`
  - `crates/lab/src/mcp/services/gateway.rs`
  - `crates/lab/src/mcp/services/logs.rs`
  - `crates/lab/src/mcp/services/radarr.rs`
  - `crates/lab/src/mcp/services/deploy.rs`
  - `crates/lab/src/mcp/services/lab_admin.rs`

### Optional follow-up cleanup

- Review later for full architectural consistency:
  - `crates/lab/src/mcp/services/sonarr.rs`
  - `crates/lab/src/mcp/services/qbittorrent.rs`
  - `crates/lab/src/mcp/services/memos.rs`
  - `crates/lab/src/mcp/services/overseerr.rs`
  - `crates/lab/src/mcp/services/gotify.rs`
  - `crates/lab/src/mcp/services/openai.rs`
  - `crates/lab/src/mcp/services/qdrant.rs`
  - `crates/lab/src/mcp/services/tei.rs`
  - `crates/lab/src/mcp/services/apprise.rs`
  - `crates/lab/src/mcp/services/arcane.rs`

These are still thin wrappers, but they are not currently in the broken hybrid state if the registry still intentionally references them. Normalize them only if the goal is full consistency, not just eliminating hybrid migration residue.

## Target State

- `registry.rs` directly registers shared dispatch for all migrated services.
- `mcp/services.rs` only compiles:
  - MCP-specific service modules
  - modules intentionally kept for MCP-only tests and still referenced
- no stale wrapper module is compiled without being referenced by the registry
- `cargo build --all-features --manifest-path crates/lab/Cargo.toml` completes without warnings caused by dead MCP wrappers

## Service Classification Matrix

### Migrate now to direct dispatch-only registration

- `prowlarr`
- `plex`
- `sabnzbd`
- `linkding`
- `mcpregistry`
- `bytestash`
- `paperless`
- `unifi`

### Already removed from active MCP module table as stale wrappers

- `tautulli`
- `tailscale`

### Preserve as real MCP-owned modules for now

- `extract`
- `gateway`
- `logs`
- `radarr`
- `deploy`
- `lab_admin`

### Decide explicitly later if full normalization is desired

- `sonarr`
- `qbittorrent`
- `memos`
- `overseerr`
- `gotify`
- `openai`
- `qdrant`
- `tei`
- `apprise`
- `arcane`

---

### Task 1: Normalize the registry ownership rules

**Files:**
- Modify: `crates/lab/src/registry.rs`
- Modify: `crates/lab/src/mcp/services.rs`
- Test: `cargo build --all-features --manifest-path crates/lab/Cargo.toml`

- [ ] **Step 1: Document the exceptions before editing**

Create a scratch checklist in the working note or commit message draft:

```text
Keep MCP-owned modules:
- extract
- gateway
- logs
- radarr
- deploy
- lab_admin

Normalize migrated services:
- prowlarr
- plex
- sabnzbd
- linkding
- mcpregistry
- bytestash
- paperless
- unifi
```

- [ ] **Step 2: Update `registry.rs` comments to describe the desired rule**

Edit the `register_service!` macro comments so they reflect the intended direction:

```rust
/// Default: use `crate::dispatch::<service>` directly for migrated services.
/// Override only when MCP-specific behavior or non-standard action sources are intentional.
```

- [ ] **Step 3: Make each migrated service registration explicitly use dispatch-layer ownership**

Ensure these registrations use shared dispatch directly:

```rust
register_service!(
    reg,
    "prowlarr",
    prowlarr,
    actions = crate::dispatch::prowlarr::ACTIONS,
    dispatch = dispatch_fn!(crate::dispatch::prowlarr::dispatch)
);
```

Repeat the same pattern for:

```rust
plex
sabnzbd
linkding
mcpregistry
bytestash
paperless
unifi
```

Use `actions()` only where the dispatch layer actually exposes a function instead of a const:

```rust
actions = crate::dispatch::unifi::actions(),
dispatch = dispatch_fn!(crate::dispatch::unifi::dispatch)
```

- [ ] **Step 4: Remove redundant `pub mod` entries from `mcp/services.rs` for migrated services**

Delete module declarations for services that no longer need a live MCP wrapper:

```rust
#[cfg(feature = "prowlarr")]
pub mod prowlarr;
```

Apply the same removal for:

```rust
plex
sabnzbd
linkding
mcpregistry
bytestash
paperless
unifi
```

- [ ] **Step 5: Run a full build to catch unresolved references**

Run:

```bash
cargo build --all-features --manifest-path crates/lab/Cargo.toml
```

Expected:
- build succeeds
- no compile failures from missing `crate::mcp::services::<service>` references

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/registry.rs crates/lab/src/mcp/services.rs
git commit -m "refactor: normalize migrated mcp registry wiring"
```

### Task 2: Retire or relocate redundant MCP wrapper files

**Files:**
- Modify/Delete:
  - `crates/lab/src/mcp/services/prowlarr.rs`
  - `crates/lab/src/mcp/services/plex.rs`
  - `crates/lab/src/mcp/services/sabnzbd.rs`
  - `crates/lab/src/mcp/services/linkding.rs`
  - `crates/lab/src/mcp/services/mcpregistry.rs`
  - `crates/lab/src/mcp/services/bytestash.rs`
  - `crates/lab/src/mcp/services/paperless.rs`
  - `crates/lab/src/mcp/services/unifi.rs`
- Optional test move targets:
  - `crates/lab/src/dispatch/<service>/`
  - `crates/lab/src/registry.rs` test module
- Test: `cargo build --all-features --manifest-path crates/lab/Cargo.toml`

- [ ] **Step 1: Classify each wrapper as one of three kinds**

For each file, mark it as:

```text
1. pure dead wrapper
2. test-only file already
3. mixed file with live wrapper + tests
```

Initial expectation:

```text
prowlarr: test-only
plex: test-only
sabnzbd: live wrapper only
linkding: test-only
mcpregistry: test-only
bytestash: test-only
paperless: test-only
unifi: test-only
```

- [ ] **Step 2: Delete pure dead wrappers**

Delete files that provide no remaining value after registry normalization.

Expected likely deletion:

```text
crates/lab/src/mcp/services/sabnzbd.rs
```

If a file is only:

```rust
pub async fn dispatch(...) { crate::dispatch::<service>::dispatch(...).await }
```

and the registry no longer references it, remove it.

- [ ] **Step 3: Decide where MCP-facing catalog tests belong**

For each test-only file, choose one destination:

```text
A. keep the file on disk but stop compiling it entirely
B. move the tests into the owning dispatch module
C. move the tests into a registry-level test module
```

Recommendation:

```text
Move service-catalog tests into the owning dispatch module where ACTIONS live.
```

- [ ] **Step 4: Move one test file at a time**

Example for `prowlarr`:

```rust
#[cfg(test)]
mod tests {
    use super::ACTIONS;

    #[test]
    fn catalog_has_expected_actions() {
        let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
        assert!(names.contains(&"indexer.list"));
    }
}
```

Repeat for each retired MCP wrapper that currently only holds catalog-shape tests.

- [ ] **Step 5: Delete now-empty MCP test shell files**

After moving tests, delete the empty shell file from `mcp/services/`.

- [ ] **Step 6: Run build again**

Run:

```bash
cargo build --all-features --manifest-path crates/lab/Cargo.toml
```

Expected:
- no unresolved test-module references
- no stale MCP wrapper warnings

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/mcp/services crates/lab/src/dispatch
git commit -m "refactor: remove stale mcp wrapper modules"
```

### Task 3: Preserve intentional MCP-specific modules explicitly

**Files:**
- Modify:
  - `crates/lab/src/mcp/services/deploy.rs`
  - `crates/lab/src/mcp/services/extract.rs`
  - `crates/lab/src/mcp/services/gateway.rs`
  - `crates/lab/src/mcp/services/logs.rs`
  - `crates/lab/src/mcp/services/radarr.rs`
  - `crates/lab/src/mcp/services/lab_admin.rs`
  - `crates/lab/src/mcp/services.rs`
  - `crates/lab/src/registry.rs`

- [ ] **Step 1: Add or tighten comments explaining why each preserved module still exists**

Use comments like:

```rust
//! Kept in `mcp/services` because this module owns MCP-specific behavior.
```

or:

```rust
//! Kept in `mcp/services` because the registry intentionally references this module.
```

- [ ] **Step 2: Make the distinction visible in `mcp/services.rs`**

Group the module table like this:

```rust
// MCP-specific modules
pub mod deploy;
pub mod extract;
pub mod gateway;
pub mod logs;
pub mod radarr;

// Internal/admin MCP-specific modules
pub mod lab_admin;
```

- [ ] **Step 3: Add a comment in `registry.rs` above the preserved exceptions**

Example:

```rust
// These remain MCP-owned because they carry MCP-specific behavior or wiring.
```

- [ ] **Step 4: Run build**

Run:

```bash
cargo build --all-features --manifest-path crates/lab/Cargo.toml
```

Expected:
- build succeeds
- future readers can see which exceptions are intentional

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/mcp/services.rs crates/lab/src/registry.rs crates/lab/src/mcp/services
git commit -m "docs: clarify intentional mcp-owned service modules"
```

### Task 4: Optional full normalization of remaining thin wrappers

**Files:**
- Modify:
  - `crates/lab/src/registry.rs`
  - `crates/lab/src/mcp/services.rs`
  - selected files under `crates/lab/src/mcp/services/`

- [ ] **Step 1: Audit the remaining thin wrappers**

Review:

```text
sonarr
qbittorrent
memos
overseerr
gotify
openai
qdrant
tei
apprise
arcane
```

- [ ] **Step 2: For each, choose one of two end states**

```text
A. direct registry -> dispatch wiring, remove wrapper
B. keep wrapper intentionally and document why
```

- [ ] **Step 3: Apply one consistent rule**

Recommendation:

```text
If the wrapper only forwards ACTIONS and dispatch, migrate it away.
```

- [ ] **Step 4: Run build**

Run:

```bash
cargo build --all-features --manifest-path crates/lab/Cargo.toml
```

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/registry.rs crates/lab/src/mcp/services.rs crates/lab/src/mcp/services
git commit -m "refactor: finish mcp service wrapper normalization"
```

### Task 5: Validate `lab serve` console cleanliness after normalization

**Files:**
- Verify:
  - `crates/lab/src/registry.rs`
  - `crates/lab/src/mcp/services.rs`
  - `crates/lab/src/main.rs`
  - `crates/lab/src/cli/serve.rs`

- [ ] **Step 1: Build the binary**

Run:

```bash
cargo build --all-features --manifest-path crates/lab/Cargo.toml
```

Expected:
- success
- no warnings from stale MCP wrapper modules

- [ ] **Step 2: Run the actual wrapper path**

Run:

```bash
timeout 20s lab serve --port 9876
```

Expected:
- no compiler warning noise before startup logs
- preflight logs appear only when web assets are actually refreshed
- startup logs remain readable and component-separated

- [ ] **Step 3: If `8765` is occupied locally, keep using an alternate port**

Use:

```bash
lab serve --port 9876
```

Expected:
- bind succeeds
- startup reaches API/web/MCP ready

- [ ] **Step 4: Commit**

```bash
git add crates/lab/src/registry.rs crates/lab/src/mcp/services.rs crates/lab/src/mcp/services
git commit -m "chore: remove mcp wrapper warning noise from serve startup"
```

## Notes for the implementer

- Do not touch service behavior in `dispatch/` unless you are only moving tests.
- Do not move `deploy` or `lab_admin` off the MCP layer without re-checking their MCP-specific behavior first.
- `sabnzbd` is the easiest hybrid candidate to fully normalize because its registry already sources actions from `dispatch`.
- `radarr` is not a pure const-based service; keep its `actions()` path intact unless the dispatch layer is reworked first.
- `unifi` already has direct dispatch registration but still leaves a compiled MCP test shell; decide whether those tests belong in dispatch or in a registry-level test.

## Review note

The writing-plans skill asks for a separate plan review pass. I did not dispatch a reviewer subagent here because this session has not been explicitly authorized for sub-agent delegation.

