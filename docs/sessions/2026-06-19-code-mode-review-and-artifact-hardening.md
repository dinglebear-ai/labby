---
date: 2026-06-19 02:47:18 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: fe5512e1
session id: 42a5241a-9475-41a0-b9ce-b09a398a1c2b
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/42a5241a-9475-41a0-b9ce-b09a398a1c2b.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab fe5512e1 [main]
---

# Code Mode review and artifact hardening

## User Request

Review the newly shipped Code Mode implementation, continue from the existing review, list issues, trace whether the legacy search path was unused, remove it if appropriate, test Code Mode through the Labby CLI, harden the `writeArtifact` `contentType` sharp edge, commit and push the changes, and save the session notes.

## Session Overview

This session removed the stale broker-side Code Mode search helper, exercised Code Mode through `target/debug/labby gateway code exec`, found that artifact `contentType` accepted header-unsafe values, hardened the host and sandbox boundaries, and pushed the resulting commits to `main`.

The latest `.claude` transcript path was inspected because the save-to-md skill requires it. Its visible tail was from an older Claude CLI flow rather than this Codex app continuation, so the Code Mode facts in this note are based on the current conversation context plus live git/test evidence.

## Sequence of Events

1. Resumed the comprehensive Code Mode review and clarified that the flagged `CodeModeBroker::search` / `search_allowed` path was a separate legacy JS runner path, not the catalog-only Code Mode discovery path.
2. Traced the search code, determined it was stale, removed the legacy search helper, and pushed commit `21a528c7 Remove stale Code Mode search helper`.
3. Exercised Code Mode through the Labby CLI, covering basic sandbox execution, catalog discovery, artifact writes, typed and raw upstream dispatch, snippets, truncation, and console capture behavior.
4. Found that `writeArtifact(..., { contentType: "text/html\nnope" })` was accepted and echoed in artifact receipts.
5. Dispatched subagent `019ede5d-0a0a-7642-b706-728d259783e9` to investigate and plan hardening, revised the plan, then had the agent execute it.
6. Tightened artifact `contentType` validation, added tests/docs/schema updates, added the ASCII-space-only trim guard, rejected non-string `options.contentType` in the JS wrapper, verified with Rust tests and live CLI smoke tests, and pushed commit `fe5512e1 Harden Code Mode artifact content types`.

## Key Findings

- `CodeModeBroker::search` and `search_allowed` were not needed for the shipped Code Mode path and were removed in `21a528c7`.
- Artifact receipts carry `content_type` back into model-visible output, so it needed stronger validation than a length cap.
- The host validator now normalizes and validates `content_type` before writing the artifact at `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs:390`.
- The validator rejects ASCII controls, embedded whitespace, non-ASCII, malformed `type/subtype` strings, multiple slashes, and over-256-byte values at `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs:451`.
- The JS wrapper rejects non-string `options.contentType` before host emission at `crates/lab/src/dispatch/gateway/code_mode/runner.rs:436`.

## Technical Decisions

- Used a conservative ASCII `type/subtype` grammar instead of full MIME parsing because the value is receipt metadata, not a transport header.
- Trimmed only surrounding ASCII spaces with `trim_matches(' ')`; tabs/newlines are rejected as control or whitespace, and NBSP/Unicode whitespace is rejected as non-ASCII.
- Treated missing or blank `contentType` as `text/plain` to preserve existing convenience behavior.
- Rejected MIME parameters such as `text/plain; charset=utf-8` for now to keep the model-visible receipt field simple and header-safe.
- Updated the MCP structured output schema so the documented artifact receipt contract matches runtime validation.

## Files Changed

| status | path | previous path | purpose | evidence |
| --- | --- | --- | --- | --- |
| modified | `crates/lab/src/dispatch/gateway/code_mode/protocol.rs` | - | Removed legacy search protocol surface. | Commit `21a528c7` file list. |
| deleted | `crates/lab/src/dispatch/gateway/code_mode/search.rs` | - | Removed stale broker-side search runner path. | Commit `21a528c7` file list. |
| modified | `crates/lab/src/dispatch/gateway/code_mode/tests_broker.rs` | - | Updated tests after legacy search removal. | Commit `21a528c7` file list. |
| modified | `crates/lab/src/dispatch/gateway/code_mode/tests_normalize.rs` | - | Updated normalization tests after search removal. | Commit `21a528c7` file list. |
| modified | `crates/lab/src/dispatch/gateway/code_mode/types.rs` | - | Removed search-related types. | Commit `21a528c7` file list. |
| modified | `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs` | - | Added `content_type` normalization and validation before artifact writes. | `artifacts.rs:390`, `artifacts.rs:451`. |
| modified | `crates/lab/src/dispatch/gateway/code_mode/runner.rs` | - | Added JS wrapper rejection for non-string `options.contentType`. | `runner.rs:436`. |
| modified | `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs` | - | Added valid, invalid, defaulting, NBSP, and wrapper regression coverage. | Commit `fe5512e1` stat: 93 touched lines. |
| modified | `crates/lab/src/mcp/handlers_tools.rs` | - | Added structured artifact output schema with content type pattern and SHA constraints. | `handlers_tools.rs:289`. |
| modified | `docs/dev/CODE_MODE.md` | - | Documented artifact `contentType` default and validation rules. | `CODE_MODE.md:74`. |
| modified | `docs/snippets/README.md` | - | Updated artifact cap and `contentType` docs. | `README.md:290`. |
| created | `docs/sessions/2026-06-19-code-mode-review-and-artifact-hardening.md` | - | Saved this session log. | Current save-to-md run. |

