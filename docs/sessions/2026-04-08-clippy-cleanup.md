# Session: Clippy Cleanup — 2026-04-08

## Session Overview

Executed the full clippy cleanup plan from `docs/superpowers/plans/2026-04-08-clippy-cleanup.md` in one batched pass. Reduced workspace warnings from 378 → 0, making `cargo clippy --workspace --all-features --all-targets -- -D warnings` exit cleanly and unblocking the lefthook pre-commit hook.

Also answered a tooling question mid-session: `.beads/` (Beads issue tracker backed by Dolt) and `.lavra/` (Lavra workflow/memory system) are both active in this repo.

---

## Timeline

| Time | Activity |
|------|----------|
| Start | Read plan file, collected full clippy JSON output baseline (55 warnings after Tasks 1–2 lint relaxations) |
| Task 1–2 | Relaxed `missing_docs` and `unused_async` workspace-wide in `Cargo.toml` (317 warnings removed) |
| Task 3 | Added `readme`/`keywords`/`categories` to both crate manifests (6 warnings removed) |
| Task 4 | Backtick-fixed 23 `doc_markdown` product names across 15 files |
| Task 5 | Fixed `core/http.rs`: `# Panics` doc, startup `#[allow(expect_used)]`, `+ Sync` bound on `post_json` |
| Task 6 | Fixed `radarr.rs`: `u64::try_from(...).unwrap_or(u64::MAX)` for latency; widened `ServiceClient` trait to `&'static str` |
| Task 7 | Rewrote `cli/install.rs` and `cli/completions.rs`: dropped `Result` wrappers, took args by reference |
| Task 8 | Added `#![allow(clippy::expect_used, clippy::unwrap_used)]` to both integration test files |
| Task 9 | Scattered fixes: `const fn`, `match_same_arms`, `collapsible_if`, `struct_excessive_bools`, `used_underscore_binding`, `or_fun_call`, `double_must_use`, `too_long_first_doc_paragraph`, `unnecessary_wraps` in `tui/app.rs`/`mcp/meta.rs`/`cli/plugins.rs` |
| Task 10 | `cargo fmt --all`, final green gate, empty commit to prove lefthook passes |
| Commit | Single batched commit `1c8a6f4` + empty verify commit `122ea7c` |

---

## Key Findings

- **378 warnings baseline** (commit `2bbe618`); 265 from `missing_docs` on servarr scaffold fields, 52 from `unused_async` on stub methods — 70% noise removed with 2 config lines.
- **`ServiceClient` trait** (`crates/lab-apis/src/core/traits.rs:15,20`) returned `&str` but impls returning `&'static str` are legitimate `unnecessary_literal_bound` fixes — trait signature had to be widened to `&'static str` since Rust requires exact match.
- **`post_json<B>` future** (`crates/lab-apis/src/core/http.rs:85`) was non-`Send` because `&B` captured across `.await` requires `B: Sync`. Adding `+ Sync` bound is zero-cost at call sites (all pass owned `Serialize` structs).
- **`too_long_first_doc_paragraph`** at `radarr/types.rs:53` traced to `radarr/types/filesystem.rs:1-4` — clippy attributes the module's `//!` doc to its `mod` declaration site.
- **`cargo fmt`** reformatted 6 files not touched by the plan (`doctor.rs`, `serve.rs`, `mcp/services/{extract,radarr}.rs`, `radarr/client/system.rs`, `api/router.rs`) — pre-existing style drift, folded into the same commit.
- **`.beads/`** is a Beads (git-native issue tracker, `bd` CLI, Dolt-backed) instance; **`.lavra/`** is the Lavra workflow/memory system with `knowledge.jsonl` + `recall.sh`.

---

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| Relax `missing_docs` workspace-wide | 265 warnings on scaffold fields that will be rewritten per-service; re-enable in a dedicated docs pass |
| Relax `unused_async` workspace-wide | 52 stubs become legitimately async once `self.http.*` calls land; self-healing |
| `#[allow(expect_used)]` item-level on `HttpClient::new` | Startup TLS failure is unrecoverable — panicking at init is better than silently failing on first request |
| `#[allow(struct_excessive_bools)]` on `Notification`/`SystemStatus` | DTOs mirror Radarr's API shape 1:1; refactoring would break serde compat |
| File-level `#![allow]` in test files only | Integration tests should fail loud; workspace lint too broad here |
| Single batched commit vs 9 per-task commits | User explicitly requested "batch the whole plan" |
| No crate-level `#![allow]` added anywhere | Preserves future warning visibility; item-level only |

---

## Files Modified

