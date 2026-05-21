---
bead: lab-9vbd.7
title: Add extract_error_info downcast integrity test
date: 2026-04-25
worker: Codex
repo: /home/jmagar/workspace/lab
branch: bd-security/marketplace-p1-fixes
head: f168964b
status: complete
plan: docs/superpowers/plans/2026-04-25-lab-9vbd7-completion.md
---

## User Request

Complete bead `lab-9vbd.7` in `/home/jmagar/workspace/lab` by adding focused MCP error extraction tests for `extract_error_info`, following the required bead workflow, verifying with `cargo test -- extract_error` plus focused MCP tests, and writing this session report.

## Session Overview

Implemented two focused tests in `crates/lab/src/mcp/server.rs:1909` and `crates/lab/src/mcp/server.rs:1931`.

The first test calls the real always-on `lab_admin` dispatcher with an unknown action, converts the resulting `ToolError` through `DispatchError`, wraps it in `anyhow::Error`, and verifies `extract_error_info` preserves `unknown_action` plus valid-action extras.

The second test constructs an `anyhow::Error` from serialized JSON and verifies the fallback parser preserves `unknown_action`, message, `valid`, and `hint` extras.

Active plan path: `docs/superpowers/plans/2026-04-25-lab-9vbd7-completion.md`.

## Sequence of Events

1. Ran `bd show lab-9vbd.7` and confirmed the bead was `IN_PROGRESS` with validation criteria requiring downcast and JSON fallback tests.
2. Reviewed `.omc/research/beads-next-round-definitive-report-2026-04-25.md` for `lab-9vbd.7`; it stated the bead was incomplete and required tests named so `cargo test -- extract_error` selects them.
3. Investigated `crates/lab/src/mcp/server.rs:1859`, `crates/lab/src/mcp/error.rs:145`, and dispatchers capable of producing structured `unknown_action` errors.
4. Selected `crate::dispatch::lab_admin::dispatch` because it is always-on and produces `ToolError::UnknownAction` with valid action extras without service credentials.
5. Created and completed `docs/superpowers/plans/2026-04-25-lab-9vbd7-completion.md` using the required writing-plans skill.
6. Added tests in `crates/lab/src/mcp/server.rs:1909` and `crates/lab/src/mcp/server.rs:1931`.
7. Ran required verification commands. `cargo test -- extract_error` passed. The initial focused MCP command using `-p lab` failed due package-name ambiguity, then the path-qualified command passed.
8. Wrote this report.

## Key Findings

- `extract_error_info` prioritizes `DispatchError` downcasting before serialized JSON fallback at `crates/lab/src/mcp/server.rs:1859`.
- MCP dispatch converts `ToolError` into `DispatchError` before `anyhow::Error` at `crates/lab/src/mcp/server.rs:1245`.
- `DispatchError::from(ToolError)` preserves `unknown_action`, `valid`, and `hint` fields at `crates/lab/src/mcp/error.rs:145`.
- Existing tests covered `canonical_kind` but did not call `extract_error_info` directly before this change.

## Technical Decisions

- Used `lab_admin` dispatch rather than a feature-gated service to keep the downcast integrity test deterministic and credential-free.
- Converted the real dispatch `ToolError` into `DispatchError` explicitly to exercise the same structured error path used by MCP dispatch.
- Used serialized JSON for the fallback test to cover the legacy/stringified error path without relying on service-specific runtime setup.
- Checked `param` and `hint` null placeholders because `extract_error_info` currently emits an object containing those keys whenever any structured extra exists.

## Files Modified

- `crates/lab/src/mcp/server.rs:1901` imports `extract_error_info` and `DispatchError` into the test module.
- `crates/lab/src/mcp/server.rs:1909` adds `extract_error_info_preserves_unknown_action_from_real_dispatch_downcast`.
- `crates/lab/src/mcp/server.rs:1931` adds `extract_error_info_preserves_unknown_action_from_json_fallback`.
- `docs/superpowers/plans/2026-04-25-lab-9vbd7-completion.md:1` records the implementation plan and completed checklist.
- `docs/sessions/2026-04-25-lab-9vbd7-completion.md:1` records this session report.

