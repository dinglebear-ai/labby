---
date: 2026-04-23 21:34:43 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: 2013dbdd
plan: none
agent: Claude (claude-sonnet-4-6)
session id: ce8f1d8c-276f-4890-bfae-a30f2b259056
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/ce8f1d8c-276f-4890-bfae-a30f2b259056.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#29 — fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

## User Request

Run `/lavra:lavra-review` (no target) to review the current in-progress bead, then `/lavra:lavra-work all P1` to implement all P1 critical security findings.

## Session Overview

The lavra-review pass found the primary bead issue (`surface=mcp` in shared dispatch) already resolved on `main`. Three new P1 security vulnerabilities were discovered in the marketplace dispatch module. All three were fixed across three sequential implementation waves, committed atomically per bead, and shipped in PR #29. A subsequent quick-push swept unrelated uncommitted gateway-admin and AI-component work into a version-bump commit.

## Sequence of Events

1. Invoked `/lavra:lavra-review` — dispatched 6 specialist agents against bead `lab-kvji.10` (remove transport-specific surface logging from shared marketplace dispatch)
2. Confirmed primary bead issue already resolved: `surface="mcp"` logging correctly lives in `mcp/server.rs`, not in any dispatch file
3. Agents surfaced three P1 vulnerabilities in `dispatch/marketplace/`; created child beads lab-kvji.10.1, lab-kvji.10.2, lab-kvji.10.3
4. Invoked `/lavra:lavra-work all P1` — sequenced three waves (file-scope conflict: all touch `dispatch.rs`, waves 2+3 also touch `client.rs`)
5. **Wave 1 (lab-kvji.10.1):** Fixed path traversal in `parse_plugin_id` (params.rs) and added canonicalize+`starts_with` check in `source_path_for_plugin` (dispatch.rs)
6. **Wave 2 (lab-kvji.10.2):** Added `entry.file_type()` symlink guards to all four filesystem walkers: `walk_artifacts_into` and `copy_tree` in dispatch.rs; `sync_tree_to_target` and `preview_tree_sync` in client.rs
7. **Wave 3 (lab-kvji.10.3):** Added `installPath` validation in `installed_target_for_plugin` (dispatch.rs) — canonicalize+starts_with for existing paths, prefix check for non-existing absolute paths, Normal-component check for relative paths
8. Discovered Wave 1 `params.rs` commit was missed; staged and committed it separately
9. Pushed branch; created PR #29
10. `/lab:quick-push` swept remaining uncommitted gateway-admin component polish, 26 new AI TSX components, ACP docs, output theme changes into a version-bump commit (0.8.0 → 0.9.0)

## Key Findings

- `parse_plugin_id` (`params.rs:7`) joined both `name` and `marketplace` parts to filesystem paths with no component-level validation — `../../../../.ssh@market` would traverse outside `plugins_root`
- `source_path_for_plugin` (`dispatch.rs`) used `root.join(marketplace).join(name)` with no subsequent bounds check; added `canonicalize` + `starts_with(plugins_root)` as belt-and-suspenders after `parse_plugin_id` is called
- All four filesystem walkers used `source.is_dir()` / `path.is_dir()` (follows symlinks via `stat`) instead of `entry.file_type()` (uses `lstat`) — a symlink to `/etc/passwd` inside a plugin tree would be silently followed and copied
- `installed_target_for_plugin` (`dispatch.rs:637`) returned `record.install_path` raw from JSON without any check — a tampered `installed_plugins.json` could point `plugin.deploy` at arbitrary directories (e.g., `../../.ssh`)
- `std::fs::canonicalize()` requires the path to exist; used only as belt-and-suspenders after existence check; `Component::Normal` validation is the primary guard for non-existing paths

## Technical Decisions

