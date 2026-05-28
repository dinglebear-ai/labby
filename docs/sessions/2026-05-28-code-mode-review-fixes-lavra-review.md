---
date: 2026-05-28 10:44:52 EST
repo: git@github.com:jmagar/lab.git
branch: fix/code-mode-review-fixes
head: b4e46881
session id: 0f1f017e-aaf9-4964-adbb-29f6ea3609eb
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/0f1f017e-aaf9-4964-adbb-29f6ea3609eb.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
pr: "#79 fix(mcp): address code review findings for search/execute Code Mode tools (https://github.com/jmagar/lab/pull/79)"
beads: lab-lt2vj, lab-lt2vj.1, lab-lt2vj.2, lab-lt2vj.3, lab-lt2vj.4, lab-lt2vj.5, lab-40vk7, lab-8v0tm, lab-l9n0n
---

# Code Mode review fixes and lavra-review session (2026-05-28)

## User Request

Continue alias-removal work from a compacted prior session — remove all remaining references to old Code Mode tool names (`tool_search`, `tool_execute`, `code_search`, `code_execute`, `scout`, `invoke`) from error messages, tracing fields, comments, and UI text; run `/lavra-review` on PR #79; triage and fix all P2 (and any pre-existing P1) findings from the review.

## Session Overview

Three-phase session: (1) completed alias removal across 8 source files and the gateway-admin UI component; (2) ran a full 6-agent lavra-review on PR #79 and filed findings as beads; (3) addressed all P1 and P2 findings — three introduced-code P2s and two pre-existing bugs (one P1, one P2). Branch is now clean of all introduced P1/P2 issues; two P3 deferred items remain open.

## Sequence of Events

1. Resumed from compaction; confirmed branch `fix/code-mode-review-fixes` at `d9c531d9` (alias-removal commit was already done pre-compaction)
2. Audited remaining old-name references in `code_mode.rs`, `manager.rs`, `semantic.rs`, `config.rs`, and the TSX component — found a batch of stale strings and tracing `service=` field values
3. Committed alias cleanup as `d9c531d9`: removed `"scout"` from config keys, updated all `service="tool_search"` / `service="scout"` tracing fields to `service="semantic"`, updated error messages and JS error strings in Code Mode runtime
4. Ran `/lavra-review` on PR #79; dispatched 6 agents in parallel: `rust-spec-reviewer`, `security-sentinel`, `silent-failure-hunter`, `architecture-strategist`, `pattern-recognition-specialist`, `code-simplicity-reviewer`
5. Synthesized findings; created parent bead `lab-lt2vj` plus 5 child beads and 3 pre-existing standalone beads; added LEARNED/PATTERN/MUST-CHECK knowledge entries to `lab-lt2vj`
6. Addressed P2 child beads:
   - `lab-lt2vj.1`: replaced `.expect()` in search/execute MCP handlers with `debug_assert! + let Some else` pattern
   - `lab-lt2vj.2`: added `tracing::warn!` to both empty-code and oversized-code `invalid_param` early-return paths in execute handler
   - `lab-lt2vj.3`: fixed stale `mode_label()` return values (`"tool_search_root"` → `"code_mode_root"`, `"tool_search_in_process_peer"` → `"code_mode_in_process_peer"`)
7. Addressed pre-existing P1 `lab-40vk7`: replaced silent `Ok(Value::Array(Vec::new()))` return in `search()` with `Err(ToolError::Sdk)` + `tracing::warn!`
8. Addressed pre-existing P2 `lab-8v0tm`: replaced `unwrap_or_else(|_| "null".to_string())` and `unwrap_or_else(|_| "{}".to_string())` in MCP search/execute success paths with explicit `match` blocks returning `CallToolResult::error` with `tracing::error!`
9. Ran `cargo fmt --all` after `debug_assert!` changes broke the fmt check
10. Committed all fixes as `b4e46881`
11. Pre-existing P2 `lab-l9n0n` (`build_error_extra` bypassing `ToolError`) assessed as wider architectural refactor — deferred; filed and left open
12. Deleted `crates/lab/src/cli/serve.rs.bak` (not git-tracked, already gitignored)
13. Invoked `/save-to-md`

