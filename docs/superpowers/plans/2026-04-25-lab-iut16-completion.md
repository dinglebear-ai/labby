# lab-iut1.6 Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete `lab-iut1.6` by shipping `artifact.update.apply`, `artifact.merge.suggest`, and `artifact.config.set` with the minimum update check/preview foundation required for end-to-end behavior.

**Architecture:** Add a focused `dispatch::marketplace::update` module that owns stash metadata, pending previews, path validation, merge classification, apply transactions, AI-merge guardrails, and tests. Wire the module into the existing marketplace action catalog and dispatcher without restructuring existing plugin workspace/deploy code. Use existing `ToolError::Sdk { sdk_kind, message }` for new stable error kinds and document those kinds in `docs/ERRORS.md`.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `tempfile`, `tokio::task::spawn_blocking`, existing marketplace dispatch helpers, filesystem-based tests under `crates/lab/src/dispatch/marketplace/update.rs`.

---

## File Structure

- Modify `crates/lab/src/dispatch/marketplace.rs`: declare the new `update` module.
- Modify `crates/lab/src/dispatch/marketplace/catalog.rs`: add `artifact.update.check`, `artifact.update.preview`, `artifact.update.apply`, `artifact.merge.suggest`, and `artifact.config.set` action specs.
- Modify `crates/lab/src/dispatch/marketplace/dispatch.rs`: route artifact update/merge/config actions to the new module.
- Create `crates/lab/src/dispatch/marketplace/update.rs`: implement params, result types, stash metadata I/O, preview computation, apply transaction, merge suggestion guardrails, and unit tests.
- Modify `docs/ERRORS.md`: document `stale_preview`, `ai_backend_not_configured`, and `content_contains_secrets` marketplace artifact update kinds.
- Create `docs/sessions/2026-04-25-lab-iut16-completion.md`: final factual session report.

## Task 1: Wire Catalog and Dispatcher

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace.rs`
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Create: `crates/lab/src/dispatch/marketplace/update.rs`

- [ ] **Step 1: Write failing catalog/dispatch tests**

Add tests in `crates/lab/src/dispatch/marketplace/update.rs` that call `dispatch("artifact.config.set", ...)` and expect the action to be known rather than `unknown_action`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lab --all-features marketplace::update::tests::config_set_updates_strategy_and_preserves_notify -- --nocapture`

Expected: FAIL because the module/action is not wired or implemented.

- [ ] **Step 3: Add module/action routing**

Declare `mod update;`, add five `ActionSpec` entries, and route matching action names to `update::dispatch_update_action(action, params).await`.

- [ ] **Step 4: Re-run the targeted test**

Run: `cargo test -p lab --all-features marketplace::update::tests::config_set_updates_strategy_and_preserves_notify -- --nocapture`

Expected: PASS after Task 2 config implementation is present.

## Task 2: Implement Stash Metadata and Config Set

**Files:**
- Modify/Create: `crates/lab/src/dispatch/marketplace/update.rs`

- [ ] **Step 1: Write failing config tests**

Cover `artifact.config.set` updating `strategy`, preserving `notify`, and rejecting invalid strategies.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p lab --all-features marketplace::update::tests::config_set -- --nocapture`

Expected: FAIL because config parsing/writing is not implemented.

- [ ] **Step 3: Implement config set**

Implement `UpdateStrategy`, `UpdateConfig`, `StashMeta`, `ConfigSetParams`, `ConfigSetResult`, `.stash.json` read/write, lock-file creation, strategy validation, and partial update semantics.

- [ ] **Step 4: Run tests**

Run: `cargo test -p lab --all-features marketplace::update::tests::config_set -- --nocapture`

Expected: PASS.

## Task 3: Implement Update Check and Preview Foundation

**Files:**
- Modify/Create: `crates/lab/src/dispatch/marketplace/update.rs`

- [ ] **Step 1: Write failing preview tests**

Cover preview returning conflicts with `MergeConflict` structs, writing `.pending-update.json`, and reporting no update when versions match/content is unchanged.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p lab --all-features marketplace::update::tests::update_preview -- --nocapture`

Expected: FAIL because check/preview is not implemented.