- **`Component::Normal` as primary guard, canonicalize as secondary:** `canonicalize` panics on non-existent paths, so it cannot be the sole defense. `parse_plugin_id` validates eagerly before any path construction.
- **`entry.file_type()` over `path.is_dir()`:** `DirEntry::file_type()` calls `lstat()` (does not follow symlinks); `Path::is_dir()` calls `stat()` (does follow). Using `file_type` is the correct POSIX defense against symlink traversal. Warn-and-skip rather than error, matching the existing `name == ".git"` skip pattern.
- **`installed_target_for_plugin` two-path validation:** For existing paths, canonicalize resolves any residual symlinks and verifies containment. For non-existing paths (install target not yet created), an absolute path must start with `plugins_root` (string prefix); relative paths must contain only `Normal` components. This handles both fresh installs and already-deployed plugins.
- **Sequential waves instead of parallel:** All three P1 beads mutate `dispatch.rs`; waves 2 and 3 both mutate `client.rs`. File-scope overlap requires strict sequential ordering.
- **Atomic per-bead commits:** Commit format `fix(lab-kvji.10.N): description` enables `git log --grep="lab-kvji.10"` to reconstruct the fix sequence.

## Files Modified

| File | Change |
|------|--------|
| `crates/lab/src/dispatch/marketplace/params.rs` | Added `Component::Normal` validation loop for both `name` and `marketplace` parts of plugin ID |
| `crates/lab/src/dispatch/marketplace/dispatch.rs` | Added canonicalize+starts_with in `source_path_for_plugin`; symlink guards in `walk_artifacts_into` and `copy_tree`; full `installPath` validation in `installed_target_for_plugin` |
| `crates/lab/src/dispatch/marketplace/client.rs` | Added `entry.file_type()` symlink guards in `sync_tree_to_target` and `preview_tree_sync` |
| `apps/gateway-admin/components/ai/*.tsx` | 26 new AI TSX components (agent, artifact, attachments, chain-of-thought, code-block, commit, confirmation, context, environment-variables, file-tree, inline-citation, package-info, plan, prompt-input, queue, reasoning, sandbox, schema-display, snippet, sources, stack-trace, task, terminal, test-results, tool, web-preview) |
| `docs/acp/README.md`, `design.md`, `research-findings.md` | New ACP design and research documentation |
| `apps/gateway-admin/components/gateway/gateway-detail-content.tsx` | Component polish pass (567 lines changed) |
| `apps/gateway-admin/components/gateway/tool-exposure-table.tsx` | Component polish |
| `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx` | Component polish |
| `apps/gateway-admin/components/chat/message-bubble.tsx` | Component polish |
| `crates/lab/src/output/theme.rs` | New `ACCENT_STRONG` palette entry |
| `crates/lab/src/output/render.rs` | Render helper updates |
| `crates/lab/src/log_fmt/formatter.rs` | Formatter updates |
| `docs/design-system-contract.md` | Additions |
| `Cargo.toml` | Version bump 0.8.0 → 0.9.0 |
| `apps/gateway-admin/package.json` | Version bump 0.3.0 → 0.4.0 |
| `CHANGELOG.md` | Added 11 undocumented commits + new Highlights entries |

## Commands Executed

```bash
# Verification after each wave
cargo check --all-features
# → clean (1 pre-existing ACCENT_STRONG unused-constant warning)

# Tests (marketplace-specific — full test blocked by pre-existing compile errors)
cargo test --all-features -- marketplace
# → blocked by pre-existing errors in mcpregistry/store.rs and upstream/pool.rs (unrelated)

# Bead management
bd close lab-kvji.10.1 lab-kvji.10.2 lab-kvji.10.3

# PR creation
gh pr create --title "fix(marketplace): P1 security fixes ..."
# → https://github.com/jmagar/lab/pull/29
```

## Errors Encountered

| Error | Root Cause | Resolution |
|-------|-----------|------------|
| `bd create --tags` flag not found | Flag is `--labels` / `-l`, not `--tags` | Used `-l` flag |
| `bd dep relate X Y` failed | Subcommand takes `--blocks` flag, not two positional args | Used `bd dep {blocker} --blocks {blocked}` |
| Edit tool "File has not been read yet" | Attempted edit before read | Read with offset+limit before applying edit |
| Wave 1 `params.rs` missing from commits | Commit was not staged before push | Staged and committed separately before PR update |
| `cargo test -p lab --all-features` failed | Multiple crates named `lab`; `--all-features` incompatible with `-p` outside workspace | Used `cargo test --all-features -- marketplace` from workspace root |
| `git push` rejected (no upstream) | Branch was new, no `-u` flag | Used `git push -u origin bd-security/marketplace-p1-fixes` |

