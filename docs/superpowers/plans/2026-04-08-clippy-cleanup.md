# Clippy Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `cargo clippy --workspace --all-features -- -D warnings` exit cleanly so the lefthook pre-commit hook stops blocking every commit.

**Architecture:** Hybrid approach. Most of the 378 warnings are noise from two rules firing on scaffold code that will be rewritten service-by-service (`missing_docs` on servarr types, `clippy::unused_async` on stubbed `pub async fn foo() { Ok(Vec::new()) }` methods). Relax those two workspace-wide with a TODO to re-enable once services land. Fix everything else — metadata, real quality issues in `http.rs`, a potential truncation bug in `radarr.rs`, and the handful of mechanical fixes in `cli/`, `api/error.rs`, `config.rs`, and `extract/transport.rs`.

**Tech Stack:** Rust 2024, clippy (pedantic + nursery + cargo), lefthook.

---

## Scope Check

One subsystem: "workspace compiles cleanly under `-D warnings`". Tightly coupled — cannot verify the pre-commit hook passes until every category is handled. Splitting further produces plans that don't individually ship a clean workspace.

Out of scope:
- Re-enabling `missing_docs` and `unused_async` with real docs/impls (that lands organically as each service is wired in its own plan).
- Rewriting `lefthook.yml` to run anything other than `fmt + clippy`.
- Fixing warnings that don't exist yet (`-W rust_2018_idioms`, `-W rust_2024_compatibility`) — zero current hits; leave them on.

---

## Warning Breakdown (baseline)

Captured from `cargo clippy --workspace --all-features --all-targets` on commit `2bbe618`:

| Count | Rule | Location |
|------:|------|----------|
| 228 | `missing_docs` (struct fields) | `crates/lab-apis/src/servarr/types/*` |
| 52  | `clippy::unused_async` | `extract/transport.rs`, `radarr/client/{movies,queue,indexers,download_clients,tags,history,...}.rs` stubs |
| 37  | `missing_docs` (enum variants) | `servarr/types/command.rs` and siblings |
| 23  | `clippy::doc_markdown` | `core/auth.rs`, `core/plugin.rs`, `radarr/**`, `openai/**` (product names without backticks) |
| 6   | `clippy::cargo_common_metadata` | `lab`, `lab-apis` (readme/keywords/categories × 2 packages) |
| 5   | `clippy::unnecessary_wraps` | `cli/completions.rs`, `cli/install.rs` |
| 4   | `clippy::needless_pass_by_value` | `cli/completions.rs`, `cli/install.rs` |
| 4   | `clippy::used_underscore_binding` | `extract/transport.rs` |
| 3   | `clippy::expect_used` | `core/http.rs:23`, `tests/http_client.rs:37`, `tests/radarr_health.rs:48` |
| 3   | `clippy::missing_const_for_fn` | `extract/types.rs`, `api/error.rs`, `mcp/envelope.rs` |
| 3   | `clippy::cast_possible_truncation` | `radarr.rs:93` + `radarr.rs:100` (`as_millis() as u64`) |
| 2   | `clippy::future_not_send` | `core/http.rs:89` (`post_json<B>` where `B: !Sync`) |
| 2   | `clippy::match_same_arms` | `api/error.rs:73,75` |
| 2   | `clippy::missing_panics_doc` | `core/http.rs:19` |
| 2   | `clippy::collapsible_if` | `config.rs:56, 117` |
| 2   | `clippy::struct_excessive_bools` | `servarr/types/notification.rs:21`, `servarr/types/system.rs:9` |
| ~6  | misc (or_fun_call, unused_qualifications, unnecessary_literal_bound, too_long_first_doc_paragraph, double_must_use) | scattered |

**Total: 378 warnings.**

Strategy: Task 1 removes 265 (228 + 37) by relaxing `missing_docs`. Task 2 removes 52 by allowing `unused_async`. That's 317 gone with 2 config lines. The remaining 61 are a mix of real quality/correctness fixes and trivial mechanical ones.