## Beads Activity

No bead activity was observed for this specific Code Mode review/hardening session. Maintenance reads were run with `bd list --all --sort updated --reverse --limit 100 --json` and `tail -200 .beads/interactions.jsonl`; the visible recent interactions were older tracker events, with the latest visible interaction closing `lab-jr390` on 2026-06-17.

## Repository Maintenance

### Plans

`find docs/plans -maxdepth 2 -type f` showed `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/complete/mcp-streamable-http-oauth-proxy.md`. The fleet WebSocket plan is still marked open in its header, and the streamable HTTP/OAuth plan is already under `complete/`; no plan files were moved.

### Beads

Beads were read but not changed. No current-session bead was identified from the observed tracker data, and no follow-up bead was created because the implemented work was completed and verified in commits `21a528c7` and `fe5512e1`.

### Worktrees and branches

`git worktree list --porcelain` showed four worktrees: the main worktree, `marketplace-no-mcp`, `codex/cloudflare-codemode-parity`, and `claude/agitated-rubin-328e01`. Status checks for the three extra worktrees were clean, but they map to named branches with unclear ownership or intentionally long-lived context, so no worktrees or branches were deleted.

### Stale docs

The stale-doc pass was scoped to the implementation touched by this session. `docs/dev/CODE_MODE.md` and `docs/snippets/README.md` were updated to match the new artifact content type behavior; broader docs cleanup was not attempted.

### Transparency

No destructive cleanup was performed. The current worktree was clean before this session note was written, `main` was even with `origin/main` at `fe5512e1`, and the session artifact commit is path-limited by the save-to-md workflow.

## Tools and Skills Used

- **Skill.** `vibin:save-to-md` was used for this session artifact and the required path-limited commit/push workflow.
- **Shell commands.** Used `git`, `cargo`, `target/debug/labby`, `bd`, `gh`, `find`, `sed`, `nl`, `tail`, `wc`, and `ls` for implementation, verification, and maintenance evidence.
- **File tools.** Used patch-based edits for Rust and docs changes and for this markdown artifact.
- **Subagents.** Used subagent `019ede5d-0a0a-7642-b706-728d259783e9` to investigate the artifact `contentType` issue, propose a plan, and execute the reviewed plan.
- **External CLI.** Used `target/debug/labby gateway code exec` for live Code Mode smoke tests.
- **MCP/tooling issues.** Live Code Mode runs emitted warnings for disconnected or expired OAuth upstreams such as Google Drive, Gmail, Calendar, People, and Globalping; those upstream warnings did not block the local Code Mode artifact tests.

## Commands Executed