## Behavior Changes (Before/After)

| Behavior | Before | After |
|----------|--------|-------|
| Plugin ID `../../../../.ssh@market` | Joined to path, potential traversal | Rejected at parse time: non-Normal component detected |
| Plugin source path outside `plugins_root` | Allowed if `parse_plugin_id` passed | Blocked by canonicalize + `starts_with(plugins_root)` |
| Symlink inside plugin tree during artifact walk / copy / sync / preview | Followed silently (`is_dir()` = `stat`) | Skipped with `tracing::warn` (`file_type()` = `lstat`) |
| Tampered `installPath` in `installed_plugins.json` | Used as-is for deploy target | Validated: existing paths via canonicalize, non-existing absolute paths via prefix check, relative paths via Normal-component check |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --all-features` (after each wave) | 0 errors | 0 errors, 1 pre-existing warning | ✅ |
| `grep -r 'surface.*=.*mcp' crates/lab/src/dispatch/` | No matches | No matches | ✅ (primary bead issue already resolved) |
| `bd close lab-kvji.10.1 lab-kvji.10.2 lab-kvji.10.3` | All closed | All closed | ✅ |

## Risks and Rollback

- **Symlink skip is silent by default:** warn log at `tracing::warn` level; if a legitimate plugin uses symlinks, it will silently be omitted. Risk is low (plugin trees are human-authored) but the behavior change could surprise.
- **`installPath` validation rejects paths outside `plugins_root`:** Any existing install with a path outside `plugins_root` (e.g., manually edited JSON) will now surface a `InvalidParam` error on `plugin.deploy`. Correct fix is to update `installed_plugins.json` to a valid path.
- **Rollback:** `git revert cd8bfa9 ca66a3b 7c4fb9f` reverts all three P1 fixes cleanly. Each commit is atomic and independent.

## Decisions Not Taken

- **Error instead of warn-and-skip for symlinks:** Considered returning `ToolError` on symlink encounter, but warn-and-skip matches the existing `.git` / `node_modules` skip pattern and is less disruptive for callers.
- **Blocking `installPath` validation on absolute paths only:** Considered only rejecting paths with `..` regardless of absolute/relative — rejected because an absolute path like `/tmp/evil` passes `Component::Normal` checks but is clearly outside `plugins_root`. The `starts_with(plugins_root)` check for absolute paths is stronger.
- **Using `jailbreak`-style chroot sandbox:** Over-engineering for this use case; path validation at the Rust layer is sufficient and consistent with the rest of the codebase.

## References

- `crates/lab/src/dispatch/CLAUDE.md` — dispatch layer architecture and required service layout
- `docs/OBSERVABILITY.md` — logging field requirements (surface, service, action, elapsed_ms)
- PR #29: https://github.com/jmagar/lab/pull/29

## Open Questions

- Pre-existing compile errors in `mcpregistry/store.rs` (missing `extensions` field on `ResponseMeta`, unknown `version` field on `ServerResponse`) and `upstream/pool.rs` (missing `enabled` field on `UpstreamConfig`) prevent full `cargo test --all-features`. These are unrelated to this PR but block CI verification of the marketplace test suite.

## Next Steps

**Unfinished (started but not completed):**
- None — all three P1 waves are complete, committed, and the beads are closed.

**Follow-on (not yet started):**
- lab-kvji.10.4: Non-atomic write in marketplace JSON persistence + spawn_blocking for filesystem ops
- lab-kvji.10.5: Oversized payload guard on artifact reads
- lab-kvji.10.6: File locking for concurrent `installed_plugins.json` access
- lab-kvji.10.10: Test panic safety (replace `unwrap()` calls in marketplace tests)
- Resolve pre-existing compile errors in `mcpregistry/store.rs` and `upstream/pool.rs` to unblock `cargo test --all-features`
- Merge PR #29 once CI passes