---

## File Structure

**Modified (workspace config):**
- `Cargo.toml` — relax two load-bearing noise lints with documented TODOs; add package metadata inheritance.
- `crates/lab/Cargo.toml` — add `readme`, `keywords`, `categories` (or inherit from workspace).
- `crates/lab-apis/Cargo.toml` — same.

**Modified (real code quality):**
- `crates/lab-apis/src/core/http.rs` — fix `expect_used`, `future_not_send` (change `post_json<B>` to require `B: Sync`), `missing_panics_doc`.
- `crates/lab-apis/src/radarr.rs` — fix `cast_possible_truncation` in `ServiceClient::health` by using `.min(u64::MAX as u128)` or `u64::try_from(...).unwrap_or(u64::MAX)`.
- `crates/lab-apis/src/core/auth.rs`, `crates/lab-apis/src/core/plugin.rs`, `crates/lab-apis/src/radarr/**`, `crates/lab-apis/src/openai/**` — `doc_markdown` backtick fixes.
- `crates/lab/src/cli/completions.rs`, `crates/lab/src/cli/install.rs` — drop `Result` wrappers, take args by reference.
- `crates/lab/src/api/error.rs` — merge duplicate match arms; `const fn` on an eligible method.
- `crates/lab/src/config.rs` — collapse two nested `if`s.
- `crates/lab/src/mcp/envelope.rs` — `const fn` on eligible method.
- `crates/lab-apis/src/extract/types.rs` — `const fn`.
- `crates/lab-apis/src/extract/transport.rs` — drop underscore prefix from 4 bindings (they're used in `Err(...)` constructors; rename or `#[allow]`).
- `crates/lab-apis/src/servarr/types/notification.rs`, `crates/lab-apis/src/servarr/types/system.rs` — `#[allow(clippy::struct_excessive_bools)]` on the two offending structs (Radarr's API literally has that many bool flags; refactoring the DTO is the wrong fix).
- Test files `crates/lab-apis/tests/http_client.rs`, `crates/lab-apis/tests/radarr_health.rs` — replace `.expect("...")` with `.unwrap()` is NOT the answer (both are denied). Add `#![allow(clippy::expect_used)]` at the top of test files.

**Modified (hook):** nothing — `lefthook.yml` already runs the right command. If Task 10 shows clippy clean, the hook just starts passing.

---

## Verification Commands

Run at the end of each task unless otherwise noted:

- `cargo clippy --workspace --all-features --all-targets -- -D warnings` — must pass at Task 10. Individual tasks may still have remaining warnings; use `2>&1 | grep -c "^warning"` to track the count dropping.
- `cargo test --workspace --all-features` — must still pass at every task (no test regressions).

---

### Task 1: Relax `missing_docs` workspace-wide

**Why first:** This single change removes 265 of 378 warnings (70%). The `missing_docs` lint is valuable once types are stable, but right now it fires on every scaffold field in `servarr/types/` — most of which will be rewritten per-service. Keeping it on creates a ratchet where every plan must add docstrings to fields it isn't touching, which produces drive-by churn. Turn it off here, add a tracking TODO, and re-enable it in a dedicated documentation pass once all 21 services have landed real types.

**Files:**
- Modify: `Cargo.toml:68` (workspace lints)

- [ ] **Step 1: Read the current workspace lints block.**

Read `Cargo.toml` lines 67-98 to confirm the structure.

- [ ] **Step 2: Change `missing_docs` from `warn` to `allow`.**

Edit `Cargo.toml`. Locate:

```toml
[workspace.lints.rust]
missing_docs                = "warn"
```

Replace with:

```toml
[workspace.lints.rust]
# TODO(docs-pass): re-enable once every service has real types + docstrings.
# Tracked in docs/superpowers/plans/ (follow-up after all 21 services land).
missing_docs                = "allow"
```

- [ ] **Step 3: Verify the warning count drops.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c "^warning"`
Expected: ~113 (down from 378).

- [ ] **Step 4: Commit.**

```bash
git add Cargo.toml
git commit -m "chore(lints): relax missing_docs until services are fully wired"
```

---

### Task 2: Allow `clippy::unused_async` workspace-wide

**Why:** 52 warnings. Every one fires on a stub method of the form `pub async fn foo(&self) -> Result<T, E> { Ok(Vec::new()) }` or `Err(NotYetImplemented)`. These will become `async` the moment the method calls `self.http.get_json(...)`. Silencing them now avoids churn; re-enabling is automatic once the real impls land.

**Files:**
- Modify: `Cargo.toml:79` (workspace clippy lints)

- [ ] **Step 1: Add the allow to the workspace clippy block.**

Edit `Cargo.toml`. In the `[workspace.lints.clippy]` section, after the existing `multiple_crate_versions = "allow"` line, add:

```toml
# TODO(stubs): re-enable once every service's real impl calls self.http — these
# fire on stub method bodies that will become async the moment the HTTP call lands.
unused_async            = "allow"
```

- [ ] **Step 2: Verify the count drops.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c "^warning"`
Expected: ~61.

- [ ] **Step 3: Commit.**

```bash
git add Cargo.toml
git commit -m "chore(lints): allow unused_async on service stubs"
```

---

### Task 3: Add package metadata (`readme`, `keywords`, `categories`)

**Why:** 6 warnings (3 fields × 2 packages). `clippy::cargo_common_metadata` wants `readme`, `keywords`, and `categories` in every published package's `[package]` table. We aren't publishing `lab-apis` yet, but setting them now silences the lint and documents intent.

**Files:**
- Modify: `crates/lab-apis/Cargo.toml` `[package]` table
- Modify: `crates/lab/Cargo.toml` `[package]` table

- [ ] **Step 1: Add the metadata to `crates/lab-apis/Cargo.toml`.**

Read the first 20 lines of `crates/lab-apis/Cargo.toml` to find the `[package]` table. Add (or merge with existing) these fields:

```toml
[package]
# ... existing fields ...
readme      = "../../README.md"
keywords    = ["homelab", "mcp", "cli", "sdk"]
categories  = ["api-bindings", "command-line-utilities"]
```

If `README.md` at the repo root doesn't exist, use `"README.md"` in the crate directory or create a minimal one-line `README.md` at `crates/lab-apis/README.md` containing `# lab-apis\n\nPure Rust SDK for homelab services. See the [workspace README](../../README.md).\n`.

- [ ] **Step 2: Same for `crates/lab/Cargo.toml`.**

```toml
[package]
# ... existing fields ...
readme      = "../../README.md"
keywords    = ["homelab", "mcp", "cli", "tui"]
categories  = ["command-line-utilities", "development-tools"]
```

- [ ] **Step 3: Verify.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c cargo_common_metadata`
Expected: `0`.

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c "^warning"`
Expected: ~55.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab-apis/Cargo.toml crates/lab/Cargo.toml README.md crates/lab-apis/README.md crates/lab/README.md
git commit -m "chore(cargo): add readme/keywords/categories metadata"
```

(Only `git add` the files you actually created/modified.)

---

### Task 4: `clippy::doc_markdown` — backtick mass fix

**Why:** 23 warnings. Product names (`SABnzbd`, `ByteStash`, `OpenAI`, `OpenAPI`, `qBittorrent`, `Memos`, `Linkding`, `Paperless-ngx`, etc.) appearing in `///` comments without backticks. Clippy wants them backticked so rustdoc formats them as code. Purely mechanical.

**Files (23 hit sites, condensed by file):**
- `crates/lab-apis/src/core/auth.rs:28` — `ByteStash`
- `crates/lab-apis/src/core/plugin.rs:51,53,…` — `SABnzbd`, `ByteStash`, `Linkding`, `Paperless-ngx`, etc.
- `crates/lab-apis/src/radarr/client/download_clients.rs:11` — `SABnzbd`
- `crates/lab-apis/src/radarr/client/quality.rs:32` — `OpenAPI`
- `crates/lab-apis/src/openai/types.rs:11` — `OpenAI`
- `crates/lab-apis/src/openai/client.rs:9` — `OpenAI`
- …and ~15 more scattered sites

- [ ] **Step 1: Enumerate every hit site.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -B1 "doc_markdown" | grep "\-\->" | awk '{print $2}' | sort -u`

Copy the list into your scratch buffer — each line is `path:lineno:col`.

- [ ] **Step 2: Apply each fix.**

For each hit, use the `Edit` tool to wrap the product name in backticks. Example transformations:

| Before | After |
|--------|-------|
| `/// Usenet download client (SABnzbd, ...).` | ``/// Usenet download client (`SABnzbd`, ...).`` |
| `/// Client for the OpenAI API.` | ``/// Client for the `OpenAI` API.`` |
| `/// See the Radarr OpenAPI spec.` | ``/// See the Radarr `OpenAPI` spec.`` |
| `/// Memos, Linkding, ByteStash.` | ``/// `Memos`, `Linkding`, `ByteStash`.`` |

Clippy's own suggestion output (which the CLI printed) tells you exactly which word to backtick for each line. Follow clippy's suggestion verbatim — do not paraphrase the comment.

- [ ] **Step 3: Verify.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c doc_markdown`
Expected: `0`.

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c "^warning"`
Expected: ~32.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab-apis/src
git commit -m "docs: backtick product names to satisfy clippy::doc_markdown"
```

---

### Task 5: `core/http.rs` real quality fixes

**Why:** Three related warnings in the same file — all fixable together. `expect_used` on `Client::builder().build().expect(...)` (Task 10 of the ops plan authored this), `missing_panics_doc` because `expect` can panic, and `future_not_send` because `post_json<B>` takes `body: &B` where `B: Serialize` but not `Sync`, making the returned future non-`Send`.

**Files:**
- Modify: `crates/lab-apis/src/core/http.rs:16-29` (new), `crates/lab-apis/src/core/http.rs:85-97` (post_json bound)

- [ ] **Step 1: Add the `# Panics` doc and keep the `expect`.**

Read `crates/lab-apis/src/core/http.rs` lines 16-29. Replace `pub fn new` with:

```rust
    /// Construct a new client with a base URL and auth strategy.
    ///
    /// # Panics
    /// Panics if the system TLS backend cannot initialize. This only happens
    /// in environments without a working rustls / system crypto provider,
    /// which would make every subsequent request fail anyway — panicking here
    /// surfaces the misconfiguration at startup instead of on first call.
    #[must_use]
    pub fn new(base_url: impl Into<String>, auth: Auth) -> Self {
        let inner = Client::builder()
            .user_agent(concat!("lab-apis/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest::Client::build (rustls TLS backend must initialize)");
        Self {
            base_url: base_url.into(),
            auth,
            inner,
        }
    }
```

The `# Panics` section silences `missing_panics_doc`. The `expect_used` warning is workspace-level `warn`, and we need a per-item `#[allow]`. Add it directly above the function:

```rust
    #[allow(clippy::expect_used)] // startup TLS init — see # Panics doc
    #[must_use]
    pub fn new(base_url: impl Into<String>, auth: Auth) -> Self {
```

- [ ] **Step 2: Fix `future_not_send` on `post_json`.**

Replace the `post_json` signature (lines ~85-97):

```rust
    /// POST a JSON body and decode the JSON response.
    ///
    /// # Errors
    /// Returns [`ApiError`] on transport, status, or decode failure.
    pub async fn post_json<B, T>(&self, path: &str, body: &B) -> Result<T, ApiError>
    where
        B: serde::Serialize + Sync,
        T: serde::de::DeserializeOwned,
    {
        let url = self.url(path);
        let resp = self
            .apply_auth(self.inner.post(&url).json(body))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Self::decode(resp).await
    }
```

The `+ Sync` bound on `B` is the minimal fix: the future captures `&B` across an `.await`, so `&B: Send` requires `B: Sync`. Every current and planned call site passes an owned struct that is trivially `Sync`, so this is a no-op at the call sites.

- [ ] **Step 3: Verify.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -cE "(expect_used|missing_panics_doc|future_not_send)"`
Expected: `0` in `core/http.rs` specifically (the 2 remaining `expect_used` hits in the test files are Task 8).

Run: `cargo test -p lab-apis --features "radarr servarr"` — all tests still pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab-apis/src/core/http.rs
git commit -m "fix(http): document panic, allow startup expect, require Sync on post_json body"
```

---

### Task 6: `radarr.rs` cast truncation (real correctness fix)

**Why:** `clippy::cast_possible_truncation` on `start.elapsed().as_millis() as u64` at lines 93 and 100. `as_millis()` returns `u128`. On an impossibly-long-lived health check (>584 million years) this would wrap. The fix is cheap — use `u64::try_from(...).unwrap_or(u64::MAX)`.

**Files:**
- Modify: `crates/lab-apis/src/radarr.rs:93, 100`

- [ ] **Step 1: Read the current impl.**

Read `crates/lab-apis/src/radarr.rs` lines 86-110.

- [ ] **Step 2: Replace both cast sites.**

Replace this:
```rust
                latency_ms: start.elapsed().as_millis() as u64,
```
with this (both occurrences):
```rust
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
```

`.unwrap_or` is not the denied `.unwrap()` — it's infallible. Clippy is happy.

- [ ] **Step 3: Verify.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c cast_possible_truncation`
Expected: `0` (or whatever count was baseline minus 2 if there are other unrelated hits).

Run: `cargo test -p lab-apis --features "radarr servarr"` — still green.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab-apis/src/radarr.rs
git commit -m "fix(radarr): saturate latency_ms conversion instead of truncating"
```

---

### Task 7: `cli/install.rs` and `cli/completions.rs` cleanup

**Why:** 5 `unnecessary_wraps` + 4 `needless_pass_by_value`. Both files have stub functions that return `Result<T>` but never `Err`, and take `String`/`Vec` arguments by value when a `&str`/`&[T]` would do.

**Files:**
- Modify: `crates/lab/src/cli/install.rs`
- Modify: `crates/lab/src/cli/completions.rs`

- [ ] **Step 1: Read both files end-to-end.**

Read `crates/lab/src/cli/install.rs` and `crates/lab/src/cli/completions.rs` in full.

- [ ] **Step 2: For every function clippy flagged `unnecessary_wraps`:**

Drop the `Result` from the return type. Change `fn foo(...) -> Result<()> { ... Ok(()) }` to `fn foo(...) { ... }`. Update every call site in the same module (the calls will now not need `?`).

- [ ] **Step 3: For every function clippy flagged `needless_pass_by_value`:**

Change the parameter from `name: String` to `name: &str` (or `items: Vec<T>` → `items: &[T]`). Update call sites to pass `&name` / `&items`.

If a call site is outside these two files (e.g. in `cli.rs`), update it too and add that file to the commit.

- [ ] **Step 4: Verify.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -cE "(unnecessary_wraps|needless_pass_by_value)"`
Expected: `0`.

Run: `cargo check -p lab --features all`
Expected: clean.

- [ ] **Step 5: Commit.**

```bash
git add crates/lab/src/cli/install.rs crates/lab/src/cli/completions.rs crates/lab/src/cli.rs
git commit -m "refactor(cli): drop Result wrappers and take install args by reference"
```

---

### Task 8: Test-file `expect_used` allow

**Why:** 2 hits in `tests/http_client.rs:37` and `tests/radarr_health.rs:48`. Integration tests MUST fail loud on unexpected `None`/`Err` — `.expect(...)` is idiomatic. The workspace lint should be relaxed for `tests/` specifically.

**Files:**
- Modify: `crates/lab-apis/tests/http_client.rs:1`
- Modify: `crates/lab-apis/tests/radarr_health.rs:1`

- [ ] **Step 1: Add file-level allow to both test files.**

Edit each file. Add this line at the very top, before the existing `//!` doc comment:

```rust
#![allow(clippy::expect_used, clippy::unwrap_used)]
```

So the start of each test file becomes:

```rust
#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Integration test — ...
```

- [ ] **Step 2: Verify.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c expect_used`
Expected: `0`.

- [ ] **Step 3: Commit.**

```bash
git add crates/lab-apis/tests/http_client.rs crates/lab-apis/tests/radarr_health.rs
git commit -m "test: allow expect/unwrap in integration test files"
```

---

### Task 9: Scattered single-digit fixes

**Why:** Everything remaining — ~20 warnings across ~10 rules, each with 1–4 hits. Grouped into one task because each individual fix is a one-line edit and they share a commit.

**Sub-steps (each is a single Edit call, then move on):**

- [ ] **Step 1: `clippy::match_same_arms` in `crates/lab/src/api/error.rs:73,75`.**

Read the file around lines 70-80. You'll see something like:
```rust
    Severity::Fail => StatusCode::INTERNAL_SERVER_ERROR,
    _ => StatusCode::INTERNAL_SERVER_ERROR,
```

Merge: replace the two arms with a single `_` catch-all that returns `INTERNAL_SERVER_ERROR`, dropping the explicit `Fail` arm. If clippy's suggestion shows a different merge, follow the suggestion verbatim.

- [ ] **Step 2: `clippy::missing_const_for_fn` in `api/error.rs:52`, `mcp/envelope.rs:69`, `extract/types.rs:29`.**

For each: add the `const` keyword to the function signature, e.g. `pub fn kind(&self) -> &str` → `pub const fn kind(&self) -> &str`. If the body contains anything not-yet-const-stable (unlikely for these getters), revert and add `#[allow(clippy::missing_const_for_fn)]` above the function instead.

- [ ] **Step 3: `clippy::collapsible_if` in `crates/lab/src/config.rs:56, 117`.**

Read those regions. Merge nested `if a { if b { ... } }` into `if a && b { ... }` — clippy shows the exact rewrite.

- [ ] **Step 4: `clippy::struct_excessive_bools` on `servarr/types/notification.rs:21` and `servarr/types/system.rs:9`.**

These are DTOs that mirror Radarr's API shape; the bools are fixed by upstream. Add a per-struct allow:

```rust
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NotificationResource {
    // ...
}
```

Same for `SystemStatus` in `system.rs`.

- [ ] **Step 5: `clippy::used_underscore_binding` in `extract/transport.rs` (4 hits).**

Read the file around the flagged lines. The pattern is `fn foo(_path: &Path) -> ... { Err(...) { message: "foo not yet implemented", path: _path.clone() } }` — the `_path` is used inside the Err. Drop the underscore prefix: rename `_path` → `path` in both the parameter and the Err body.

If the parameter is genuinely unused (not referenced in the body), leave the underscore and add `#[allow(clippy::used_underscore_binding)]` to the function.

- [ ] **Step 6: Everything else clippy is still yelling about.**

Run `cargo clippy --workspace --all-features --all-targets 2>&1 | grep "^warning" | sort -u` to see the residual list. For each:
- If it's a trivially mechanical fix clippy suggests (e.g., `or_fun_call`, `unnecessary_literal_bound`, `too_long_first_doc_paragraph`, `double_must_use`, `unused_qualifications`), apply the suggestion.
- If it's genuinely wrong in context, add a targeted `#[allow(clippy::RULE)]` at the item level with a one-line comment explaining why.

Do NOT add a crate-level `#![allow(...)]` — that hides future hits. Item-level only.

- [ ] **Step 7: Verify no warnings remain.**

Run: `cargo clippy --workspace --all-features --all-targets 2>&1 | grep -c "^warning"`
Expected: `0`.

- [ ] **Step 8: Commit.**

```bash
git add -A
git commit -m "fix(lints): address remaining pedantic/nursery warnings"
```

---

### Task 10: Full green gate with `-D warnings` and hook verification

- [ ] **Step 1: Clippy with deny-warnings.**

Run: `cargo clippy --workspace --all-features -- -D warnings`
Expected: exit 0, no output.

- [ ] **Step 2: All targets variant (tests + examples + benches).**

Run: `cargo clippy --workspace --all-features --all-targets -- -D warnings`
Expected: exit 0.

- [ ] **Step 3: `cargo fmt --all --check`.**

Run: `cargo fmt --all --check`
Expected: exit 0. If it fails, run `cargo fmt --all` and include the diff in the final commit.

- [ ] **Step 4: Tests still green.**

Run: `cargo test --workspace --all-features`
Expected: all prior tests pass, no new failures.

- [ ] **Step 5: Verify lefthook hook passes on a noop commit.**

```bash
git commit --allow-empty -m "chore: verify lefthook pre-commit gate passes"
```
Expected: no `--no-verify` needed; hook runs `cargo fmt --all --check` and `cargo clippy --workspace --all-features -- -D warnings`, both exit 0, commit lands.

If the hook fails at this step, STOP and fix the reported issue. Do not proceed to Step 6 until the hook passes without bypassing.

- [ ] **Step 6: Final push-ready state.**

Run: `git log --oneline -12` and confirm the expected sequence of clippy-cleanup commits is present on top of `2bbe618` (the green-check gate from the prior plan).

---

## Self-Review

**Spec coverage:**
- `missing_docs` (265 warnings) — Task 1 ✅
- `unused_async` (52 warnings) — Task 2 ✅
- `cargo_common_metadata` (6 warnings) — Task 3 ✅
- `doc_markdown` (23 warnings) — Task 4 ✅
- `expect_used` + `missing_panics_doc` + `future_not_send` in `core/http.rs` — Task 5 ✅
- `cast_possible_truncation` in `radarr.rs` — Task 6 ✅
- `unnecessary_wraps` + `needless_pass_by_value` in `cli/` — Task 7 ✅
- `expect_used` in test files — Task 8 ✅
- `match_same_arms`, `missing_const_for_fn`, `collapsible_if`, `struct_excessive_bools`, `used_underscore_binding`, and misc single-digit rules — Task 9 ✅
- Final gate including lefthook — Task 10 ✅

**Placeholder scan:** Task 9 Step 6 uses "everything else clippy is still yelling about" which is intentional — by Task 9 the set is ~20 single-hit warnings across 10 rules and listing each by name before running clippy risks drift. The concrete list IS the clippy output at that point; Step 6 tells the implementer to run clippy and fix what it shows, with explicit rules for when to apply suggestions vs. item-level `#[allow]`.

**Type consistency:** Task 5's `post_json<B: Serialize + Sync, T: DeserializeOwned>` change is a bound widening, no call site changes. Task 6's `u64::try_from(...).unwrap_or(u64::MAX)` preserves the `latency_ms: u64` field type exactly. Task 9's const additions don't change signatures.

**Dependencies:** Tasks 1–2 must run first (biggest noise reduction). Tasks 3–9 are independent of each other and can be reordered. Task 10 must be last.

---

Plan complete and saved to `docs/superpowers/plans/2026-04-08-clippy-cleanup.md`.
