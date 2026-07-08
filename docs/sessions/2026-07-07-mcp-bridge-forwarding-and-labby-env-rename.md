```yaml
date: 2026-07-07 22:54:00 EST
repo: git@github.com:jmagar/labby.git
branch: main
head: 9b99f08e
working directory: /home/jmagar/workspace/lab
beads: lab-jtp96
```

## User Request

Continue verifying/deploying MCP bridge forwarding work from a prior session, then (per explicit follow-up asks): add automated test coverage for the bridge, fix pre-existing workspace clippy failures, configure a missing HMAC secret on the live container, and perform a full hard-break rename of every `LAB_*` environment variable to `LABBY_*` across the repo and the live deployment. Later, extend that work into a real `.env` → `config.toml` migration for previously env-only preferences, including the `labby-auth` rate-limit fields. Finish with a cleanup pass and a saved session log.

## Session Overview

Verified and closed out live deployment of MCP bridge protocol forwarding (`ping`, SEP-1319 task management, custom requests) added in a prior session, then added in-memory end-to-end test coverage for it — which surfaced and fixed two real production bugs (a test-harness deadlock and a genuine `rmcp` `ServerResult` wire-ambiguity bug in `cancel_task`/`get_task_result` forwarding). Fixed 13 pre-existing clippy findings. Performed a full `LAB_*` → `LABBY_*` environment-variable rename across the entire workspace (4 Rust crates, docs, scripts, CI, `.env.example`, docker-compose) as an explicit hard break with no aliasing, then migrated the live `labby` Incus container to match (binary rebuild, `.env` key rename, restart, verified healthy). Extended the work into a genuine `.env`-to-`config.toml` migration, adding real config.toml fields (not just docs) for 17 of ~19 previously env-only preferences across four crates, in four separately verified and committed batches. Merged in an unrelated externally-landed PR (#197, Code Mode pause-gate removal) that arrived mid-session and re-verified the merge before pushing. Closed with a repo cleanup pass and this session log.

## Sequence of Events

1. Verified the release build for the MCP bridge forwarding fix (`ping`/task-management/custom-request support) had completed, deployed the new binary to the `labby` Incus container, restarted the service, and confirmed clean startup logs, `/health` 200, and a working authenticated `/v1/gateway` call.
2. User asked "Any other suggestions?" — proposed three follow-ups: add automated bridge-forwarding tests, fix a pre-existing workspace-wide clippy failure, and configure `LABBY_CODEMODE_HMAC_SECRET` on the container. User approved all three plus a fourth: a full `LAB_*` → `LABBY_*` rename, explicitly authorizing use of parallel agents.
3. Dispatched three parallel `Agent` tool calls (clippy fixes, bridge test, env-var audit) plus a direct `Bash` action to configure the HMAC secret on the container.
4. Several dispatched agents exhibited a failure mode: recursively spawning further background agents instead of doing the assigned work, and (in the case of a `worktree`-isolated agent) reapplying a stale worktree diff onto the main checkout via raw file overwrite, silently reverting already-completed clippy fixes and the in-progress rename multiple times.
5. After detecting the corruption via direct `grep`/`cargo clippy`/`cargo nextest` verification (not agent self-reports), abandoned further agent delegation for the mechanical rename work and performed it directly via `perl -pi` regex substitution, crate by crate, re-verifying after each pass.
6. Diagnosed and fixed a genuine deadlock in the newly-added bridge test harness (`crates/labby/src/mcp/bridge.rs`): two `tokio::io::duplex` `.serve()` calls were awaited sequentially instead of concurrently via `tokio::join!`, so neither side's MCP `initialize` handshake could complete.
7. Diagnosed and fixed a genuine production bug surfaced by the same test: `GetTaskResult` and `CancelTaskResult` are wire-identical in `rmcp` 2.1.0's untagged `ServerResult` enum (`allOf[Result, Task]`, no discriminant), and `GetTaskResult` is declared first, so a real `cancel_task` response always deserializes as `GetTaskResult` — meaning the bridge's `cancel_task` forwarding was unconditionally broken against any real daemon. Fixed by accepting both variants in `BridgeServerHandler::cancel_task`. Similarly documented (already-correct) handling for `get_task_result`'s `GetTaskPayloadResult`/`CustomResult` ambiguity.
8. Completed the `LAB_*` → `LABBY_*` rename across `crates/labby`, `crates/labby-gateway`, `crates/labby-codemode`, `crates/labby-apis`, docs, scripts, CI workflows, `Justfile`, `README.md`, `.env.example`, and docker-compose files. Deliberately left `labby-auth`'s own `DEFAULT_ENV_PREFIX` constant unchanged (shared crate default reused by other rmcp-family repos), with this repo's own call site passing `"LABBY"` explicitly instead.
9. Migrated the live container: backed up `.env` and the running binary, renamed all `LAB_*` keys in the container's `.env` (including 10 per-upstream `LABBY_GW_*_AUTH_HEADER` entries), rebuilt and redeployed the binary, restarted, and verified clean startup (`env_prefix=LABBY`, OAuth fully resolved) plus a live authenticated `/v1/gateway` call.
10. User asked what else remained; reported three items: the actual `.env`→`config.toml` field migration (only documented, not implemented), a doc inconsistency (`REMOTE_LAB_TOKEN` vs `LABBY_UPSTREAM_TOKEN` example values), and cosmetic `LAB_`-labeled error strings in `labby-auth`.
11. User directed all three be done. Fixed the doc inconsistency and the `labby-auth` error-message prefixes (now built from the caller's actual `env_prefix` instead of hardcoded).
12. Implemented the `config.toml` migration in four separately verified batches: (a) `labby` crate — 7 vars (`show_all`, `dev_mode`, `protected_mcp_connect_timeout_secs`, `widget_callbacks`, `symbols`, `log.color`, `log.dir`); (b) `labby-gateway` — 4 vars (`upstream_discovery_concurrency`, `upstream_max_response_bytes`, `mcp_list_warm_timeout_ms`, `upstream_stderr_level`), also consolidating a pre-existing rule violation where three call sites had drifted into duplicated raw env reads; (c) `labby-codemode` — 5 vars (artifact retention/size, call budget), explicitly declining to wire the 3 warm-runner-pool sizing vars due to a genuine construction-order (chicken-and-egg) constraint; (d) `labby-auth`'s three OAuth rate-limit fields, threaded through `crates/labby/src/config.rs`'s existing merge pattern without touching the shared `labby-auth` crate itself.
13. On push after the final batch, hit a non-fast-forward rejection: PR #197 (from a separate, pre-existing worktree branch, unrelated to this session) had merged to `main` independently, deleting the entire Code Mode pause-gate subsystem (~6,500 lines). Merged it locally, re-verified `cargo check`/`clippy`/`nextest` from scratch (1742/1742 passing, down from 1802 due to the deleted test suites — not a regression), confirmed this session's own additions survived the merge intact, then pushed.
14. User flagged a confusing claim about `labby-auth` being "the repo" — clarified that `crates/labby-auth` is a workspace crate inside this same `jmagar/labby` monorepo, not a separate git repository; the earlier "shared across repos" comment referred to the crate's design intent (per the user's own global memory notes), not its git location.
15. User asked for a cleanup pass and `/save-to-md`. Verified the working tree was already clean and the stray `nostalgic-villani-597ea3` worktree/branch (now fully merged via PR #197) had already been removed. Ran the `save-to-md` skill, which included creating a follow-up bead for the deferred pool-sizing config.toml work.

## Key Findings

- `crates/labby/src/mcp/bridge.rs` test harness deadlock: both `.serve()` calls for the test-client↔bridge hop were awaited sequentially; an MCP server-side `.serve()` only resolves after completing the `initialize` handshake with a peer, which cannot happen until the peer side is also connecting concurrently. Fixed via `tokio::join!` (`bridge.rs:779-792`).
- `rmcp` 2.1.0 (`model/task.rs:139-232`): `GetTaskResult` and `CancelTaskResult` are structurally identical (`allOf[Result, Task]`), and `ServerResult`'s `#[serde(untagged)]` union lists `GetTaskResult` before `CancelTaskResult`, so a genuine `CancelTaskResult` response always deserializes as `GetTaskResult`. `GetTaskPayloadResult` has a deliberately-failing `Deserialize` impl for the same reason (rmcp's own doc comment acknowledges it's wire-identical to `CustomResult`).
- `crates/labby-gateway/CLAUDE.md` (upstream module doc) explicitly restricts `LAB_UPSTREAM_DISCOVERY_CONCURRENCY` env reads to `pool/helpers.rs`'s canonical function — three call sites (`gateway/runtime.rs`, `gateway/manager/code_mode_runtime.rs`, `gateway/manager/pool_lifecycle.rs`) had each independently duplicated the raw env read instead, per an in-code comment explaining the canonical function lived in a private module unreachable from those sites. Fixed by re-exporting it `pub(crate)`.
- `crates/labby-gateway/src/gateway/manager/core.rs:137`: `RunnerPool::from_env()` is called synchronously inside `GatewayManager::try_with_store()`, constructed with `GatewayConfig::default()` before any real `config.toml` is loaded (the manager only gets a real config on the first `reload()` call afterward) — a genuine architectural constraint on wiring `LABBY_CODE_MODE_POOL_*` into `config.toml` without restructuring startup order.
- `crates/labby/src/entrypoint.rs`: the real boot sequence is `config.toml` load → tracing init → `.env` load (not `.env` then `config.toml`, as an earlier doc claimed) — meaning `LABBY_LOG`/`LABBY_LOG_FORMAT`/`LABBY_LOG_COLOR`/`LABBY_LOG_DIR` set only in `~/.labby/.env` are silently ineffective at cold boot; `config.toml` is the only reliable file-based override for these two specifically.
- Several `Agent`-tool-dispatched subagents recursively spawned further background agents instead of executing their assigned task directly, and at least one `isolation: "worktree"` agent corrupted the main checkout by reapplying a stale worktree diff as a raw overwrite, silently reverting completed work (clippy fixes, partial rename) multiple times — only caught via direct `grep`/build/test verification, not agent self-reporting.

## Technical Decisions

- Fixed `cancel_task`'s wire ambiguity in production code (accept both `ServerResult::CancelTaskResult` and `ServerResult::GetTaskResult`) rather than only tolerating it in the test, since the underlying daemon response is genuinely ambiguous for any real caller, not just the test fixture.
- Left `labby-auth`'s `DEFAULT_ENV_PREFIX` constant (`"LAB"`) and its own tests unchanged rather than renaming, since the crate is documented (user's own global memory) as shared across other rmcp-family repos; this repo's `crates/labby/src/config.rs::resolve_auth()` already passes `"LABBY"` explicitly via `.env_prefix("LABBY")`, so this repo's actual runtime behavior is fully renamed without touching the shared crate's default.
- Used a process-wide resolved-value cache (`AtomicBool`/`Mutex`, mirroring the pre-existing `PROCESS_CODE_MODE_ENABLED` pattern) for `config.toml` preferences read from deep call sites without direct config access, rather than threading a config reference through every caller — consistent with an established pattern already in the codebase.
- Explicitly declined to wire `LABBY_CODE_MODE_POOL_SIZE`/`_RECYCLE_AFTER`/`_MAX_OVERFLOW` into `config.toml` rather than rushing a construction-order restructuring; documented the constraint and filed a follow-up bead (`lab-jtp96`) instead.
- Abandoned agent delegation for the mechanical rename mid-task after repeated corruption, switching to direct `perl -pi` regex substitution with `cargo check`/`clippy`/`nextest` verification after every change — judged more reliable than continuing to trust agent self-reports for this specific class of mechanical, easily-verified work.

## Files Changed

| status | path | purpose | evidence |
|---|---|---|---|
| modified | `crates/labby/src/mcp/bridge.rs` | Forward `ping`/task-management/custom-request through the MCP bridge; add end-to-end tests; fix deadlock and `cancel_task`/`get_task_result` wire-ambiguity bugs | commits `7611351f`, `6b4b11c1` |
| modified | ~13 files across `crates/labby-gateway` | Fix pre-existing clippy findings (`needless_raw_string_hashes`, `float_cmp`, `clippy::panic`, `range_plus_one`) | verified via `cargo clippy --workspace --all-features --all-targets -- -D warnings` clean |
| modified | ~111 files across `crates/labby*`, `docs/`, `scripts/`, `.github/`, `Justfile`, `README.md`, `.env.example`, `docker-compose*.yml`, `CLAUDE.md` files | `LAB_*` → `LABBY_*` env var rename (hard break, no aliasing); dead-var doc cleanup; load-order doc correction | commit `f3bb7855` |
| modified | `docs/services/GATEWAY.md`, `docs/services/UPSTREAM.md` | Canonicalize example env var name (`REMOTE_LAB_TOKEN` → `LABBY_UPSTREAM_TOKEN`) | commit `ae45de36` |
| modified | `crates/labby-auth/src/token.rs`, `crates/labby-auth/src/authorize.rs` | TTL-expiry error messages now build the field label from the caller's actual `env_prefix` instead of a hardcoded `LAB_` string | commit `ae45de36` |
| modified | `crates/labby/src/config.rs`, `crates/labby/src/entrypoint.rs`, `crates/labby/src/registry.rs`, `crates/labby/src/api/router.rs`, `crates/labby/src/api/state.rs`, `crates/labby/src/output/theme.rs`, `crates/labby/src/cli/serve.rs`, `crates/labby-runtime/src/gateway_config.rs`, `docs/runtime/CONFIG.md` | `config.toml` fields for 7 previously env-only `labby`-crate preferences | commit `496b65b8` |
| modified | `crates/labby-gateway/src/gateway/manager/{code_mode_runtime,pool_lifecycle}.rs`, `crates/labby-gateway/src/gateway/runtime.rs`, `crates/labby-gateway/src/upstream/pool.rs`, `crates/labby-gateway/src/upstream/pool/{discover,helpers,stdio_stderr}.rs`, `crates/labby-runtime/src/gateway_config.rs`, `docs/runtime/CONFIG.md` | `[gateway]` config.toml fields for 4 previously env-only vars; consolidated duplicated discovery-concurrency reads | commit `2a849f27` |
| modified | `crates/labby-codemode/src/{artifacts,config,lib}.rs`, `crates/labby-gateway/src/gateway/manager/pool_lifecycle.rs`, `crates/labby-runtime/src/gateway_config.rs`, `docs/runtime/CONFIG.md` | `[code_mode]` config.toml fields for 5 previously env-only artifact/call-budget vars | commit `4066c217` |
| modified | `crates/labby/src/config.rs` | `[auth]` rate-limit fields (`register_requests_per_minute`, `authorize_requests_per_minute`, `max_pending_oauth_states`) wired through `resolve_auth()` | commit `95aa4884` |
| merged | (~51 files, see PR #197) | External Code Mode pause-gate removal, unrelated to this session, merged to resolve a push conflict | commit `407d4992` |
| created | `docs/sessions/2026-07-07-mcp-bridge-forwarding-and-labby-env-rename.md` | This session log | this commit |

## Beads Activity

- **`lab-jtp96`** — "Wire LABBY_CODE_MODE_POOL_SIZE/_RECYCLE_AFTER/_MAX_OVERFLOW into config.toml" — created (P3, task) during the `save-to-md` maintenance pass to track the one deliberately-deferred item from the config.toml migration (the warm-runner pool sizing knobs, blocked on `GatewayManager` construction order). No other bead activity occurred during this session; no existing beads were found to directly cover the bridge-forwarding, clippy, or rename work, and none were claimed/closed.

## Repository Maintenance

- **Plans**: `docs/plans/fleet-ws-plan-lab-n07n.md` exists under `docs/plans/` (not `complete/`) but is unrelated to this session's work and was not touched or evaluated for completion status; left as-is.
- **Beads**: Ran `bd ready` (10 of 325 ready issues shown; none related to this session's work). Created `lab-jtp96` for the one known remaining gap. No other beads were relevant to claim, edit, or close.
- **Worktrees and branches**: `git worktree list` showed only `/home/jmagar/workspace/lab` (main) and `/home/jmagar/workspace/_no_mcp_worktrees/lab` (marketplace-no-mcp, a documented long-lived variant branch, left untouched) at cleanup time. The `.claude/worktrees/nostalgic-villani-597ea3` worktree and its local `claude/nostalgic-villani-597ea3` branch (verified via `git merge-base --is-ancestor origin/claude/nostalgic-villani-597ea3 main` → fully merged, matching PR #197) had already been removed by the time of the cleanup pass — likely automatic cleanup from the PR merge process; confirmed absent via `git worktree list` and `git branch -d` reporting "not found". Remote branches `origin/claude/bold-chaum-49f836` and `origin/claude/nostalgic-villani-597ea3` still exist on the remote; left untouched (unclear ownership of remote branch deletion, out of scope for a local cleanup pass).
- **Stale docs**: Corrected `docs/runtime/CONFIG.md`'s load-order description (was documented as `.env` before `config.toml`; actual order is `config.toml` → tracing init → `.env`) as part of the rename batch. Fixed the `REMOTE_LAB_TOKEN`/`LABBY_UPSTREAM_TOKEN` inconsistency across `docs/services/GATEWAY.md` and `docs/services/UPSTREAM.md`. No other stale docs were identified as directly contradicted by this session's changes.
- **Transparency**: The corrupted-then-recovered agent work (item 4-5 in Sequence of Events) is disclosed above and was reported to the user directly during the session, including an explicit acknowledgment that it cost significant extra time/token spend.

## Tools and Skills Used

- **Shell commands (`Bash`)**: primary tool throughout — `cargo check`/`clippy`/`nextest`, `git` (status, diff, log, merge, worktree, branch), `perl -pi` for the mechanical rename, `incus exec`/`incus file push` for container deployment, `curl` for live verification, `bd` for beads. No issues beyond the agent-corruption incident described below.
- **File tools (`Read`/`Edit`/`Write`)**: used for all source, doc, and config edits. No issues.
- **`Agent` tool (parallel subagent dispatch)**: used for the initial clippy-fix, bridge-test, and env-audit tasks, and again for the rename effort. Multiple dispatched agents recursively delegated to further background agents instead of doing the work, and at least one `isolation: "worktree"` agent corrupted the main checkout via a stale-diff raw overwrite, reverting completed work more than once. Abandoned further agent delegation for the mechanical rename after detecting this, completing it directly instead.
- **`ScheduleWakeup`**: used to resume the conversation after long-running background builds (`cargo build --release`) completed, avoiding active polling.
- **`Skill` tool**: `save-to-md` (this document).
- **No browser, MCP server, or external CLI tools were used** in this session beyond `incus`/`curl`/`git`/`cargo`/`bd` already listed.

## Commands Executed

| command | result |
|---|---|
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` (run repeatedly throughout) | Failing (13 findings) at session start → clean after fixes; re-broken twice by agent corruption → clean again after direct fixes; clean at every subsequent checkpoint |
| `cargo nextest run --workspace --all-features` (run repeatedly) | 830 → 836 (bridge tests) → 1800 (rename) → 1801 → 1802 (config batches) → 1742 (post-PR#197-merge, expected drop from deleted test suites) — all passing at each checkpoint |
| `incus file push .../labby ... && incus exec labby -- systemctl restart labby` (x2, mid-session and post-rename) | Both deploys succeeded; `journalctl` showed clean startup, `curl /health` returned 200 |
| `git merge origin/main -m "Merge origin/main (PR #197...)"` | Auto-merged cleanly (`ort` strategy), no conflict markers; re-verified with full check/clippy/nextest before pushing |
| `git push` (final) | Succeeded after the merge: `50f4dae2..407d4992 main -> main` |
| `bd create --title="Wire LABBY_CODE_MODE_POOL_SIZE..." ...` | Created `lab-jtp96` |

## Errors Encountered

- **Bridge test deadlock**: initial test harness for `crates/labby/src/mcp/bridge.rs` hung indefinitely because two `.serve()` calls for one hop were awaited sequentially instead of concurrently. Root cause: an MCP server-side `.serve()` only resolves after completing the `initialize` handshake, which requires the peer side to be connecting at the same time. Fixed with `tokio::join!`.
- **`cancel_task`/`get_task_result` forwarding bug**: production code matched only the "expected" `ServerResult` variant, but `rmcp`'s untagged enum resolves a real `CancelTaskResult` response to `GetTaskResult` instead (wire-identical structs, declaration-order tiebreak). The earlier "live verified" claim from the prior session never actually exercised `cancel_task`/`get_task_result`. Fixed by accepting the actual wire-level variant in both cases.
- **Agent-tool corruption**: repeated recursive self-delegation and at least one raw-overwrite revert from a `worktree`-isolated agent silently undid completed clippy fixes and partial rename work multiple times. Root cause not fully diagnosed (appears related to how a worktree-isolated agent's "reapply diff onto main checkout" step was implemented, likely copying full file content from a stale base rather than a true patch). Resolved by abandoning agent delegation for that specific mechanical task and doing it directly with `perl -pi`, verifying after every change.
- **Push rejected (non-fast-forward)**: an unrelated PR (#197) merged to `main` externally mid-session. Resolved via `git merge origin/main`, full re-verification, then push.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| MCP bridge `ping`/task-management/custom-request | Not forwarded at all (prior session's initial gap) | Forwarded; `cancel_task` and `get_task_result` genuinely work against a real daemon (previously silently broken even after the initial "fix") |
| Environment variable prefix | `LAB_*` (e.g. `LAB_MCP_HTTP_TOKEN`) | `LABBY_*` (e.g. `LABBY_MCP_HTTP_TOKEN`), hard break, no dual-read |
| `.env` vs `config.toml` for 17 preferences (log color/dir/symbols, dev mode, admin show-all, protected-MCP timeout, code-mode widget callbacks, gateway discovery concurrency/max-response-bytes/warm-timeout/stderr-level, code-mode artifact retention/size caps, call-budget caps, auth rate limits) | Env-var-only | `config.toml` field with env var as override, both functional |
| Live `labby` container | Running pre-rename binary with `LAB_*`-keyed `.env` | Running post-rename binary with `LABBY_*`-keyed `.env`, verified healthy |
| `LABBY_CODE_MODE_HMAC_SECRET` on live container | Unset (ephemeral key regenerated every restart) | Set persistently in `.env` |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo check --workspace --all-features` (final) | Clean | Clean | pass |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` (final) | Clean | Clean | pass |
| `cargo nextest run --workspace --all-features` (final, post-PR#197-merge) | All pass | 1742 passed, 14 skipped, 0 failed | pass |
| `curl -s -o /dev/null -w "%{http_code}" https://labby.tootie.tv/health` (post-rename deploy) | 200 | 200 | pass |
| Authenticated `POST /v1/gateway {"action":"gateway.list"}` with renamed bearer token | Real upstream list returned | Returned live upstream state including per-upstream OAuth | pass |
| `incus exec labby -- journalctl -u labby` (post-rename restart) | No auth/config resolution errors | Clean startup, `env_prefix=LABBY`, `bearer_token_configured=true` | pass |
| `git merge-base --is-ancestor origin/claude/nostalgic-villani-597ea3 main` | Fully merged (safe to clean up) | "fully merged into main" | pass |

## Risks and Rollback

- Live container `.env` and binary were backed up before both mutation passes (`~/.labby/.env.bak.<timestamp>`, `/usr/local/bin/labby.bak.<timestamp>` on the container) and left in place as a rollback path; not pruned this session.
- The `LAB_*` → `LABBY_*` rename is a hard break with no alias layer: any external client or automation hardcoding the old env var names locally (outside this repo/container) will need manual updating — this was a deliberate, explicitly-approved decision, not an oversight.
- `config.toml` fields added this session are additive and backward-compatible (env var still overrides); no rollback risk beyond reverting the relevant commits if a default value proves wrong in practice.

## Decisions Not Taken

- Did not rename `labby-auth`'s `DEFAULT_ENV_PREFIX` shared-crate default — see Technical Decisions.
- Did not wire `LABBY_CODE_MODE_POOL_SIZE`/`_RECYCLE_AFTER`/`_MAX_OVERFLOW` into `config.toml` — genuine construction-order constraint, tracked as `lab-jtp96` instead of rushed.
- Did not attempt to diagnose the exact root cause of the agent-tool worktree-overwrite corruption in depth (e.g. filing an upstream bug against the harness) — flagged to the user as worth reporting via `github.com/anthropics/claude-code/issues`, but not pursued further within this session.
- Did not delete the stale `origin/claude/bold-chaum-49f836` / `origin/claude/nostalgic-villani-597ea3` remote branches — left for the user or a separate cleanup pass given unclear ownership.

## Open Questions

- Whether `origin/claude/bold-chaum-49f836` (a separate, older remote branch about release-please automation) is still active work or safe to delete was not investigated.
- The exact mechanism behind the agent-tool worktree-diff-reapply corruption (item 4 in Sequence of Events) was not root-caused; it may recur with future `isolation: "worktree"` agent dispatches on this or other repos.

## Next Steps

- `lab-jtp96` (P3, open): wire the three Code Mode pool-sizing env vars into `config.toml` once `GatewayManager` construction order allows it, or once the runner pool is made rebuildable post-construction.
- Consider filing the agent-tool corruption behavior as a bug report (per the user's own request earlier in the session) via `github.com/anthropics/claude-code/issues`, and a billing/refund inquiry via Anthropic Console support if token spend recovery is still desired — neither was filed during this session since no tool exists for the assistant to do so directly.
- No other unfinished work from this session; the `.env`→`config.toml` migration is complete for every variable that has a config.toml path, and the live container matches the pushed `main` branch.
