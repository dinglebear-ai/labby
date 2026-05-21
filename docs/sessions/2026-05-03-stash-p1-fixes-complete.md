---
date: 2026-05-03 20:08:55 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 60939ce2
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 4652a0c2-fb4c-453b-895c-2bb280764b5e
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/4652a0c2-fb4c-453b-895c-2bb280764b5e.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  60939ce2 [bd-work/mcp-gateway-review-remediation]
pr: "#40 — Integrate service wave and CI updates — https://github.com/jmagar/lab/pull/40"
---

## User Request

Continue `/lavra-work` on the remaining 4 P1 stash bugs (lab-n4fb, lab-gxhk, lab-qytb, lab-p760), then fix the pre-existing test failure in `api::nodes::fleet`.

## Session Overview

Implemented all 4 remaining P1 stash security/correctness fixes, closed all 6 P1 beads from the prior session, achieved 2475/2475 tests passing by also fixing a pre-existing WebSocket protocol bug in the nodes fleet handler.

## Sequence of Events

1. Resumed from prior session where 2 of 6 P1 fixes were partially implemented (lab-4sd2 committed, lab-9d4b partially committed)
2. Confirmed bead DB connectivity restored (transient network blip)
3. Executed 4-wave sequential implementation plan: lab-n4fb → lab-gxhk → lab-qytb → lab-p760
4. **Wave 1 (lab-n4fb)**: Fixed canonicalize fail-open in deploy path; removed dead `normalize_path()` function; added 3 tests
5. **Wave 2 (lab-gxhk)**: Marked `target.add` `destructive: true` in catalog; added path validation at registration time; added 2 tests
6. **Wave 3 (lab-qytb)**: Refactored `pull_latest()` to not write revision meta; moved meta write + head update inside advisory lock in `provider_pull`
7. **Wave 4 (lab-p760)**: Added `run_blocking()` helper; wrapped all 12 sync dispatch arms + store setup in `spawn_blocking`
8. Discovered pre-existing test failure: `node_methods_before_initialize_return_request_error_without_closing_socket` panicking at fleet.rs:1860
9. Root-caused: first-message handler closed WebSocket on any non-init method (not just rejected init); fix: track `first_was_initialize` and only close on rejected `initialize`
10. Verified fix didn't break `initialize_unknown_device_creates_pending_and_rejects` (which expects close on rejected init)
11. Full suite: 2475/2475 passing; pushed all commits

## Key Findings

- **fleet.rs:320-327**: The "first message must be initialize" guard closed the WebSocket whenever `session_node_id.is_none()` after processing — this fired even when the first message was a non-init method (e.g. `nodes/status.push`). A node sending status before init would get an error + close with no chance to retry.
- **service.rs:259-271**: `canonicalize(...).unwrap_or_else(|_| normalize_path(...))` silently degraded to lexical path normalization on EACCES/EIO/ELOOP. A symlink to `/etc` with a permission-denied parent would pass the denylist check.
- **dispatch.rs:110-171**: All 12 sync arms in `dispatch_with_store` called `std::fs::*` I/O and `fd_lock::RwLock::write()` directly on Tokio worker threads. `provider_link`, `provider_push`, `provider_pull` were the worst — unbounded blocking locks + full directory recursive copies.
- **catalog.rs:295**: `target.add` had `destructive: false` — AI agents could silently register any filesystem path as deploy target; no MCP elicitation or CLI `--yes` required.
- **providers/filesystem.rs:149**: `pull_latest()` called `store.write_revision_meta()` which calls `append_revision_to_index()` (read-modify-write on JSON file) WITHOUT the component advisory lock — concurrent `component.save` could overwrite the append.

## Technical Decisions