- [ ] **Step 3: Implement check/preview**

Resolve source/workspace/base paths, compare `.stash.json` `upstream_version` to source manifest version, classify files as `unchanged`, `upstream_only`, `user_only`, clean merge, or conflict. Write pending preview to `.pending-update.json`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p lab --all-features marketplace::update::tests::update_preview -- --nocapture`

Expected: PASS.

## Task 4: Implement Update Apply

**Files:**
- Modify/Create: `crates/lab/src/dispatch/marketplace/update.rs`

- [ ] **Step 1: Write failing apply tests**

Cover `confirm: true` requirement, `keep_mine`, `take_upstream`, `always_ask`, metadata version update, and AI strategy applying a deterministic test merge suggestion.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p lab --all-features marketplace::update::tests::update_apply -- --nocapture`

Expected: FAIL because apply is not implemented.

- [ ] **Step 3: Implement apply transaction**

Implement two-pass change collection, original-file backups, best-effort rollback logging, base snapshot updates, pending preview clearing, and structured `ApplyResult` with `Complete` or `PartialConflicts`.

- [ ] **Step 4: Run apply tests**

Run: `cargo test -p lab --all-features marketplace::update::tests::update_apply -- --nocapture`

Expected: PASS.

## Task 5: Implement AI Merge Suggest Guardrails

**Files:**
- Modify/Create: `crates/lab/src/dispatch/marketplace/update.rs`
- Modify: `docs/ERRORS.md`

- [ ] **Step 1: Write failing merge tests**

Cover `artifact.merge.suggest` returns `ai_backend_not_configured` by default and returns `content_contains_secrets` before any backend call when changed content contains credential-like patterns.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p lab --all-features marketplace::update::tests::merge_suggest -- --nocapture`

Expected: FAIL because merge suggest is not implemented.

- [ ] **Step 3: Implement merge suggest**

Read base/yours/theirs, validate relative path, scan changed content for secret markers, build the OWASP LLM01-resistant prompt, and return `ai_backend_not_configured` because no existing configured marketplace AI backend exists in this repo. Keep deterministic internal suggestion helper for `AiSuggest` apply tests.

- [ ] **Step 4: Document new error kinds**

Add `stale_preview`, `ai_backend_not_configured`, and `content_contains_secrets` to `docs/ERRORS.md` with status mappings.

- [ ] **Step 5: Run merge tests**

Run: `cargo test -p lab --all-features marketplace::update::tests::merge_suggest -- --nocapture`

Expected: PASS.

## Task 6: Full Verification and Session Report

**Files:**
- Create: `docs/sessions/2026-04-25-lab-iut16-completion.md`

- [ ] **Step 1: Run focused tests**

Run: `cargo test -p lab --all-features marketplace::update::tests -- --nocapture`

Expected: PASS.

- [ ] **Step 2: Run broad required Rust verification**

Run: `cargo test -p lab --all-features`

Expected: PASS.

Run: `cargo clippy -p lab --all-features -- -D warnings`

Expected: PASS.

- [ ] **Step 3: Gather session report context**

Run the exact commands requested in the user prompt and capture factual outputs without secrets.

- [ ] **Step 4: Write session report**

Create `docs/sessions/2026-04-25-lab-iut16-completion.md` with YAML metadata and all required sections.

- [ ] **Step 5: Final closeability check**

Run `bd show lab-iut1.6` if needed for final evidence and report whether all required task/testing bullets are satisfied.

---

## Execution Notes

- Focused RED test initially failed because `artifact.config.set` was unknown.
- Implemented catalog, dispatch routing, config set, preview, apply, merge suggest guardrails, and error docs.
- Focused verification passed: `cargo test --manifest-path crates/lab/Cargo.toml --lib --all-features marketplace::update::tests -- --nocapture` reported 10 passed.
- Broad `cargo test --manifest-path crates/lab/Cargo.toml --all-features` failed in unrelated API router tests at `crates/lab/src/api/router.rs:611`.
- Broad `cargo clippy --manifest-path crates/lab/Cargo.toml --all-features -- -D warnings` failed on unrelated warnings outside `crates/lab/src/dispatch/marketplace/update.rs` after owned warnings were fixed.
