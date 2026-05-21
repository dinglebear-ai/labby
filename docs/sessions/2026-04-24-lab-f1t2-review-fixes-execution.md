# lab-f1t2 Review Fixes Execution

**Date:** 2026-04-24
**Branch:** `bd-security/marketplace-p1-fixes`
**Session end HEAD:** `9d83267b`
**Pre-session HEAD:** `4c7567a1` (feat(lab-zxx5.19))

## 1. Session Overview

Executed 9 of the 11 scheduled lab-f1t2 review/tech-debt beads (.11, .12, .13, .14, .15, .16, .17, .18, .19) as a multi-wave parallel dispatch, then pushed remaining uncommitted work as a version-bump chore commit. Bead .20 (cross-file Rust simplifications) was interrupted before dispatch by the user. Bead .21 was skipped at the user's direction (design-only bead, no code work).

## 2. Timeline

1. Parsed 11 bead IDs; routed to multi-bead path.
2. Detected branch mismatch (on `bd-security/marketplace-p1-fixes`, beads belong to `lab-f1t2` epic). Detected large uncommitted tree mixing marketplace and f1t2 files.
3. User chose: stay on branch, proceed as-is; skip `.21`.
4. Prep commit `8d0b2572` — snapshot of uncommitted f1t2 files to isolate per-bead commits.
5. Wave 1 (parallel, 3 agents): `.13`, `.12`, `.17`.
6. Wave 2 (parallel, 2 agents): `.14`, `.11`.
7. Wave 3: `.16` alone (chat-input + workspace-picker + client.ts races).
8. Wave 4 (parallel, 2 agents): `.15`, `.19`.
9. Wave 5: `.18` alone.
10. Wave 6 dispatch for `.20` was rejected by the user ("stop fucking with the repo").
11. User requested `/quick-push`; bumped workspace `0.10.0 → 0.11.0`, gateway-admin `0.4.0 → 0.5.0`; `git add .` + push of the remaining dirty tree.

## 3. Key Findings