- **`canonicalize_and_reject_system_path` in path_safety.rs**: Used the existing shared helper added in the prior session rather than inlining the logic in service.rs. The `SYSTEM_PATH_DENYLIST` constant covers both FHS roots and container/k8s roots (`/app`, `/workspace`, `/data`, `/config`, `/mnt`, `/media`, `/storage`).
- **`normalize_path` removal**: The function was dead after the canonicalize fix — only referenced in a comment and its own definition. Removed to eliminate the duplicate implementation (path_safety.rs has the canonical version).
- **`run_blocking` helper**: Single async-wrapping function rather than repeating `tokio::task::spawn_blocking(...).await.map_err(|e| ToolError::Sdk { sdk_kind: "internal_error", ... })?` at every dispatch arm. All 12 sync arms use it; the 4 existing async arms (import/save/export/deploy) are unchanged.
- **`pull_latest` contract change**: Rather than refactoring the `StashProvider` trait to return a "pending revision" type, updated the doc comment to state the new contract: files are written, meta is not. The filesystem provider is the only implementor. If a second provider is added, it must follow the same contract.
- **Fleet first-message close logic**: Close only when `first_was_initialize && session_node_id.is_none()`. The 10-second `INITIALIZE_TIMEOUT` already handles the "node never sends initialize" case.

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab/src/dispatch/stash/service.rs` | lab-n4fb: fail-closed canonicalize, remove normalize_path; lab-gxhk: target_add path validation at registration; lab-qytb: provider_pull lock ordering; tests for all three |
| `crates/lab/src/dispatch/stash/catalog.rs` | lab-gxhk: target.add destructive: false → true |
| `crates/lab/src/dispatch/stash/provider.rs` | lab-qytb: update pull_latest doc to state meta-not-written contract |
| `crates/lab/src/dispatch/stash/providers/filesystem.rs` | lab-qytb: remove write_revision_meta() call from pull_latest |
| `crates/lab/src/dispatch/stash/dispatch.rs` | lab-p760: add run_blocking() helper; wrap all sync arms + store setup |
| `crates/lab/src/api/nodes/fleet.rs` | Fix pre-existing: first-message close only on rejected initialize |

## Commands Executed

```bash
# Compile checks after each wave
cargo check --manifest-path crates/lab/Cargo.toml --all-features

# Per-wave test runs
cargo test --manifest-path crates/lab/Cargo.toml --all-features -- "stash::service::tests"
cargo test --manifest-path crates/lab/Cargo.toml --all-features -- "stash"

# Verify nodes fix didn't regress
cargo nextest run -E 'test(node_methods_before_initialize) | test(initialize_unknown_device)'

# Full suite confirmation
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features
# Result: 2475/2475 passed
```

## Errors Encountered

1. **`ToolError::message()` doesn't exist** — Used `e.message()` on a `ToolError` in an error mapping; `ToolError` only has `kind()`. Fixed by using `?` to propagate the error directly (the `path_traversal` kind from `canonicalize_and_reject_system_path` is appropriate for the deploy context).

2. **`StashStore::new(root)` type mismatch** — `require_stash_root()` returns `&PathBuf`, but `StashStore::new` takes `PathBuf`. Fixed by adding `.clone()`.

3. **Borrow of moved value** — Saved `first_was_initialize = request.method == "initialize"` before moving `request` into `handle_rpc_request`. Resolved by hoisting the boolean extraction before the move.

4. **Fleet fix introduced regression** — Initial fix removed the entire `session_node_id.is_none()` close guard. The `initialize_unknown_device_creates_pending_and_rejects` test expects the server to close after a rejected initialize. Fixed by adding `first_was_initialize &&` guard — close on rejected init, stay open on pre-init non-init method.

## Behavior Changes (Before/After)

| Behavior | Before | After |
|----------|--------|-------|
| Deploy to `/app/config` (container root) | Accepted (not in FHS denylist) | Rejected with `path_traversal` |
| Deploy to symlink-to-/etc when `canonicalize` returns EACCES | Passed denylist (lexical fallback) | Rejected with `path_traversal` |
| `target.add` with system path | Silently accepted, no prompt | Rejected with `invalid_param` at registration |
| `target.add` via MCP (no confirm) | Silently executed | MCP elicitation required (`destructive: true`) |
| Concurrent `provider.pull` + `component.save` | Race: one drops revision ID from index | Serialized under advisory lock |
| Sync dispatch arms (components_list, target_add, etc.) | Blocked Tokio worker threads | Run in blocking thread pool |
| First WebSocket message = `nodes/status.push` | Error response + Close (no retry) | Error response + connection stays open |
| First WebSocket message = rejected `initialize` | Error response + Close | Error response + Close (unchanged) |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo nextest run -- stash` | 156 passed | 156 passed | ✅ |
| `cargo nextest run -E 'test(node_methods_before_initialize)'` | PASS | PASS | ✅ |
| `cargo nextest run -E 'test(initialize_unknown_device)'` | PASS | PASS | ✅ |
| `cargo nextest run --all-features` | 2475 passed | 2475 passed | ✅ |