| command | result |
| --- | --- |
| `git branch --show-current` | Confirmed branch `main`. |
| `git status --short --branch` | Confirmed `main...origin/main` clean before this session note. |
| `git log --oneline -5` | Confirmed recent commits include `fe5512e1` and `21a528c7`. |
| `cargo fmt --all --check` | Passed after hardening changes. |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features write_code_mode_artifact_accepts_and_trims_common_content_types -- --nocapture` | Passed. |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features write_code_mode_artifact_rejects_invalid_content_types_without_writing -- --nocapture` | Passed. |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features code_mode_runner_wrapper_exposes_write_artifact -- --nocapture` | Passed. |
| `cargo check --manifest-path crates/lab/Cargo.toml --all-features` | Passed. |
| `cargo build --manifest-path crates/lab/Cargo.toml --all-features` | Passed. |
| `target/debug/labby gateway code exec --json --code 'async () => { ... contentType: "\\u00a0text/plain\\u00a0" ... }'` | Rejected NBSP-wrapped content type with `invalid_param`. |
| `target/debug/labby gateway code exec --json --code 'async () => { ... contentType: " application/vnd.lab+json " ... }'` | Accepted and normalized to `application/vnd.lab+json`. |
| `target/debug/labby gateway code exec --json --code 'async () => { ... contentType: 123 ... }'` | Rejected with `writeArtifact options.contentType must be a string`; no artifact call emitted. |
| `git commit -m "Harden Code Mode artifact content types"` | Created commit `fe5512e1`. |
| `git push origin main` | Pushed `main` to `origin/main`. |

## Errors Encountered

- A live ASCII-trim smoke test initially passed an object as `writeArtifact` content, but the helper requires string content. The smoke was rerun with `JSON.stringify(...)` and passed.
- During verification, parallel cargo commands waited on package/artifact locks. Polling through completion showed the commands passed.
- Live Code Mode catalog cache logs warned about missing or expired OAuth credentials for several upstreams. The tested artifact helper path did not require those upstreams.

## Behavior Changes (Before/After)

| area | before | after |
| --- | --- | --- |
| Legacy Code Mode search | `CodeModeBroker::search` / `search_allowed` remained as stale separate runner code. | Removed; Code Mode discovery uses the current catalog-oriented path. |
| Artifact `contentType` newline | `text/html\nnope` was accepted and echoed in receipts. | Rejected as `invalid_param` before artifact write. |
| Artifact `contentType` Unicode whitespace | Unicode-wrapped values risked being treated as trim-safe if generic trim was used. | Only ASCII spaces are trimmed; NBSP-wrapped values are rejected as non-ASCII. |
| Artifact `contentType` malformed syntax | Only length was capped. | Requires simple ASCII `type/subtype` with token characters and max 256 bytes. |
| JS wrapper bad `contentType` type | Non-string `options.contentType` silently defaulted to omitted. | Non-string `options.contentType` throws `writeArtifact options.contentType must be a string`. |

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `cargo fmt --all --check` | Formatting clean. | Passed. | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features write_code_mode_artifact_accepts_and_trims_common_content_types -- --nocapture` | Valid common content types accepted; ASCII-space wrapping normalized. | Passed. | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features write_code_mode_artifact_rejects_invalid_content_types_without_writing -- --nocapture` | Invalid content types rejected without writing files. | Passed. | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features code_mode_runner_wrapper_exposes_write_artifact -- --nocapture` | Wrapper includes artifact helper and `contentType` type guard. | Passed. | pass |
| `cargo check --manifest-path crates/lab/Cargo.toml --all-features` | All-features check succeeds. | Passed. | pass |
| `cargo build --manifest-path crates/lab/Cargo.toml --all-features` | Current `target/debug/labby` builds. | Passed. | pass |
| Live CLI NBSP content type smoke | Reject `\u00a0text/plain\u00a0`. | Rejected with `invalid_param`. | pass |
| Live CLI ASCII trim smoke | Accept and normalize ` application/vnd.lab+json `. | Returned receipt content type `application/vnd.lab+json`. | pass |
| Live CLI non-string content type smoke | Reject `contentType: 123` before artifact call. | Returned message `writeArtifact options.contentType must be a string` with `calls: []`. | pass |
| `git diff --check` | No whitespace errors. | Passed. | pass |

## Risks and Rollback

The content type validator is intentionally stricter than full MIME metadata. Snippets that previously used parameters such as `text/plain; charset=utf-8` will now fail and should use `text/plain` instead. Rollback is `git revert fe5512e1` for the artifact hardening, or `git revert 21a528c7` for the legacy search removal, followed by the same all-features build/test path.

## Decisions Not Taken

- Did not implement full RFC-style MIME parsing because the value is a model-visible receipt field and a narrow `type/subtype` contract is easier to reason about.
- Did not delete any extra worktrees or branches because the observed clean worktrees are tied to named branches with plausible ongoing ownership.
- Did not move `docs/plans/fleet-ws-plan-lab-n07n.md` because it is marked open and was not part of this completed Code Mode work.
- Did not create a bead retroactively; no directly relevant current-session bead activity was observed, and the work was already complete and verified.

## References

- `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs:390`
- `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs:451`
- `crates/lab/src/dispatch/gateway/code_mode/runner.rs:436`
- `crates/lab/src/mcp/handlers_tools.rs:289`
- `docs/dev/CODE_MODE.md:74`
- `docs/snippets/README.md:290`
- Commit `21a528c7 Remove stale Code Mode search helper`
- Commit `fe5512e1 Harden Code Mode artifact content types`

## Open Questions

- The inspected `.claude` transcript file did not visibly correspond to this Codex app continuation, so exact earlier conversational details beyond the current context summary were not recoverable from that transcript.
- The `claude/agitated-rubin-328e01` worktree points at `fe5512e1` and tracks `origin/main`, but ownership was not established; it was left intact.

## Next Steps

The implementation work for this session is complete and pushed. Recommended immediate next command is `git status --short --branch` after the session-note commit to confirm the repository remains clean and synced.
