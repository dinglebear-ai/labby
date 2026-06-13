---
date: 2026-06-12 18:59:44 EDT
repo: git@github.com:jmagar/lab.git
branch: codex/fix-code-mode-mcp-app-callbacks
head: 293c1617
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab 293c1617 [codex/fix-code-mode-mcp-app-callbacks]
pr: none
beads: lab-zn7es
---

# Code Mode MCP App callbacks and Vibin repo-status

## User Request

Fix the Lab host bug where Code Mode allowed an MCP App UI tool to render but blocked its `callServerTool` callbacks to sibling tools, then stage, commit, push everything, build the latest release binary, and sync it to PATH and the container.

## Session Overview

This session fixed the Code Mode MCP App callback routing bug and moved the `repo-status` skill into the Vibin plugin bundle. The workspace version was bumped from `0.24.0` to `0.24.1`, the changelog was updated, and the related bead was closed after verification.

## Sequence of Events

1. Investigated Code Mode tool visibility and confirmed the existing bypass only allowed the MCP App UI tool itself.
2. Added a same-upstream MCP App sibling-tool lookup in the upstream pool.
3. Updated the MCP `call_tool` Code Mode gate to allow exposed sibling callbacks while keeping raw tools hidden from `list_tools`.
4. Added targeted regressions for sibling callback routing and route-scope behavior.
5. Moved the `repo-status` skill into `plugins/vibin` and refreshed marketplace/plugin metadata.
6. Bumped the workspace version to `0.24.1`, updated `Cargo.lock`, and documented the release in `CHANGELOG.md`.

## Key Findings

- `crates/lab/src/mcp/call_tool.rs:221` handled Code Mode hidden raw-tool gating and previously only bypassed for UI tools themselves or the broad `LAB_CODE_MODE_WIDGET_CALLBACKS` escape hatch.
- `crates/lab/src/dispatch/upstream/pool/tools.rs:150` is now the narrow allowlist for exposed tools whose upstream also exposes at least one MCP App UI tool.
- `crates/lab/src/mcp/handlers_tools/tests.rs:365` now proves a `youtube_probe`-style sibling callback no longer returns the hidden-tool envelope.

## Technical Decisions

- Kept ordinary raw sibling tools hidden from the model-facing `list_tools` output.
- Scoped callback reachability to exposed tools on the same healthy upstream that exposes an MCP App UI tool.
- Preserved the destructive-tool guard so unconfirmed callback paths cannot perform destructive upstream actions.
- Left the existing broad `LAB_CODE_MODE_WIDGET_CALLBACKS` opt-in in place for legacy/operator escape-hatch behavior.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `.agents/plugins/marketplace.json` | - | Refresh Codex marketplace metadata for Vibin/mcp-apps work | `git status --short` |
| modified | `.claude-plugin/marketplace.json` | - | Refresh Claude marketplace metadata for Vibin/mcp-apps work | `git status --short` |
| modified | `CHANGELOG.md` | - | Add `0.24.1` release notes | `git diff --name-status` |
| modified | `Cargo.toml` | - | Bump workspace version to `0.24.1` | `cargo check --manifest-path crates/lab/Cargo.toml --all-features` |
| modified | `Cargo.lock` | - | Sync locked package versions after workspace version bump | `cargo check --manifest-path crates/lab/Cargo.toml --all-features` |
| modified | `crates/lab/src/dispatch/upstream/pool/tools.rs` | - | Add MCP App sibling callback allowlist lookup and tests | targeted unit test passed |
| modified | `crates/lab/src/mcp/call_tool.rs` | - | Allow same-upstream MCP App sibling callbacks in Code Mode | targeted MCP test passed |
| modified | `crates/lab/src/mcp/handlers_tools/tests.rs` | - | Add regression for hidden raw tools vs sibling callback reachability | targeted MCP test passed |
| modified | `plugins/vibin/.claude-plugin/plugin.json` | - | Advertise repo-status skill in Claude plugin metadata | plugin validation previously passed |
| modified | `plugins/vibin/.codex-plugin/plugin.json` | - | Advertise repo-status skill in Codex plugin metadata | skill validation previously passed |
| created | `plugins/vibin/skills/repo-status/` | `/home/jmagar/.codex/skills/repo-status` | Move repo readiness audit skill into Vibin plugin source | installed cache refreshed earlier |