| File | Change |
|------|--------|
| `Cargo.toml` | Relax `missing_docs` → `allow`; add `unused_async = "allow"` |
| `crates/lab-apis/Cargo.toml` | Add `readme`, `keywords`, `categories` |
| `crates/lab/Cargo.toml` | Add `readme`, `keywords`, `categories` |
| `crates/lab-apis/src/core/traits.rs` | `fn name/service_type` return `&'static str` |
| `crates/lab-apis/src/core/http.rs` | `# Panics` doc, `#[allow(expect_used)]`, `B: Serialize + Sync` |
| `crates/lab-apis/src/core/auth.rs` | Backtick `Memos`, `ByteStash` |
| `crates/lab-apis/src/core/plugin.rs` | Backtick `SABnzbd`, `qBittorrent`, `Memos`, `Linkding`, `ByteStash`, `UniFi`, `OpenAI` |
| `crates/lab-apis/src/radarr.rs` | `u64::try_from(...).unwrap_or(u64::MAX)`; `&'static str` impl |
| `crates/lab-apis/src/radarr/client/download_clients.rs` | Backtick `SABnzbd`, `qBittorrent` |
| `crates/lab-apis/src/radarr/types/filesystem.rs` | Split `//!` first paragraph; backtick `OpenAPI` |
| `crates/lab-apis/src/radarr/types/import_list.rs` | Backtick `IMDb` |
| `crates/lab-apis/src/radarr/types/movie.rs` | Backtick `IMDb` |
| `crates/lab-apis/src/radarr/types/queue.rs` | Backtick `OpenAPI` |
| `crates/lab-apis/src/servarr/types.rs` | Backtick `OpenAPI` |
| `crates/lab-apis/src/servarr/types/command.rs` | Backtick `OpenAPI` |
| `crates/lab-apis/src/servarr/types/download_client.rs` | Backtick `SABnzbd`, `qBittorrent` |
| `crates/lab-apis/src/servarr/types/indexer.rs` | Backtick `OpenAPI` |
| `crates/lab-apis/src/servarr/types/notification.rs` | Backtick `OpenAPI`; `#[allow(struct_excessive_bools)]` |
| `crates/lab-apis/src/servarr/types/protocol.rs` | Backtick `OpenAPI`, `BitTorrent` |
| `crates/lab-apis/src/servarr/types/quality.rs` | Backtick `OpenAPI` |
| `crates/lab-apis/src/servarr/types/release.rs` | Backtick `OpenAPI` |
| `crates/lab-apis/src/servarr/types/system.rs` | `#[allow(struct_excessive_bools)]` |
| `crates/lab-apis/src/servarr/types/tag.rs` | Backtick `OpenAPI` |
| `crates/lab-apis/src/extract/transport.rs` | Rename `_path`/`_dir` → `path`/`dir` |
| `crates/lab-apis/src/extract/types.rs` | `const fn path()`; `ok_or` → `ok_or_else` |
| `crates/lab-apis/src/openai/client.rs` | Backtick `OpenAI` |
| `crates/lab-apis/src/openai/types.rs` | Backtick `OpenAI` |
| `crates/lab-apis/tests/http_client.rs` | `#![allow(clippy::expect_used, clippy::unwrap_used)]` |
| `crates/lab-apis/tests/radarr_health.rs` | `#![allow(clippy::expect_used, clippy::unwrap_used)]` |
| `crates/lab/src/api/error.rs` | Merge duplicate match arms; `const fn kind()` |
| `crates/lab/src/api/router.rs` | Drop redundant `#[must_use]` on `build_router` |
| `crates/lab/src/cli.rs` | Wrap stub returns in `Ok(...)` |
| `crates/lab/src/cli/completions.rs` | Drop `Result` wrapper; take `args: &CompletionsArgs` |
| `crates/lab/src/cli/install.rs` | Drop `Result` wrappers; take args by reference; drop `anyhow` import |
| `crates/lab/src/cli/plugins.rs` | Drop `Result` wrapper |
| `crates/lab/src/config.rs` | Collapse 2 nested `if`s into `if let … && …` let-chains |
| `crates/lab/src/mcp/envelope.rs` | `const fn kind()` |
| `crates/lab/src/mcp/meta.rs` | Drop `Result` wrapper; return `ToolEnvelope<Catalog>` directly |
| `crates/lab/src/tui/app.rs` | Drop `Result` wrapper; `run()` returns `()` |

---

## Commands Executed

