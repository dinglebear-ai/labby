# lab-iut1.2 Marketplace Artifact Action Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire all 11 marketplace `artifact.*` actions into catalog, parameter parsing, dispatch routing, CLI/API visibility, docs, and tests so bead `lab-iut1.2` can close.

**Architecture:** Keep marketplace as the existing Tier 2 action-string dispatch surface. Add missing artifact action specs and parser structs, route missing lifecycle/diff/patch actions to stub domain modules returning structured `not_implemented`, and preserve already-implemented update actions from `lab-iut1.5/.6`. Keep destructive confirmation at surface gates (`ActionSpec.destructive`, CLI `-y`, API `params.confirm`) while stripping `confirm` before parser/domain execution.

**Tech Stack:** Rust 2024, axum 0.8, serde_json, existing `ToolError`, marketplace shared dispatch catalog, fs4-backed stash metadata helpers.

---

### Task 1: Catalog all 11 artifact actions

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Test: `crates/lab/src/dispatch/marketplace.rs`

- [ ] Add `ActionSpec` entries for `artifact.fork`, `artifact.list`, `artifact.unfork`, `artifact.reset`, `artifact.diff`, and `artifact.patch` before existing update actions.
- [ ] Confirm existing `artifact.update.check`, `artifact.update.preview`, `artifact.update.apply`, `artifact.merge.suggest`, and `artifact.config.set` remain cataloged.
- [ ] Set exact returns strings: `ForkResult`, `ForkedPluginStatus[]`, `UnforkResult`, `ResetResult`, `ArtifactDiffResult`, `PatchResult`, `UpdateCheckResult[]`, `UpdatePreviewResult`, `ApplyResult`, `MergeSuggestResult`, `ConfigSetResult`.
- [ ] Set exact destructive flags: true only for `artifact.unfork`, `artifact.reset`, and `artifact.update.apply`.
- [ ] Keep `confirm` as a catalog/surface key only for destructive actions so API/CLI confirmation gates remain discoverable; do not include it in parser structs.

### Task 2: Add parser structs and functions

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/params.rs`
- Modify: `crates/lab/src/dispatch/marketplace/update.rs`
- Test: `crates/lab/src/dispatch/marketplace.rs`

- [ ] Add `ForkParams`, `ArtifactListParams`, `UnforkParams`, `ArtifactResetParams`, `ArtifactDiffParams`, `PatchParams`, `UpdateApplyParams`, `MergeSuggestParams`, and `ConfigSetParams`.
- [ ] Add `parse_fork_params`, `parse_artifact_list_params`, `parse_unfork_params`, `parse_artifact_reset_params`, `parse_artifact_diff_params`, `parse_patch_params`, `parse_update_apply_params`, `parse_merge_suggest_params`, and `parse_config_set_params`.
- [ ] Validate plugin ids with existing `parse_plugin_id` and artifact paths with shared stash metadata path validation semantics.
- [ ] Validate `strategy` values into `ConflictStrategy::{KeepMine, TakeUpstream, AlwaysAsk, AiSuggest}`.
- [ ] Remove update-local parser structs/functions and use `params.rs` parsers.
- [ ] Remove domain-level `confirm` checks from `artifact.update.apply`; surface gates own confirmation.

### Task 3: Add stub domain modules and dispatch routing

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Create: `crates/lab/src/dispatch/marketplace/fork.rs`
- Create: `crates/lab/src/dispatch/marketplace/patch.rs`
- Create: `crates/lab/src/dispatch/marketplace/diff.rs`

- [ ] Add module declarations for `fork`, `patch`, and `diff`.
- [ ] Route lifecycle actions to `fork::{artifact_fork, artifact_list, artifact_unfork, artifact_reset}`.
- [ ] Route diff/patch actions to `patch::{artifact_diff, artifact_patch}`.
- [ ] Leave update/merge/config actions routed to existing `update::dispatch_update_action`.
- [ ] Make new stubs return `ToolError::Sdk { sdk_kind: "not_implemented", ... }`, not `todo!()` or panic.
- [ ] Add `diff.rs` placeholder signatures for git diff/merge helpers using hardened local git env and no `fs2`.

### Task 4: Add API path aliases and docs

**Files:**
- Modify: `crates/lab/src/api/services/marketplace.rs`
- Modify: `docs/MCP.md`

- [ ] Add POST aliases for `/artifact/fork`, `/artifact/list`, `/artifact/unfork`, `/artifact/reset`, `/artifact/diff`, `/artifact/patch`, `/artifact/update/check`, `/artifact/update/preview`, `/artifact/update/apply`, `/artifact/merge/suggest`, and `/artifact/config/set`.
- [ ] Keep generic POST `/` action dispatch unchanged.
- [ ] Ensure path aliases call the same `handle_action` helper so unknown-action, destructive confirmation, `confirm` stripping, and logs remain consistent.
- [ ] Document all 11 artifact actions and confirmation behavior in `docs/MCP.md`.

### Task 5: Tests and verification

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace.rs`

- [ ] Add tests for exact artifact catalog membership, returns, and destructive flags.
- [ ] Add help/catalog visibility test for all artifact actions.
- [ ] Add dispatch stub roundtrip test for `artifact.fork` returning `not_implemented`.
- [ ] Add unknown action test for `artifact.bogus` returning `unknown_action`.
- [ ] Add parser tests for fork artifact path validation and update strategy validation.
- [ ] Run `cargo test --package lab --all-features`.
- [ ] Run `cargo clippy --package lab --all-features -- -D warnings`.
- [ ] Run catalog/help/API/source checks covering all 11 actions, returns, destructive flags, parser functions, API paths, unknown action behavior, no `fs2`, and `lab help` visibility.