## Key Findings

- **`mode_label()` stale values** (`crates/lab/src/mcp/catalog.rs:29-35`): `ToolSearchVisibility::mode_label()` is emitted as `visibility_mode` in every `list_tools ok` trace event. Values had not been updated when tool names were renamed from `tool_search_*` to `code_mode_*`. Would have poisoned production telemetry on every MCP session start.
- **Silent `Ok([])` on missing gateway** (`crates/lab/src/dispatch/gateway/code_mode.rs:240-249`): `CodeModeBroker::search()` returned an empty array when `gateway_manager` was `None`. MCP handler was protected by visibility guard, but the CLI Code Mode path could construct a `CodeModeBroker` without a manager and receive a silent empty result.
- **Silent serialization fallbacks** (`crates/lab/src/mcp/server.rs`, search ~1362, execute ~1516`): Both success paths used `unwrap_or_else(|_| "null"/"{}".to_string())` — a serialization failure would emit `CallToolResult::success` with a synthetic body. No log, no error signal.
- **`.expect()` in async MCP handlers** (`server.rs:1336, 1430`): The visibility guard proves `gateway_manager.is_some()` by contract, but `.expect()` panics in production if the invariant ever breaks. `debug_assert! + let-Some-else` costs nothing and preserves graceful degradation.
- **Missing tracing on execute error paths**: Empty-code and oversized-code validation rejections in the execute handler emitted no trace events, creating monitoring blind spots.
- **`serve.rs.bak` false positive**: Security agent flagged it as a committed file. `git show HEAD:crates/lab/src/cli/serve.rs.bak` confirmed it was not tracked — already gitignored. Only physical deletion needed.

## Technical Decisions

- **`debug_assert! + let Some else` vs. `.expect()`**: The guard at `exposes_synthetic_tools()` proves the invariant by convention, not by type. `debug_assert!` documents it for dev builds; the `let Some else` branch returns `CallToolResult::error` in production, matching the handler's existing error-return pattern. This is cheaper than making the type system enforce the invariant (would require refactoring the MCP server's field layout).
- **`lab-l9n0n` deferred**: `build_error_extra` is used 10+ times throughout `server.rs` for scope-denial paths. A spot-fix for two call sites would create inconsistency. The correct fix is a `ToolError::Forbidden { required_scopes }` variant — a spec change touching all three surfaces. Deferred as standalone P2 bead, does not block PR #79.
- **`service="semantic"` for gateway tracing**: The `tool_search`-related tracing events in `manager.rs` covered the semantic indexing/scoring subsystem, not the surface tool. Renaming to `service="semantic"` is more accurate and avoids confusion with the user-facing `search` tool name.
- **No `docs/plans/complete/` move**: Neither `fleet-ws-plan-lab-n07n.md` nor `mcp-streamable-http-oauth-proxy.md` was worked on or completed this session. Left as-is.

## Files Changed

| Status | Path | Purpose | Evidence |
|--------|------|---------|----------|
| modified | `crates/lab/src/mcp/catalog.rs` | Fix stale `mode_label()` return values (lab-lt2vj.3) | commit b4e46881 |
| modified | `crates/lab/src/mcp/server.rs` | debug_assert+let-Some-else, tracing::warn!, serialization error handling, test renames (lab-lt2vj.1/2, lab-8v0tm) | commits 55b861fa, b4e46881 |
| modified | `crates/lab/src/dispatch/gateway/code_mode.rs` | Fix silent Ok([]), rename old tool name strings (lab-40vk7) | commits d9c531d9, b4e46881 |
| modified | `crates/lab/src/dispatch/gateway/catalog.rs` | Action description: "tool_search/tool_execute mode" → "Code Mode (search/execute)" | commit d9c531d9 |
| modified | `crates/lab/src/dispatch/gateway/config.rs` | Remove `"scout"` from `KNOWN_LAB_CONFIG_KEYS` | commit d9c531d9 |
| modified | `crates/lab/src/dispatch/gateway/manager.rs` | Rename `service="tool_search"/"scout"` → `service="semantic"` in tracing (10 occurrences); update Code Mode error message | commit d9c531d9 |
| modified | `crates/lab/src/dispatch/gateway/semantic.rs` | Remove incorrect `#![allow(dead_code)]`; update module doc comment | commit d9c531d9 |
| modified | `crates/lab/src/config.rs` | Update comments, tracing fields, docstrings from old names | commit d9c531d9 |
| modified | `apps/gateway-admin/components/gateway/tool-search-toggle.tsx` | Display text: `code tool_search` → `search`, `tool_execute`/`code_execute` → `execute` | commit d9c531d9 |
| modified | `docs/generated/action-catalog.json` | Regenerated (removed `gateway.scout.*` alias actions) | commit 980c0d43 |
| modified | `docs/generated/action-catalog.md` | Regenerated | commit 980c0d43 |
| modified | `docs/generated/cli-help.md` | Regenerated | commit 980c0d43 |
| modified | `docs/generated/mcp-help.json` | Regenerated | commit 980c0d43 |
| modified | `docs/generated/mcp-help.md` | Regenerated | commit 980c0d43 |
| modified | `docs/generated/openapi.json` | Regenerated | commit 980c0d43 |
| deleted | `crates/lab/src/cli/serve.rs.bak` | Stale backup file; not git-tracked (gitignored); deleted physically | manual deletion |

## Beads Activity

| Bead | Title | Action | Final Status | Why it mattered |
|------|-------|--------|-------------|-----------------|
| `lab-lt2vj` | PR #79 review tracking | Created as parent for review beads | OPEN (P3 children remain) | Anchors all review findings for this PR |
| `lab-lt2vj.1` | Restore graceful error return: .expect() in search/execute handlers | Created → fixed → closed | CLOSED | `.expect()` in async MCP handler panics in production if invariant breaks |
| `lab-lt2vj.2` | Add missing tracing::warn! to execute handler invalid_param guards | Created → fixed → closed | CLOSED | Missing events create monitoring blind spots |
| `lab-lt2vj.3` | Fix stale mode_label() names in ToolSearchVisibility | Created → fixed → closed | CLOSED | Stale values poisoned telemetry on every MCP session start |
| `lab-lt2vj.4` | Hoist tool_search_visibility() to single snapshot in call_tool | Created | OPEN (P3, deferred) | Minor double-await; not blocking |
| `lab-lt2vj.5` | Add CHANGELOG.md entry for Code Mode rename | Created | OPEN (P3, deferred) | Documentation follow-up |
| `lab-40vk7` | search() silently returns empty array when gateway_manager is None | Created (pre-existing) → fixed → closed | CLOSED | Silent empty result indistinguishable from zero matches |
| `lab-8v0tm` | Serialization failure on search/execute success path returns null/{} silently | Created (pre-existing) → fixed → closed | CLOSED | Silent serialization failures reported as success |
| `lab-l9n0n` | MCP scope-denial envelope uses build_error_extra bypassing canonical ToolError | Created (pre-existing) | OPEN (P2, deferred) | Wider refactor needed; does not block PR #79 |

LEARNED/PATTERN/MUST-CHECK entries added to `lab-lt2vj`:
- LEARNED: `.expect()` in async MCP handlers converts graceful error-return to panic
- PATTERN: visibility-guard + debug_assert! + let-Some-else for production resilience
- MUST-CHECK: verify `exposes_synthetic_tools()` can only be true when `gateway_manager.is_some()`
- LEARNED: Every early-return error path must emit structured tracing per OBSERVABILITY.md
- PATTERN: Logging parity gap — when adding one trace event, audit all sibling early-return paths
- LEARNED: `mode_label()` is emitted in telemetry; renaming tool aliases without updating it leaves stale values
- MUST-CHECK: When renaming Code Mode tool names, grep for `mode_label()` in `mcp/catalog.rs`

## Repository Maintenance

- **Plans**: `docs/plans/fleet-ws-plan-lab-n07n.md` and `mcp-streamable-http-oauth-proxy.md` — neither was worked on or completed this session. Left as-is. `docs/plans/complete/` does not exist and was not created (no plans completed).
- **Beads**: All introduced P1/P2 findings closed; pre-existing `lab-l9n0n` left open (P2, wider refactor). `lab-lt2vj` remains open until P3 children are resolved.
- **Worktrees**: `/home/jmagar/workspace/lab` (fix/code-mode-review-fixes) and `/home/jmagar/workspace/lab-code-mode` (bd-work/code-mode-cloudflare-parity) — both active branches with open PRs or work, not cleaned up.
- **Stale docs**: `mcp/CLAUDE.md` still references `tool_search` and `code_search`/`code_execute` in the dispatch-pattern section. Minor comment staleness; not corrected this session (would be a separate non-blocking cleanup).
- **`serve.rs.bak`**: Physically deleted. Was not git-tracked; already gitignored. No git staging needed.

## Tools and Skills Used

- **Shell/Bash**: `cargo fmt --all`, `cargo check --all-features`, `just docs-generate`, `git` operations, `bd` CLI for bead management
- **File tools**: Read, Edit, Write for all code and documentation changes
- **Skills**: `lavra:lavra-review` (multi-agent review), `save-to-md` (session documentation)
- **Sub-agents**: 6 review agents dispatched in parallel by `lavra-review`: `rust-spec-reviewer`, `security-sentinel`, `silent-failure-hunter`, `architecture-strategist`, `pattern-recognition-specialist`, `code-simplicity-reviewer`
- **Issues**: `cargo fmt --check` failed after multi-line `debug_assert!` changes — resolved with `cargo fmt --all`

## Commands Executed

```bash
# Alias removal verification
grep -r "tool_search\|tool_execute\|code_search\|code_execute\|scout\|invoke" \
  crates/lab/src/dispatch/gateway/ crates/lab/src/config.rs apps/gateway-admin/

# Build verification (all-features, warnings as errors)
RUSTFLAGS="-D warnings" cargo check --all-features

# Format
cargo fmt --all

# Docs regeneration
just docs-generate

# Bead operations
bd create "PR #79: fix(mcp) — address code review findings" --labels "code-mode,fix/code-mode-review-fixes,pr-review"
bd create "lab-40vk7" --labels "pre-existing,review-sweep,bug"
# ... (multiple bd create/close calls)
```

## Errors Encountered

- **`cargo fmt --check` failed** after adding multi-line `debug_assert!` macro call. Root cause: multi-line macro body formatting differed from rustfmt's expectation. Fix: `cargo fmt --all` before commit.
- **`serve.rs.bak` false positive**: Security agent reported it as a committed secret-adjacent file. Verification with `git show HEAD:crates/lab/src/cli/serve.rs.bak` confirmed the file was not tracked (gitignored). Physical deletion was sufficient.
- **`bd create --tags` flag doesn't exist**: `bd create` uses `--labels` not `--tags`. Corrected immediately on first use.

## Behavior Changes (Before/After)

| Component | Before | After |
|-----------|--------|-------|
| `mode_label()` in `ToolSearchVisibility` | Emitted `"tool_search_root"` / `"tool_search_in_process_peer"` in telemetry | Emits `"code_mode_root"` / `"code_mode_in_process_peer"` |
| `search()` with no gateway_manager | Silently returned `Ok([])` — indistinguishable from zero results | Returns `Err(ToolError::Sdk)` + `tracing::warn!` |
| Serialization failure in search success path | Returned `CallToolResult::success("null")` silently | Returns `CallToolResult::error(...)` + `tracing::error!` |
| Serialization failure in execute success path | Returned `CallToolResult::success("{}")` silently | Returns `CallToolResult::error(...)` + `tracing::error!` |
| `gateway_manager.as_ref().expect(...)` in search/execute | Panic in production if invariant breaks | `debug_assert!` + graceful `CallToolResult::error` |
| execute empty-code / oversized-code validation | Rejected with no trace event | Rejects with `tracing::warn!` carrying surface/service/action/kind fields |
| `service=` field in gateway semantic tracing events | `"tool_search"` or `"scout"` | `"semantic"` |
| Code Mode enable-check error message | `"tool search is not enabled; tool_execute requires tool_search mode"` | `"Code Mode is not enabled; execute requires Code Mode to be enabled"` |
| `KNOWN_LAB_CONFIG_KEYS` in gateway config | Included `"scout"` | `"scout"` removed |
| gateway-admin UI toggle display text | `<code>tool_search</code>`, `<code>code_execute</code>` | `<code>search</code>`, `<code>execute</code>` |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `RUSTFLAGS="-D warnings" cargo check --all-features` | Zero errors and warnings | Zero errors and warnings | PASS |
| `cargo fmt --check` | No formatting changes needed | Passed after `cargo fmt --all` | PASS |
| `just docs-generate` | 15 artifacts regenerated | Completed successfully | PASS |
| `git show HEAD:crates/lab/src/cli/serve.rs.bak` | File not tracked | `fatal: Path '...' does not exist` | PASS (confirms not tracked) |

## Risks and Rollback

- **`mode_label()` change** is a telemetry-only change. Existing dashboards or alerts keyed on `visibility_mode="tool_search_root"` will no longer match. Low risk — the old values were stale aliases with no documented consumer.
- **Rollback**: All changes are on `fix/code-mode-review-fixes`; `git revert b4e46881` or `git revert d9c531d9` restores any individual commit. PR #79 has not been merged.

## Decisions Not Taken

- **`lab-l9n0n` immediate fix**: Replacing `build_error_extra` with `ToolError::Forbidden { required_scopes }` requires a new `ToolError` variant, updated CLI and HTTP surfaces, and coordination across all 10+ call sites in `server.rs`. Deferred to avoid scope creep in PR #79.
- **P3 items (`lab-lt2vj.4`, `lab-lt2vj.5`)**: Hoisting `tool_search_visibility()` to a single snapshot is a minor performance improvement with no correctness impact. CHANGELOG.md update is documentation. Both deferred to keep the PR focused on correctness fixes.

## References

- PR #79: https://github.com/jmagar/lab/pull/79
- OBSERVABILITY.md: `docs/dev/OBSERVABILITY.md`
- ERRORS.md: `docs/dev/ERRORS.md`
- MCP surface CLAUDE.md: `crates/lab/src/mcp/CLAUDE.md`
- Prior session (PR #78 analysis): `docs/sessions/2026-05-27-pr78-code-mode-review-fixes.md`

## Open Questions

- `mcp/CLAUDE.md` still references `tool_search`, `code_search`, `code_execute` in the dispatch-pattern section (the exception-layer description). This is comment staleness, not a correctness issue. Should it be updated in PR #79 or as a follow-up?
- `lab-l9n0n`: When the `ToolError::Forbidden` variant is added, should `required_scopes` be `Vec<String>` or `&'static [&'static str]`? The scope strings are all compile-time constants.

## Next Steps

**Immediate (unfinished scope):**
- None — all P1 and P2 introduced-code findings from lavra-review are resolved

**Follow-on (deferred, can proceed without blocking merge):**
- `lab-lt2vj.4`: Hoist `tool_search_visibility()` to single snapshot in `call_tool()` — `await` is called twice in the same handler
- `lab-lt2vj.5`: Add CHANGELOG.md entry for Code Mode rename (search/execute) and removed `gateway.scout.*` catalog entries
- `lab-l9n0n`: Implement `ToolError::Forbidden { required_scopes }` variant and replace all `build_error_extra` scope-denial call sites in `server.rs`

**Merge path:**
```bash
# PR #79 is ready to merge — no P1/P2 blockers on introduced code
gh pr merge 79 --squash   # or --merge per project preference
```

**Branch cleanup after merge:**
```bash
git branch -d fix/code-mode-review-fixes
```

**Other open dirty files** (unrelated to PR #79, separate work in progress):
- `Justfile`, `apps/gateway-admin/package.json`, `pnpm-lock.yaml`
- `crates/lab/src/cli.rs`, `cli/gateway.rs`, `cli/help.rs`, `cli/helpers.rs`
- `crates/lab/src/dispatch/gateway/projection.rs`
- `crates/lab/src/output/render.rs`
- `plugins/dozzle/.claude-plugin/plugin.json`, `plugins/lab/.claude-plugin/plugin.json`

These are separate work items not staged for PR #79 and should be handled in a separate branch/PR.