## Commands Executed

```bash
bd show lab-9vbd.7
```

Result: bead `lab-9vbd.7` was `IN_PROGRESS`; validation required a real dispatch unknown-action test and a JSON fallback test; testing step was `cargo test -- extract_error`.

```bash
sed -n '349,372p' .omc/research/beads-next-round-definitive-report-2026-04-25.md
```

Result: report marked `lab-9vbd.7` incomplete and listed all required test criteria.

```bash
sed -n '1080,1170p;1688,1895p;1900,1985p' crates/lab/src/mcp/server.rs
sed -n '1,260p' crates/lab/src/mcp/error.rs
sed -n '1,120p' crates/lab/src/dispatch/lab_admin/dispatch.rs
sed -n '1,140p' crates/lab/src/dispatch/doctor/dispatch.rs
sed -n '1,120p' crates/lab/src/dispatch/fs/dispatch.rs
sed -n '1,120p' crates/lab/src/dispatch/error.rs
```

Result: confirmed `extract_error_info`, `DispatchError`, `ToolError`, and candidate real dispatch paths.

```bash
cargo test -- extract_error
```

Result: passed. Evidence included both new tests passing in `src/lib.rs` and `src/main.rs`: 2 passed, 0 failed in each target; filtered integration/doc tests reported 0 matching tests and passed.

```bash
cargo test -p lab mcp::server::tests
```

Result: failed before tests ran due ambiguous package specification: local `path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0` and crates.io `lab@0.11.0` both matched `lab`.

```bash
cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' mcp::server::tests
```

Result: passed. Evidence: 10 passed, 0 failed in `src/lib.rs`; 10 passed, 0 failed in `src/main.rs`; integration targets had 0 matching tests and passed.

### Required Metadata Commands

```bash
TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'
```

Output: `2026-04-25 15:31:14 EST`

```bash
git remote get-url origin
```

Output: `git@github.com:jmagar/lab.git`

```bash
git branch --show-current
```

Output: `bd-security/marketplace-p1-fixes`

```bash
git rev-parse --short HEAD
```

Output: `f168964b`

```bash
git log --oneline -5
```

Output:

```text
f168964b fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized
39266dce refactor(lab-f1t2): address simplify + review findings on the f1t2 wave
b7f488af fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk
7b051062 fix(lab-zxx5.29): validate node install result shape
12eb0ea0 fix(lab-zxx5.28): typed error markers restore install taxonomy
```

```bash
git status --short
```

Output summary: the working tree was already broadly dirty with many unrelated modified/deleted/untracked files across `apps/gateway-admin`, `crates/lab-apis`, `crates/lab`, and `docs`. Relevant entry for this bead: `M crates/lab/src/mcp/server.rs`. The new plan/report paths did not appear in the captured short status output, likely due ignore rules or status timing.

```bash
git log --oneline --name-only -10
```

Output summary:

```text
f168964b fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized
crates/lab/src/api/nodes/fleet.rs
crates/lab/src/dispatch/marketplace/acp_dispatch.rs
crates/lab/src/dispatch/marketplace/dispatch.rs
crates/lab/src/dispatch/node/send.rs
crates/lab/src/node/install.rs
39266dce refactor(lab-f1t2): address simplify + review findings on the f1t2 wave
apps/gateway-admin/components/chat/chat-input.tsx
crates/lab/src/api/services/fs.rs
crates/lab/src/dispatch/fs/dispatch.rs
crates/lab/src/mcp/CLAUDE.md
crates/lab/src/mcp/services/fs.rs
b7f488af fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk
crates/lab/src/dispatch/marketplace/acp_dispatch.rs
7b051062 fix(lab-zxx5.29): validate node install result shape
crates/lab/src/dispatch/marketplace/acp_dispatch.rs
crates/lab/src/dispatch/marketplace/dispatch.rs
12eb0ea0 fix(lab-zxx5.28): typed error markers restore install taxonomy
crates/lab/src/node/install.rs
crates/lab/src/node/ws_client.rs
ae302ef6 docs(lab-f1t2.32): document MCP transport auth requirement for fs
crates/lab/src/mcp/CLAUDE.md
crates/lab/src/registry.rs
86e943eb fix(lab-f1t2.26): redact path from deny-list oracle log events
crates/lab/src/api/services/fs.rs
crates/lab/src/dispatch/fs/dispatch.rs
c9be4573 fix(lab-f1t2.30): reset AttachmentChip thumbUrl at effect start
apps/gateway-admin/components/chat/chat-input.tsx
33db1293 fix(lab-f1t2.29): reset loading/truncated when picker closes mid-fetch
apps/gateway-admin/components/chat/workspace-picker.tsx
0e7a569f fix(lab-f1t2.24): handle help/schema before workspace_root resolution
crates/lab/src/dispatch/fs/dispatch.rs
```

