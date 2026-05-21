# lab-cgxg MCP Registration Normalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete bead `lab-cgxg` by making direct `crate::dispatch::<service>` registration the default MCP path and removing stale `mcp/services` wrapper modules that do not own MCP-specific behavior.

**Architecture:** `crates/lab/src/registry.rs` becomes the single default registration contract: normal services register their catalog and dispatch directly from `crate::dispatch::<service>`. `crates/lab/src/mcp/services/` remains an exception layer only for modules with proven MCP-specific behavior (`deploy`, `fs`, and `nodes`). Tests previously stranded in dead wrapper modules are either already covered in dispatch tests or moved into the dispatch owner.

**Tech Stack:** Rust 2024, existing `lab_apis::core::action::ActionSpec`, shared `crate::dispatch` modules, MCP registry/catalog plumbing, scaffold/audit tooling, Markdown docs.

---

## File Structure

- Modify: `crates/lab/src/registry.rs`
  - Update `register_service!` documentation and default arm so normal feature-gated services use `crate::dispatch::<service>::ACTIONS` and `crate::dispatch::<service>::dispatch`.
  - Keep override forms for services with `actions()` instead of `ACTIONS` and for MCP-specific exception modules.
  - Change manual always-on registrations (`extract`, `gateway`, `doctor`, `logs`, `marketplace`, `lab_admin`) to direct dispatch where possible.
  - Keep `device` via `mcp/services/nodes`, `deploy` via `mcp/services/deploy`, and `fs` via `mcp/services/fs` because those modules own MCP-specific behavior.
- Modify: `crates/lab/src/mcp/services.rs`
  - Replace the old “per-service dispatch modules” contract with an exception-only module list.
  - Keep only `deploy`, `fs`, and `nodes` declarations.
- Delete: thin or tests-only wrappers under `crates/lab/src/mcp/services/` that have no MCP-specific behavior.
- Modify: selected dispatch entrypoints (`radarr`, `linkding`, `paperless`, `plex`, `unifi`, `lab_admin`) to preserve useful tests from deleted wrappers.
- Modify: scaffold/audit code under `crates/lab/src/scaffold*` and `crates/lab/src/audit/checks/*` so future services do not regenerate or require MCP wrapper files.
- Modify: docs (`crates/lab/src/mcp/CLAUDE.md`, `docs/SERVICE_ONBOARDING.md`, `docs/SCAFFOLD_AND_AUDIT.md`, selected coverage docs) so documented onboarding matches the new registry contract.
- Create: `docs/sessions/2026-04-25-lab-cgxg-completion.md` after verification.

## Task 1: Normalize the registry contract

**Files:**
- Modify: `crates/lab/src/registry.rs`

- [ ] **Step 1: Update the macro contract comments**

Describe the default form as direct `crate::dispatch::<service>` registration. Document that override forms are only for `actions()` catalogs or MCP-specific exception modules.

- [ ] **Step 2: Update the default macro arm**

Change the default arm to:

```rust
let actions: &'static [ActionSpec] = crate::dispatch::$svc::ACTIONS;
dispatch: dispatch_fn!(crate::dispatch::$svc::dispatch),
```

- [ ] **Step 3: Normalize registrations**

Use direct dispatch for all normal services. Keep only these MCP-specific references:

```rust
crate::mcp::services::nodes::{ACTIONS, dispatch}
crate::mcp::services::deploy::{ACTIONS, dispatch}
crate::mcp::services::fs::{ACTIONS, dispatch}
```

Use direct `actions()` overrides for services that do not expose a top-level `ACTIONS` const:

```rust
crate::dispatch::radarr::actions()
crate::dispatch::unifi::actions()
crate::dispatch::marketplace::actions()
```

- [ ] **Step 4: Run targeted compile check after the full edit batch**

Run later with the rest of verification:

```bash
cargo build --all-features --manifest-path crates/lab/Cargo.toml
```

Expected: PASS, with no stale `mcp/services` dead-code warnings.

## Task 2: Retire stale MCP wrappers and preserve tests

**Files:**
- Modify: `crates/lab/src/mcp/services.rs`
- Delete stale wrappers in `crates/lab/src/mcp/services/*.rs` except `deploy.rs`, `fs.rs`, and `nodes.rs`
- Modify: `crates/lab/src/dispatch/radarr.rs`
- Modify: `crates/lab/src/dispatch/linkding.rs`
- Modify: `crates/lab/src/dispatch/paperless.rs`
- Modify: `crates/lab/src/dispatch/plex.rs`
- Modify: `crates/lab/src/dispatch/unifi.rs`
- Modify: `crates/lab/src/dispatch/lab_admin.rs`

- [ ] **Step 1: Prune `mcp/services.rs`**

Keep only MCP-specific exception modules and comments explaining why each remains.

- [ ] **Step 2: Move or preserve wrapper tests**

Add missing assertions to dispatch-layer tests where wrapper files currently hold unique checks:

```rust
// radarr: core read-only actions in actions()
// linkding: full core catalog and destructive bookmark.delete
// paperless: full resource catalog smoke check
// plex: full core catalog and destructive actions
// unifi: help/read-only catalog plus action-count parity
// lab_admin: help and onboarding.audit catalog tests
```

- [ ] **Step 3: Delete stale wrapper files**

Remove all `crates/lab/src/mcp/services/<service>.rs` files that only forward, re-export, or contain tests for dispatch-owned behavior.

Expected remaining files:

```text
crates/lab/src/mcp/services/deploy.rs
crates/lab/src/mcp/services/fs.rs
crates/lab/src/mcp/services/nodes.rs
```

## Task 3: Stop scaffold/audit from reintroducing wrappers

**Files:**
- Modify: `crates/lab/src/scaffold.rs`
- Modify: `crates/lab/src/scaffold/patcher.rs`
- Modify: `crates/lab/src/scaffold/patcher/source.rs`
- Modify: `crates/lab/src/scaffold/templates.rs`
- Modify: `crates/lab/src/scaffold/templates/adapters.rs`
- Delete: `crates/lab/src/scaffold/templates/adapter_mcp.tpl`
- Modify: `crates/lab/src/audit/checks/files.rs`
- Modify: `crates/lab/src/audit/checks/registration.rs`
- Modify: `crates/lab/src/audit/checks/tests.rs`

- [ ] **Step 1: Remove MCP adapter generation**

Remove the generated `crates/lab/src/mcp/services/<service>.rs` file op and the `adapter_mcp_template` export/helper.

- [ ] **Step 2: Remove MCP services patching**

Remove `patch_mcp_services_rs` and its `compute_patches` entry. New services should only patch `registry.rs`.

- [ ] **Step 3: Update audit checks**

Remove required checks for `mcp/services.rs`, `crates/lab/src/mcp/services/<service>.rs`, and `impl.mcp` so onboarding audit aligns with direct registry registration.

## Task 4: Update docs to the new contract

**Files:**
- Modify: `crates/lab/src/mcp/CLAUDE.md`
- Modify: `docs/SERVICE_ONBOARDING.md`
- Modify: `docs/SCAFFOLD_AND_AUDIT.md`
- Modify: `docs/coverage/*.md` where stale wrapper paths are documented

- [ ] **Step 1: Update MCP docs**

Document direct `crate::dispatch::<service>` registration as the default and `mcp/services` as exception-only.

- [ ] **Step 2: Update service onboarding and scaffold docs**

Remove instructions to create or declare thin MCP adapter files for normal services.

- [ ] **Step 3: Update coverage docs**

Replace stale references to deleted wrapper files with registry/direct-dispatch wording.

## Task 5: Verify closure

**Files:**
- None, except session report.

- [ ] **Step 1: Search for stale references**

```bash
rg -n "mcp::services::(apprise|arcane|bytestash|doctor|extract|gateway|gotify|lab_admin|linkding|logs|marketplace|memos|openai|overseerr|paperless|plex|prowlarr|qbittorrent|qdrant|radarr|sabnzbd|sonarr|tei|unifi)|crate::mcp::services::(apprise|arcane|bytestash|doctor|extract|gateway|gotify|lab_admin|linkding|logs|marketplace|memos|openai|overseerr|paperless|plex|prowlarr|qbittorrent|qdrant|radarr|sabnzbd|sonarr|tei|unifi)|crates/lab/src/mcp/services/(apprise|arcane|bytestash|doctor|extract|gateway|gotify|lab_admin|linkding|logs|marketplace|memos|openai|overseerr|paperless|plex|prowlarr|qbittorrent|qdrant|radarr|sabnzbd|sonarr|tei|unifi)\.rs" crates/lab/src docs/coverage docs/SERVICE_ONBOARDING.md docs/SCAFFOLD_AND_AUDIT.md crates/lab/src/mcp/CLAUDE.md
```

Expected: no hits for deleted wrappers.

- [ ] **Step 2: Confirm only exception modules remain**

```bash
find crates/lab/src/mcp/services -maxdepth 1 -type f | sort
```

Expected: `deploy.rs`, `fs.rs`, and `nodes.rs` only.

- [ ] **Step 3: Build all features**

```bash
cargo build --all-features --manifest-path crates/lab/Cargo.toml
```

Expected: PASS and no warnings caused by stale MCP service wrappers.

- [ ] **Step 4: Run relevant tests**

```bash
cargo test -p lab --all-features registry::tests dispatch::radarr::tests dispatch::linkding::tests dispatch::paperless::tests dispatch::plex::tests dispatch::unifi::tests dispatch::lab_admin::tests --no-fail-fast
```

Expected: PASS for the moved/affected tests.

- [ ] **Step 5: Write the session report**

Capture required command context and save `docs/sessions/2026-04-25-lab-cgxg-completion.md` without secrets.
