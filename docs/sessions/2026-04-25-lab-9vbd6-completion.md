---
session: lab-9vbd.6-completion
bead: lab-9vbd.6
title: Cache sorted action names for completion handler
date: 2026-04-25
worker: codex
repo: /home/jmagar/workspace/lab
branch: bd-security/marketplace-p1-fixes
head: f168964b
plan: docs/superpowers/plans/2026-04-25-lab-9vbd6-completion.md
status: bead-scope-complete-with-unrelated-clippy-blocker
---

## User Request

Finish bead `lab-9vbd.6` in `/home/jmagar/workspace/lab`: cache sorted action names for the MCP completion handler, restore/implement current rmcp prompt/action completion support if absent, write a Superpowers plan, execute it, verify focused and all-features signals, and write this session report.

## Session Overview

Implemented a registry-owned sorted, deduplicated action-name cache and restored MCP prompt argument completion support. The completion handler now advertises `ServerCapabilities.completions`, handles `completion/complete`, and uses the registry cache for `run-action.action` completions.

Bead-specific tests pass. `cargo check --all-features` passes. `cargo clippy --all-features -- -D warnings` remains blocked by an unrelated marketplace lint in `crates/lab/src/dispatch/marketplace/stash_meta.rs:86` that is outside this worker's write scope.

## Sequence of Events

1. Gathered bead details with `bd show lab-9vbd.6`.
2. Reviewed `.omc/research/beads-next-round-definitive-report-2026-04-25.md`, which said `lab-9vbd.6` was incomplete because cached action-name completion infrastructure was absent.
3. Read `superpowers:writing-plans` instructions from `/home/jmagar/.codex/superpowers/skills/writing-plans/SKILL.md`.
4. Investigated current registry and MCP server code.
5. Found `ToolRegistry` only stored services before this change at `crates/lab/src/registry.rs:137`.
6. Found `LabMcpServer` had prompt support but no completion capability or `complete` override before this change.
7. Wrote plan `docs/superpowers/plans/2026-04-25-lab-9vbd6-completion.md`.
8. Added RED registry tests for cache ordering, deduplication, output equivalence, prefix filtering, and under-1ms empty-prefix completion.
9. Implemented registry cache storage and lookup at `crates/lab/src/registry.rs:139`, `crates/lab/src/registry.rs:194`, and `crates/lab/src/registry.rs:203`.
10. Added RED MCP completion tests for capability advertisement, action completion cache usage, prefix filtering, service completion, and unknown argument empty output.
11. Implemented MCP completion helper and handler at `crates/lab/src/mcp/server.rs:66`, `crates/lab/src/mcp/server.rs:143`, and `crates/lab/src/mcp/server.rs:176`.
12. Ran focused tests, all-features check, clippy, and all-features lib test.
13. Marked the plan checklist complete.
14. Wrote this report.

## Key Findings

- The original bead location `crates/lab/src/cli/serve.rs:611` is stale; the active MCP `ServerHandler` implementation now lives in `crates/lab/src/mcp/server.rs`.
- The completion handler had effectively been removed or never restored in the current server file; rmcp defaulted `complete` to an empty `CompleteResult`.
- The correct cache owner is `ToolRegistry`, because it is built at startup and rebuilt via `register()` when `lab serve --services` filters the registry.
- The registry cache must update only for accepted service registrations to preserve duplicate-service first-registration-wins semantics.

## Technical Decisions

- Store cached names as `Vec<&'static str>` in `ToolRegistry` at `crates/lab/src/registry.rs:139`; action names originate from static `ActionSpec` slices.
- Maintain sorted uniqueness incrementally in `ToolRegistry::register()` using `binary_search()` and `Vec::insert()` instead of rebuilding on each completion request.
- Expose `ToolRegistry::action_name_completions(prefix)` at `crates/lab/src/registry.rs:203`; it uses `partition_point()` to start the prefix scan from the sorted cache.
- Return all matches in `CompletionInfo` directly instead of using `CompletionInfo::new()`, because the bead explicitly requires empty-prefix completion to return all actions.
- Implement service argument completion without a separate service-name cache because the bead's performance issue is action-name collection/sort/dedup.