```bash
pwd
```

Output: `/home/jmagar/workspace/lab`

```bash
git worktree list | grep $(pwd) | head -1
```

Output: `/home/jmagar/workspace/lab                                   f168964b [bd-security/marketplace-p1-fixes]`

```bash
gh pr view --json number,title,url 2>/dev/null || echo "none"
```

Output:

```json
{"number":29,"title":"fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation","url":"https://github.com/jmagar/lab/pull/29"}
```

## Errors Encountered

- `cargo test -p lab mcp::server::tests` failed before executing tests because Cargo found both the local `lab` package and crates.io `lab@0.11.0`. The command was rerun successfully as `cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' mcp::server::tests`.

## Behavior Changes

- No runtime behavior changed.
- Test coverage now verifies `extract_error_info` preserves structured `unknown_action` metadata through the `DispatchError` downcast path and serialized JSON fallback path.

## Verification Evidence

- `cargo test -- extract_error`: passed; both new tests passed in `src/lib.rs` and `src/main.rs`.
- `cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' mcp::server::tests`: passed; 10 MCP server tests passed in `src/lib.rs` and 10 passed in `src/main.rs`.
- Bead validation criteria are covered:
  - Real dispatch unknown action: `crates/lab/src/mcp/server.rs:1909`.
  - `DispatchError` downcast path: `crates/lab/src/mcp/server.rs:1914`.
  - Direct `extract_error_info` call and `kind == "unknown_action"`: `crates/lab/src/mcp/server.rs:1917`.
  - Valid-action extras preserved: `crates/lab/src/mcp/server.rs:1926`.
  - JSON fallback path: `crates/lab/src/mcp/server.rs:1931`.
  - JSON fallback message and extras preserved: `crates/lab/src/mcp/server.rs:1944`.

## Risks and Rollback

- Risk: the downcast test depends on `lab_admin` action ordering keeping `help` first. If `ACTIONS` ordering changes, the assertion `extra["valid"][0] == "help"` may need to become a membership assertion.
- Risk: the working tree has extensive unrelated changes from other agents; do not revert or stage unrelated files while integrating this bead.
- Rollback: remove the two tests and import additions from `crates/lab/src/mcp/server.rs:1901`, `crates/lab/src/mcp/server.rs:1909`, and `crates/lab/src/mcp/server.rs:1931`; remove this plan/report if desired.

## Decisions Not Taken

- Did not edit registry, completion, or marketplace code per write-scope instruction.
- Did not use feature-gated service dispatchers because they could add config or feature assumptions unrelated to the bead.
- Did not close the bead from the CLI; this report states closeability after verification.

## References

- Bead details from `bd show lab-9vbd.7`.
- Research section: `.omc/research/beads-next-round-definitive-report-2026-04-25.md:349`.
- Extraction function: `crates/lab/src/mcp/server.rs:1859`.
- MCP dispatch conversion to `DispatchError`: `crates/lab/src/mcp/server.rs:1245`.
- Structured error conversion: `crates/lab/src/mcp/error.rs:145`.
- Real dispatch source: `crates/lab/src/dispatch/lab_admin/dispatch.rs:63`.

## Open Questions

- Transcript/session source was not exposed by the available tools, so no transcript path is included.
- The captured `git status --short` showed a broad dirty tree with unrelated files; ownership of those changes is outside this bead.

## Next Steps

- Bead `lab-9vbd.7` is closeable based on passing focused verification and satisfied validation criteria.
