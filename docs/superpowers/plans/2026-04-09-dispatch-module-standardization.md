# Dispatch Module Standardization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the shared service layer from `services` to `dispatch`, standardize migrated services on a directory-first layout, and lock the contract into docs and local instructions.

**Architecture:** `cli/`, `mcp/services/`, and `api/services/` remain thin adapters over a shared `dispatch` layer. Each migrated service gets a thin entrypoint file plus predictable `catalog`, `client`, `params`, and `dispatch` modules, with optional domain modules for broad integrations.

**Tech Stack:** Rust, Cargo, `serde_json`, `tracing`, repo docs under `docs/`

---

### Task 1: Rename The Shared Layer Module

**Files:**
- Create: `crates/lab/src/dispatch.rs`
- Create: `crates/lab/src/dispatch/`
- Modify: `crates/lab/src/main.rs`
- Modify: `crates/lab/src/cli/*.rs`
- Modify: `crates/lab/src/mcp/services/*.rs`
- Modify: `crates/lab/src/api/services/*.rs`
- Test: `cargo check --manifest-path crates/lab/Cargo.toml --features 'bytestash unifi'`

- [ ] Move `crates/lab/src/services.rs` to `crates/lab/src/dispatch.rs`.
- [ ] Move shared support modules from `crates/lab/src/services/` to `crates/lab/src/dispatch/`.
- [ ] Update all imports from `crate::services::...` to `crate::dispatch::...`.
- [ ] Run `cargo check --manifest-path crates/lab/Cargo.toml --features 'bytestash unifi'`.

### Task 2: Standardize ByteStash Layout

**Files:**
- Modify: `crates/lab/src/dispatch/bytestash.rs`
- Create: `crates/lab/src/dispatch/bytestash/catalog.rs`
- Create: `crates/lab/src/dispatch/bytestash/client.rs`
- Create: `crates/lab/src/dispatch/bytestash/params.rs`
- Create: `crates/lab/src/dispatch/bytestash/dispatch.rs`
- Test: `cargo test --manifest-path crates/lab/Cargo.toml bytestash --features 'bytestash unifi' -- --nocapture`

- [ ] Write a failing compile/test target by removing the old helper module assumptions.
- [ ] Move the action catalog into `catalog.rs`.
- [ ] Move env/client construction into `client.rs`.
- [ ] Move param/body construction into `params.rs`.
- [ ] Move action execution into `dispatch.rs`.
- [ ] Keep `bytestash.rs` as a thin entrypoint only.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml bytestash --features 'bytestash unifi' -- --nocapture`.

### Task 3: Standardize UniFi Layout

**Files:**
- Modify: `crates/lab/src/dispatch/unifi.rs`
- Create: `crates/lab/src/dispatch/unifi/catalog.rs`
- Create: `crates/lab/src/dispatch/unifi/client.rs`
- Create: `crates/lab/src/dispatch/unifi/dispatch.rs`
- Modify: `crates/lab/src/dispatch/unifi/*.rs`
- Test: `cargo test --manifest-path crates/lab/Cargo.toml unifi --features 'bytestash unifi' -- --nocapture`

- [ ] Extract the combined action catalog builder into `catalog.rs`.
- [ ] Move client resolution into `client.rs`.
- [ ] Move top-level routing into `dispatch.rs`.
- [ ] Keep `unifi.rs` as a thin entrypoint only.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml unifi --features 'bytestash unifi' -- --nocapture`.

### Task 4: Lock The Contract In Docs And Local Instructions

**Files:**
- Modify: `docs/DISPATCH.md`
- Modify: `docs/SERVICE_LAYER_MIGRATION.md`
- Modify: `docs/SERVICE_ONBOARDING.md`
- Modify: `docs/OBSERVABILITY.md`
- Modify: `docs/SERIALIZATION.md`
- Modify: `docs/MCP.md`
- Modify: `docs/SERVICES.md`
- Modify: `docs/README.md`
- Modify: `docs/coverage/bytestash.md`
- Modify: `docs/coverage/unifi.md`
- Create: `crates/lab/src/dispatch/CLAUDE.md`
- Test: `rtk rg -n '\\bHTTP API\\b|shared service layer|crates/lab/src/services\\b' docs crates/lab/src/dispatch/CLAUDE.md -g '*.md'`

- [ ] Normalize product-surface wording to `API`.
- [ ] Replace `services` layer references with `dispatch` where they mean the shared layer.
- [ ] Add the directory-first layout contract and forbid new single-file migrated dispatch modules.
- [ ] Add `crates/lab/src/dispatch/CLAUDE.md` with the local enforcement rules.
- [ ] Run the grep check for stale wording and old paths.

### Task 5: Final Verification

**Files:**
- Test only

- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml cli::helpers --features 'bytestash unifi' -- --nocapture`.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml api::services::helpers --features 'bytestash unifi' -- --nocapture`.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml bytestash --features 'bytestash unifi' -- --nocapture`.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml unifi --features 'bytestash unifi' -- --nocapture`.
- [ ] Run `cargo check --manifest-path crates/lab/Cargo.toml --features 'bytestash unifi'`.
- [ ] Run `rtk rg -n 'crate::services::|crates/lab/src/services\\b|\\bHTTP API\\b' crates/lab/src docs -g '*.rs' -g '*.md'`.