```bash
# Baseline warning count (after Tasks 1-3)
cargo clippy --workspace --all-features --all-targets 2>&1 | tail -5
# → 55 warnings

# JSON extraction of all warning sites
cargo clippy --workspace --all-features --all-targets --message-format=json > /tmp/clippy.json
# → 431 lines; python3 parsed to structured per-warning list

# Final green gate
cargo clippy --workspace --all-features --all-targets -- -D warnings
# → "No issues found"

cargo fmt --all --check   # → had drift; ran cargo fmt --all
cargo test --workspace --all-features  # → 4 passed

# Lefthook verify
git commit --allow-empty -m "chore: verify lefthook pre-commit gate passes"
# → hook ran, passed, commit landed as 122ea7c
```

---

## Behavior Changes (Before/After)

| Before | After |
|--------|-------|
| `cargo clippy … -D warnings` → 378 warnings, exit 1 | → 0 warnings, exit 0 |
| Lefthook pre-commit hook blocked every commit | Passes cleanly without `--no-verify` |
| `post_json<B>` future non-`Send` (missing `B: Sync`) | Future is `Send`; safe to spawn across threads |
| `ServiceClient::name/service_type` returned `&str` (lifetime-tied to `&self`) | Return `&'static str` (correct for literal returns) |
| Nested `if`s in `config.rs:56,117` | Collapsed to `if let … && …` let-chains |
| `cli/install`, `cli/completions`, `cli/plugins`, `tui/app`, `mcp/meta` returned `Result<T>` unnecessarily | Return `T` directly; callers wrap in `Ok(...)` |
| `latency_ms: start.elapsed().as_millis() as u64` (truncates after 584M years) | `u64::try_from(…).unwrap_or(u64::MAX)` |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo clippy … -- -D warnings` | exit 0, no output | exit 0, "No issues found" | ✅ |
| `cargo clippy … --all-targets -- -D warnings` | exit 0 | exit 0, "No issues found" | ✅ |
| `cargo fmt --all --check` | exit 0 | exit 0 (after `cargo fmt --all`) | ✅ |
| `cargo test --workspace --all-features` | all pass | 4 passed, 5 suites | ✅ |
| `git commit --allow-empty` (lefthook) | hook passes | landed as `122ea7c` | ✅ |

---

## Source IDs + Collections Touched

No vector/embedding collections used in this session.

---

## Risks and Rollback

- **Low risk.** All changes are lint suppressions, doc edits, or trivial refactors. No business logic altered.
- **`B: Sync` on `post_json`** — only risk is a call site passing a non-`Sync` body type. All current call sites pass owned serde structs which are trivially `Sync`. If a future call site uses a non-`Sync` body, the compiler will catch it.
- **`ServiceClient` trait widened to `&'static str`** — only one impl (`RadarrClient`). All future impls returning string literals will satisfy this automatically.
- **Rollback:** `git revert 1c8a6f4` restores all 44 files in one step.

---

## Decisions Not Taken

| Alternative | Why Rejected |
|-------------|-------------|
| Re-enable `missing_docs` with stub doc comments | Produces drive-by churn on every service plan; real docs should land with real types |
| Re-enable `unused_async` with `#[allow]` per-stub | 52 individual `#[allow]`s is noise; workspace-level is cleaner and self-heals |
| Per-task commits (9 commits) | User explicitly requested batching |
| Crate-level `#![allow(…)]` for test files | Hides future hits in the whole crate; file-level is the minimum scope |
| Refactor `Notification`/`SystemStatus` bools into bitflags | These DTOs mirror upstream API shape 1:1; refactoring breaks serde round-trip |
| `unwrap_or(0)` for latency truncation | Loses information on impossibly long health check; `u64::MAX` is a more honest sentinel |

---

## Open Questions

- `lefthook.yml` runs `cargo clippy --workspace --all-features -- -D warnings` (without `--all-targets`). Tests and examples are therefore not checked on commit — only lib/bin targets. This is consistent with the plan's scope but means test-file warnings only surface in CI.
- The two workspace `allow`s (`missing_docs`, `unused_async`) need a tracking mechanism to ensure they get re-enabled. Currently documented only in `Cargo.toml` TODO comments and the plan file; no bead created.

---

## Next Steps

1. Create a Beads bead tracking re-enablement of `missing_docs` once all 21 services have real types.
2. Create a Beads bead tracking re-enablement of `unused_async` once stub HTTP calls are wired.
3. The `cli.rs` diff visible in the session shows all 20 service subcommands are now registered — those new CLI shim files (`apprise.rs`, `bytestash.rs`, etc.) under `crates/lab/src/cli/` are untracked (`??` in git status at session start) and need to be committed on the feature branch.