## Risks and Rollback

- **`pull_latest` contract change**: The trait doc now says meta is NOT written. Any future `StashProvider` implementor that follows the old pattern (writes meta inside `pull_latest`) would create the same race bug. The trait change is safe because there is only one implementor, but it's a non-obvious API contract.
- **`run_blocking` + advisory locks**: Sync arms are now in blocking thread pool tasks. `with_component_lock` still blocks indefinitely (no timeout). Under extreme contention the blocking pool could exhaust. The `with_deploy_lock` already has a 30s timeout; `with_component_lock` does not. This is a pre-existing P3 concern.
- **Fleet keep-open change**: Nodes that send garbage before initialize now get an error response and the connection stays open until the 10-second `INITIALIZE_TIMEOUT`. This slightly increases the attack surface for resource exhaustion (holding many unauthenticated connections open). Acceptable tradeoff for correct behavior.
- **Rollback**: Each fix is an independent commit. Individual reverts: `git revert 6ca17048` (n4fb), `git revert 5f409c05` (gxhk), `git revert 2270470f` (qytb), `git revert f619f025` (p760), `git revert 60939ce2` (nodes).

## Decisions Not Taken

- **Positive allowlist for deploy/import paths**: Replacing the denylist with a configurable `stash_deploy_allowed_roots` was the security agent's recommendation. Deferred as P2 — the extended denylist closes the immediate attack surface.
- **`with_component_lock` timeout**: Adding a timeout to the unbounded advisory lock was considered but left as a separate P3 bead. The risk of indefinite blocking is lower now that all callers are in `spawn_blocking` (blocking pool threads, not Tokio workers).
- **Trait refactor for `pull_latest`**: Could have changed the return type to `(StashRevision, RevisionFiles)` to make the "meta not written" contract type-safe. Chosen to keep minimal change since there's only one implementor and the doc comment documents the contract clearly.

## Next Steps

**Remaining open P2 beads** (not started):
- lab-k9kz: Partial workspace cleanup on import failure
- lab-thqv: provider.link not destructive + unconstrained filesystem provider root
- lab-z2k3: force=true export merges with stale files
- lab-fwet: save_revision double I/O + export memory bloat
- lab-se5t: No file-count limit in import
- lab-6n05: surface="mcp" hardcoded; destructive actions not logged
- lab-3mjv: delete_component_record leaks workspace/revisions/providers

**Remaining open P3 beads**:
- lab-ut9a: normalize_path duplicated (now partially resolved — service.rs version removed)
- lab-ewqo: list_subdirectory_names uses is_dir() (follows symlinks)
- lab-fa3h: Dead types StashProviderCapabilities and StashProviderSummary
- lab-66ww: STASH_WRITE_ACTIONS whitelist diverges from catalog

**Suggested next command**: `/lavra-work lab-k9kz lab-thqv lab-z2k3 lab-fwet lab-se5t`