## Beads Activity

| id | title | action | final status | why |
|---|---|---|---|---|
| `lab-zn7es` | Code mode MCP Apps callServerTool access | Created, worked, closed | closed | Tracked the host-side fix for MCP App sibling callbacks blocked by Code Mode |

## Repository Maintenance

### Plans

`docs/plans/complete/mcp-streamable-http-oauth-proxy.md` was already complete. `docs/plans/fleet-ws-plan-lab-n07n.md` was left untouched because it was not part of this session.

### Beads

`lab-zn7es` was closed with verification evidence after the Code Mode callback fix and tests passed.

### Worktrees and branches

The worktree was moved from `main` to `codex/fix-code-mode-mcp-app-callbacks` before committing because the push workflow creates a feature branch from `main`.

### Stale docs

No stale docs beyond `CHANGELOG.md` were identified in the touched surface.

## Tools and Skills Used

- **Skills.** Used `superpowers:systematic-debugging` for the root-cause workflow and `vibin:quick-push` / `vibin:save-to-md` for closeout.
- **Shell commands.** Used `rg`, `sed`, `git`, `cargo`, `bd`, and `gh` for inspection, verification, tracker updates, and release prep.
- **File edits.** Used patch-based edits for Rust source, tests, changelog, version bump, and this session note.
- **MCP/tools.** Used Lumen semantic search earlier for code discovery.

## Commands Executed

| command | result |
|---|---|
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features mcp_app_sibling_lookup_requires_exposed_ui_tool_on_same_upstream` | passed |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden` | passed |
| `cargo fmt --manifest-path crates/lab/Cargo.toml --all -- --check` | passed |
| `cargo check --manifest-path crates/lab/Cargo.toml --all-features` | passed |
| `bd close lab-zn7es --reason "Fixed Code Mode MCP App sibling callback routing; added regressions; cargo check --all-features passed"` | closed bead |

## Errors Encountered

- `cargo test -p lab ... --all-features` failed because the current workspace invocation cannot specify features for that package form. The test command was rerun successfully with `--manifest-path crates/lab/Cargo.toml --all-features`.
- Repo-root `cargo fmt --all` did not reformat the nested crate as expected, so formatting was rerun with `--manifest-path crates/lab/Cargo.toml`.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Code Mode MCP Apps | UI tools with `_meta.ui.resourceUri` could render, but sibling callbacks such as `youtube_probe` returned hidden-tool errors | Exposed sibling callbacks from the same MCP App upstream can reach the upstream proxy |
| Raw tool visibility | Ordinary upstream tools were hidden from `list_tools` in Code Mode | Still hidden from `list_tools` |
| Destructive callbacks | Callback bypass could not confirm destructive actions | Destructive sibling tools remain blocked and must use `execute` with confirmation |
| Vibin plugin | `repo-status` existed as a standalone local skill | `repo-status` now ships from the Vibin plugin bundle |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features mcp_app_sibling_lookup_requires_exposed_ui_tool_on_same_upstream` | sibling lookup only returns same-upstream UI-backed candidates | passed | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden` | sibling callback avoids hidden-tool error and reaches proxy routing | passed | pass |
| `cargo fmt --manifest-path crates/lab/Cargo.toml --all -- --check` | Rust formatting is clean | passed | pass |
| `cargo check --manifest-path crates/lab/Cargo.toml --all-features` | all-feature Lab binary compiles | passed | pass |

## Risks and Rollback

The callback allowlist depends on an upstream exposing at least one MCP App UI tool and the requested sibling tool being exposed by the same upstream. Roll back by reverting the commit that changes `call_tool.rs`, `tools.rs`, and the related tests.

## Decisions Not Taken

- Did not globally expose all hidden raw tools in Code Mode; that remains behind `LAB_CODE_MODE_WIDGET_CALLBACKS`.
- Did not refactor duplicate `0.24.0` changelog history because it was unrelated release-note cleanup.

## References

- Downstream mitigation PR noted by the user: `https://github.com/jmagar/ytdl-mcp/pull/4`

## Next Steps

1. Commit and push the full worktree.
2. Build the latest release binary.
3. Sync the new binary to the local PATH target and the running dev container.