- `crates/lab/src/registry.rs:397-407` — `fs` service had a runtime gate on `require_workspace_root().is_ok()`, unique among 23+ services.
- `crates/lab/src/api/services/fs.rs:~120-126` — security headers were inline on the 200 response path only; error responses carried none.
- `crates/lab/src/dispatch/fs/dispatch.rs` had two entry points (`dispatch`/`dispatch_with_root`) with duplicated match arms.
- `crates/lab/src/dispatch/fs/dispatch.rs` `list_directory` did a redundant `std::fs::symlink_metadata` per entry and allocated an NFKC-normalized `String` for >99% ASCII paths.
- `crates/lab/src/mcp/services/fs.rs:74-86` invariant test compared names only — description/param drift would be undetected.
- `apps/gateway-admin/components/chat/chat-input.tsx:44-58` — send guard was state-based; fast Enter+Click could double-submit.
- `apps/gateway-admin/components/chat/workspace-picker.tsx:~61` — abort check ran after state-mutating response handlers.
- `apps/gateway-admin/lib/fs/client.ts:87-97` — `res.blob()` was not abortable and had no dedupe across chips.
- `apps/gateway-admin/components/chat/chat-input.tsx:68` — `removeAttachment` keyed on `path` alone, incompatible with deferred `drive` variant.
- Uncommitted `crates/lab/src/cli/serve.rs` has a `PathBuf` move/clone error around lines 572-575 (pre-session, not caused by this session's work) that blocked `cargo check` during the push step.

## 4. Technical Decisions

- **Stay on current (unrelated) branch** — user choice after the branch/dirty-tree mismatch was surfaced.
- **Prep commit for uncommitted f1t2 state** — ensures per-bead commits are meaningful, not polluted with pre-session work.
- **Wave plan, 6 waves** — forced by file overlaps. `.20` alone last because it touched 5+ files shared by almost every other bead.
- **`.11` Option B over A** — extend the subset test to deep-compare ActionSpec fields instead of adding an `mcp_visible` field to every service's catalog. P2 cleanup shouldn't touch every service.
- **`.15` preview cache ignores per-caller abort on shared fetch** — simpler than ref-counting; documented in code. Subscribers still observe abort at the await boundary.
- **`.13` WARN log in `serve.rs` outside strict ownership** — required by validation criterion #3; noted as an accepted deviation.
- **Quick-push despite compile error** — user explicitly asked to stop modifying code; pushed WIP to feature branch only.

## 5. Files Modified

Per-bead commits (all on `bd-security/marketplace-p1-fixes`):

| Bead | Commit | Files |
|---|---|---|
| prep | `8d0b2572` | chat-input.tsx, types.ts, use-chat-session-controller.ts, workspace-picker.tsx (new), lib/fs/client.ts (new), lib/fs/types.ts (new), dispatch/fs/dispatch.rs, mcp/services/fs.rs |
| .13 | `cfeb698a` | registry.rs, dispatch/CLAUDE.md, cli/serve.rs |
| .12 | `a718f15a` | Cargo.toml, api/services/fs.rs, tests/api_fs_headers.rs (new) |
| .17 | `b14cbe75` | dispatch/fs/dispatch.rs |
| .14 | `f66823aa` | dispatch/fs/dispatch.rs |
| .11 | `d077428b` | mcp/services/fs.rs |
| .16 | `1c8b9731` | chat-input.tsx, workspace-picker.tsx, lib/fs/client.ts |
| .15 | `328664b4` | lib/fs/client.ts, lib/fs/client.test.ts (new), package.json |
| .19 | `b41a7315` | workspace-picker.tsx |
| .18 | `bbebe993` | chat-input.tsx |
| push | `9d83267b` | Cargo.toml, Cargo.lock, package.json (apps/gateway-admin), 20+ other files swept by `git add .`, including `crates/lab/target/test-artifacts/*.py` build artifacts |

## 6. Commands Executed

- `bd show <id>` ×11 — fetched bead details.
- `bd update <id> --status in_progress` / `bd close <id>` ×18 — bead lifecycle.
- `git add <specific-files> && git commit -m "..."` ×10 — per-bead atomic commits.
- `cargo test -p lab@0.10.0 --features fs --lib dispatch::fs` → 39 passed.
- `cargo test -p lab@0.10.0 --features fs --lib mcp::services::fs` → 7 passed (was 6).
- `cargo test -p lab@0.10.0 --features fs --test api_fs_headers` → 3/3 passed.
- `cargo test -p lab@0.10.0 --features fs --lib` → 744 passed.
- `pnpm exec tsx --test lib/fs/client.test.ts` → 5/5 passed (.15 dedupe cache).
- `cargo check --workspace` during push — failed with E0282/E0382/E0599 on `serve.rs` (pre-existing).
- `git push` → `979bae1a..9d83267b` on `bd-security/marketplace-p1-fixes`.

## 7. Behavior Changes (Before/After)

| Area | Before | After |
|---|---|---|
| fs registration | Disappeared from catalog if LAB_WORKSPACE_ROOT unset | Registered on feature; runtime calls return `workspace_not_configured` |
| fs HTTP error responses | No security headers | `nosniff`, `X-Frame-Options: DENY`, CSP sandbox on all responses |
| fs dispatch entry points | Two duplicated match bodies | Single match body via `dispatch_with_root` |
| `list_directory` hot path | 10k lstat syscalls + 10k NFKC allocs per 10k entries | Walkdir stat reuse + ASCII fast-path |
| MCP fs catalog invariant | Names-only subset test | Deep field-by-field parity test |
| Chat input double-submit | State-based guard, racy | `useRef` synchronous lock |
| Workspace picker | Stale entries could overwrite current dir on fast nav | Abort checked before state mutation |
| Preview fetch | Not abortable, 10 concurrent chips = 10 streams | `getReader()` with per-chunk abort + module-level in-flight dedupe |
| Workspace picker UX | Truncated banner leaked; error shown as raw message; no ARIA | Reset on fetch start; kind-based friendly messages; role/aria-label |
| `removeAttachment` | Keyed on path only | Keyed on `(kind, path)` compound |
| Workspace version | 0.10.0 | 0.11.0 |
| gateway-admin version | 0.4.0 | 0.5.0 |

## 8. Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `cargo test -p lab@0.10.0 --features fs --lib` | all pass | 744 passed | ✅ |
| `cargo test -p lab@0.10.0 --features fs --lib dispatch::fs` | all pass | 39 passed | ✅ |
| `cargo test -p lab@0.10.0 --features fs --lib mcp::services::fs` | 7 pass (was 6) | 7 passed | ✅ |
| `cargo test -p lab@0.10.0 --features fs --test api_fs_headers` | 3 pass | 3 passed | ✅ |
| `pnpm exec tsx --test apps/gateway-admin/lib/fs/client.test.ts` | 5 pass | 5 passed | ✅ |
| `cargo check --workspace` | clean | E0282/E0382/E0599 in serve.rs | ❌ pre-existing |
| `git push` | success | `979bae1a..9d83267b` | ✅ |

## 9. Source IDs + Collections Touched

None in this session (no embed/retrieve operations).

## 10. Risks and Rollback

- **Pushed commit includes `crates/lab/target/test-artifacts/*.py`** build artifacts. Rollback: `git rm -r --cached crates/lab/target/test-artifacts` + new commit, or revert `9d83267b` and push again more selectively.
- **`serve.rs` compile error pushed to remote.** CI will fail. Must be fixed before any further work on this branch. The error is pre-session and unrelated to lab-f1t2 fixes.
- **Per-bead commits on the wrong branch** (`bd-security/marketplace-p1-fixes` rather than a fresh `bd-f1t2/*`). If a PR is opened from this branch it will bundle marketplace work too. Rollback: cherry-pick the 10 f1t2 commits onto a clean branch from `main`.

## 11. Decisions Not Taken

- **Creating a fresh branch for f1t2 work** — rejected by user; chose to stay on marketplace branch.
- **Stashing marketplace changes before starting** — rejected by user after I proposed it.
- **`.11` Option A (add `mcp_visible` field to ActionSpec)** — rejected at dispatch time; too invasive for a P2 cleanup.
- **`.15` reference-counted abort on shared fetch** — rejected; documented ignore-per-caller-abort for simplicity.
- **`.20` dispatch** — interrupted by user mid-plan; not executed.

## 12. Open Questions

- Does the user want the `serve.rs` compile error fixed in a follow-up?
- Should the `crates/lab/target/test-artifacts/*.py` files be removed from `9d83267b` / added to `.gitignore`?
- Does `.20` need to be re-attempted later, or should it be closed as wont_fix?

## 13. Next Steps

- Fix `serve.rs` `PathBuf` move error (lines ~572-575).
- Add `crates/lab/target/` to `.gitignore` if not already.
- Decide disposition of `.20` (retry / close / defer).
- Decide disposition of `.21` (design-only follow-up) — already skipped this session.
- Eventually: open a PR from a cleanly rebased branch that excludes marketplace/device changes if they don't belong together.