## Files Modified

- `crates/lab/src/registry.rs`: added the action-name cache and cache tests.
- `crates/lab/src/mcp/server.rs`: added completion capability, prompt completion handler, and completion tests.
- `docs/superpowers/plans/2026-04-25-lab-9vbd6-completion.md`: implementation plan, checklist marked complete.
- `docs/sessions/2026-04-25-lab-9vbd6-completion.md`: this session report.

## Commands Executed

Required context and metadata:

```bash
bd show lab-9vbd.6
awk '/lab-9vbd\.6/{flag=1} flag{print} flag && NR>1 && /^## / && !/lab-9vbd\.6/{exit}' .omc/research/beads-next-round-definitive-report-2026-04-25.md
sed -n '1,220p' /home/jmagar/.codex/superpowers/skills/writing-plans/SKILL.md
TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'
git remote get-url origin
git branch --show-current
git rev-parse --short HEAD
git log --oneline -5
git status --short
git log --oneline --name-only -10
pwd
git worktree list | grep $(pwd) | head -1
gh pr view --json number,title,url 2>/dev/null || echo "none"
```

Required metadata output:

```text
Date: 2026-04-25 16:46:29 EST
Origin: git@github.com:jmagar/lab.git
Branch: bd-security/marketplace-p1-fixes
HEAD: f168964b
PWD: /home/jmagar/workspace/lab
Worktree: /home/jmagar/workspace/lab f168964b [bd-security/marketplace-p1-fixes]
PR: {"number":29,"title":"fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation","url":"https://github.com/jmagar/lab/pull/29"}
```

Recent commits:

```text
f168964b fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized
39266dce refactor(lab-f1t2): address simplify + review findings on the f1t2 wave
b7f488af fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk
7b051062 fix(lab-zxx5.29): validate node install result shape
12eb0ea0 fix(lab-zxx5.28): typed error markers restore install taxonomy
```

Verification commands:

```bash
cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --lib registry::tests::action --all-features
cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --lib mcp::server::tests::completion --all-features
cargo check -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features
cargo clippy -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features -- -D warnings
cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features --lib registry::tests::action
cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features --lib mcp::server::tests::completion
cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features --lib
cargo test -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features --lib oauth::local_relay::tests::oauth_local_relay_returns_bad_gateway_for_unreachable_target
```

## Errors Encountered

- Initial `cargo test -p lab ...` failed because `lab` is ambiguous with crates.io `lab@0.11.0`. Subsequent commands used package ID `path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0`.
- During early RED/GREEN runs, other-worker marketplace edits temporarily caused missing module and parser errors under `crates/lab/src/dispatch/marketplace.rs`; those resolved later without changes from this worker.
- `cargo clippy --all-features -- -D warnings` fails on unrelated marketplace code at `crates/lab/src/dispatch/marketplace/stash_meta.rs:86` with `clippy::derivable_impls` for a manual `Default` implementation.
- Full `cargo test --all-features --lib` had one unrelated transient failure in `oauth::local_relay::tests::oauth_local_relay_returns_bad_gateway_for_unreachable_target` due to `AddrInUse` on `127.0.0.1:35691`; rerunning that exact test passed.

## Behavior Changes

- Registry construction now caches sorted unique action names as services are registered at `crates/lab/src/registry.rs:139`.
- Action completion no longer collects, sorts, and deduplicates every service action on each request; it uses `ToolRegistry::action_name_completions()` at `crates/lab/src/registry.rs:203`.
- MCP server now advertises completion support at `crates/lab/src/mcp/server.rs:143`.
- MCP server now handles `completion/complete` at `crates/lab/src/mcp/server.rs:176`.
- `run-action.action` prompt completion uses the cached registry action-name list at `crates/lab/src/mcp/server.rs:73`.
- `run-action.service` and `service-discover.service` complete service names at `crates/lab/src/mcp/server.rs:75`.

## Verification Evidence

Focused registry cache tests:

```text
running 4 tests
test registry::tests::action_names_cache_is_sorted_and_deduplicated_at_registration_time ... ok
test registry::tests::action_name_completions_filter_by_prefix_from_cached_names ... ok
test registry::tests::action_name_completions_empty_prefix_returns_all_actions_under_one_ms ... ok
test registry::tests::action_name_completions_match_legacy_collect_sort_dedup_output ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 843 filtered out; finished in 0.00s
```

Focused MCP completion tests:

```text
running 4 tests
test mcp::server::tests::completion_run_action_empty_action_prefix_uses_cached_action_names ... ok
test mcp::server::tests::completion_run_action_action_prefix_filters_cached_action_names ... ok
test mcp::server::tests::completion_prompt_service_arguments_filter_service_names ... ok
test mcp::server::tests::completion_unknown_prompt_argument_returns_empty_result ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 843 filtered out; finished in 0.00s
```

All-features check:

```text
cargo check -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features
Finished `dev` profile [unoptimized + debuginfo] target(s) in 30.99s
EXIT:0
```

All-features clippy:

```text
cargo clippy -p 'path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0' --all-features -- -D warnings
error: this `impl` can be derived
  --> crates/lab/src/dispatch/marketplace/stash_meta.rs:86:1
EXIT:101
```

All-features lib test:

```text
running 847 tests
846 passed; 1 failed
failure: oauth::local_relay::tests::oauth_local_relay_returns_bad_gateway_for_unreachable_target
reason: AddrInUse on 127.0.0.1:35691
```

Exact failed-test rerun:

```text
running 1 test
test oauth::local_relay::tests::oauth_local_relay_returns_bad_gateway_for_unreachable_target ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 846 filtered out; finished in 0.03s
```

Required bead criteria mapping:

- Action names cached at registry build/registration time: implemented at `crates/lab/src/registry.rs:139` and tested at `crates/lab/src/registry.rs:798`.
- Completion handler uses cached list: implemented at `crates/lab/src/mcp/server.rs:73` and tested at `crates/lab/src/mcp/server.rs:2234`.
- Output equivalence before/after cache: tested at `crates/lab/src/registry.rs:824`.
- Empty-prefix completion returns all cached actions in under 1ms: tested at `crates/lab/src/registry.rs:860`.

## Risks and Rollback

- Risk: `CompletionInfo` returns more than 100 values for empty-prefix action completion. This is intentional for this bead because the validation criteria require all actions for empty prefix.
- Risk: Service-name completion remains uncached. This is not the bead's hot path and only scans registered services.
- Rollback: revert changes in `crates/lab/src/registry.rs`, `crates/lab/src/mcp/server.rs`, `docs/superpowers/plans/2026-04-25-lab-9vbd6-completion.md`, and this report.

## Decisions Not Taken

- Did not implement a marketplace artifact action or change marketplace dispatch files. User explicitly limited this worker's scope away from marketplace artifact action implementation.
- Did not add a `OnceLock`; registry build-time caching was simpler and preserves filtered registry behavior through `ToolRegistry::register()`.
- Did not paginate completion output because the bead explicitly requires empty-prefix completion to return all actions.

## References

- Bead: `lab-9vbd.6`
- Research report: `.omc/research/beads-next-round-definitive-report-2026-04-25.md`
- Plan: `docs/superpowers/plans/2026-04-25-lab-9vbd6-completion.md`
- rmcp 1.4 local source: `CompleteRequestParams`, `CompleteResult`, and `ServerHandler::complete` patterns in local Cargo registry.

## Open Questions

- Transcript/session source was not exposed in this environment.
- Whether the unrelated marketplace clippy failure at `crates/lab/src/dispatch/marketplace/stash_meta.rs:86` should be fixed by the marketplace worker before closing this bead under a strict all-green gate.

## Next Steps

1. Have the marketplace worker fix or finish `crates/lab/src/dispatch/marketplace/stash_meta.rs:86` so `cargo clippy --all-features -- -D warnings` can pass.
2. Close `lab-9vbd.6` after deciding whether bead-specific green tests plus unrelated clippy blocker are sufficient for closure.
