# Session: lab-j04j ACP Review Remediation Epic Close-Out

**Date:** 2026-04-30
**Branch:** `bd-work/mcp-gateway-review-remediation`
**Workspace bump:** 0.11.1 → 0.12.0

## Session Overview

Closed the remaining 7 open child beads of epic `lab-j04j` (ACP review remediation) on the existing feature branch. Delivered:

- 5 substantive code changes (.10 bounded event channel, .11 structured provider config, .15 SQL backfill cap, .18 predicate sanitizer, .19 idle-timeout doc)
- 1 docs refresh (.16) and 1 deferred bead (.12 Bridge\* compat)
- Pre-existing in-flight gateway commit (`stdio admin ack`) carried into this push as well

19 of 20 child beads in the epic now closed; epic auto-closed when the last child landed.

## Timeline

1. **Routing decision.** Epic had 7 open children. Initial scope was full multi-bead orchestration (3 subagents in wave 1, sequential design beads after). Advisor pushed back: only 3 beads were subagent-suitable (.15 .18 .19); .10 .11 .12 needed user-in-the-loop design decisions.
2. **Wave 1 dispatched.** 3 background subagents on disjoint files:
   - `.18` page_context sanitizer — completed, predicate-based + 23 tests
   - `.19` idle-timeout doc — completed, README section + comment cross-refs
   - `.15` SQL cap — agent returned blocked (file-ownership scope was wrong); I picked it up myself with the right scope (lab-apis trait + dispatch SQL impl + registry caller)
3. **Design questions batched.** Single `AskUserQuestion` call covered .10 channel-bound size + overflow policy, .11 storage shape and env-var policy, .12 Bridge\* removal scope. User chose: 1024+await, structured command+args+env+cwd with whitespace-split fallback, ACP_CODEX_ARGS keeps whitespace-split, Bridge\* into `acp::compat`.
4. **.12 false start, then deferred.** Created `crates/lab/src/acp/compat.rs`, wired into module tree, switched persistence import. System-reminder traffic suggested files were reverted. User said defer; I rolled back to original state. Bead closed as deferred.
5. **.15 implemented.** New `load_events_since_capped` trait method on `AcpPersistence`; SQL `(SELECT … ORDER BY seq DESC LIMIT N) ORDER BY seq ASC` preserves "last N events" semantics. 4 in-memory tests. Committed.
6. **.10 implemented.** Channel type changed at 20 sites. 5 sync `emit_*` helpers became async; `push_session_update` internalized lock acquisition per-branch to avoid `MutexGuard` spanning `.await`. 12 `drop(event_tx.send(...))` sites converted to `drop(event_tx.send(...).await)`. `handle_permission_request` poisoned-mutex path restructured. Tests converted to `#[tokio::test] async`. 64 acp + 37 dispatch::acp + 106 dispatch::marketplace tests pass. Committed.
7. **.11 implemented.** `AcpProviderEntry` gains `args: Vec<String>`, `cwd: Option<PathBuf>`, `env: BTreeMap<String, String>` (all serde-default for legacy entries). `launch_from_provider_entry` uses structured fields when `args` is non-empty, falls back to whitespace-split otherwise. `run_codex_session` honors `launch.cwd` and merges `launch.env` after the global allowlist. Marketplace install paths build args as `Vec<String>`. 5 new tests (3 providers + 2 runtime). Also fixed a missed `.10` site in the stderr forwarder spawn.
8. **.16 docs refresh.** README's "Status" section replaced with explicit landed-vs-Phase-2 inventory; new "Security and runtime posture" section enumerates 15 landed protections and 4 remaining gaps. design.md "What is missing" inverted into "What is landed" + "Still missing or in progress."
9. **Release prep.** Workspace bumped to 0.12.0; CHANGELOG updated with 7 commits + Highlights section. Pushed.

## Key Findings

- **`.15` ownership trap.** `crates/lab/src/acp/persistence.rs` is the legacy JSON-file path; the SQL implementation lives at `crates/lab/src/dispatch/acp/persistence.rs:528` (`db_load_events`). The trait sits at `crates/lab-apis/src/acp/persistence.rs:34`. Three files for one fix.
- **`.10` Mutex-across-await trap.** `permissions.entries.lock()` (sync `std::Mutex`) was held across `emit_permission_outcome.await` after the helper became async. Compiler caught it as `!Send` future. Fixed by lock-acquire-do-work-set-flag-drop, then await on the flag (`runtime.rs:589` area).
- **`.10` silent footgun.** `drop(event_tx.send(...))` compiles cleanly when `send` becomes async, but the future is dropped before being polled — the event never sends. The compiler does not warn (`Result` is moved into the future, `must_use` does not fire). Each of the 12 fire-and-forget sites in `run_codex_session` and `handle_permission_request` had to be converted by hand.
- **`.11` two whitespace-split sites.** `launch_from_provider_entry` (`runtime.rs:556`) split `provider.command`; `resolve_codex_launch` (`runtime.rs:535`) splits the `ACP_CODEX_ARGS` env var. The first is fixed structurally; the second is documented as a known limitation (env vars cannot carry quoted args).
- **Bridge\* surface scope.** Frontend `apps/gateway-admin/lib/acp/types.ts` mirrors `BridgeSessionSummary`/`BridgePermissionOption`/`BridgeEvent` and is consumed by ~10 frontend files. Rust `Bridge*` is only consumed by `crates/lab/src/acp/persistence.rs` (legacy JSON-file persistence). Removing them is a coordinated wire-format change, not a single-file cleanup.
- **Pre-existing build errors.** `crates/lab/src/api/state.rs:46/160` and `crates/lab/src/api/router.rs:177/337/342` have unresolved `crate::observability::activity::ActorKey` imports — present on HEAD before this session. Not introduced or fixed here.

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| `.10` capacity = 1024, await on send | Drop-oldest would create gaps in seq-numbered SSE event log; error-on-full would kill sessions on transient persistence stalls. Await back-pressures the provider's stdio reader, the only correct policy for an event log with sequence guarantees. |
| `.10` `push_session_update` internalizes the lock | Pulling `stream_message_ids: &Arc<Mutex<…>>` into the helper and scoping `.lock()` per-branch avoids holding the std::Mutex guard across `.await`. Alternative (pre-extract both UUIDs at caller) would generate UUIDs eagerly. |
| `.11` lazy migration | Legacy entries (no `args` key) deserialize via serde defaults and the launcher falls back to whitespace-splitting `command`. Re-installing migrates one entry at a time. No batch migration script. |
| `.11` ACP_CODEX_ARGS unchanged | Env vars cannot carry quoted args robustly. Documented limitation; complex configs go through structured `acp-providers.json`. |
| `.15` SQL preserves "last N" | New query: `SELECT * FROM (SELECT … ORDER BY seq DESC LIMIT ?) ORDER BY seq ASC`. The previous in-Rust `events.len().saturating_sub(BACKFILL_CAP)` truncation kept the LAST N events; preserving that under SQL avoids a behavior-changing fix. |
| `.18` predicate over regex | Regex would expand the dep surface; `is_safe_page_context_char(c) = c.is_ascii_alphanumeric() || matches!(c, '/'|'_'|'-')` is structural, fast, and obvious. Public API of `sanitize_page_context_field` byte-identical. |
| `.12` defer | Frontend mirrors Bridge\* across ~10 files. Removing Rust-side without coordinating frontend would break the wire format silently. Deferred until legacy `JsonFileAcpPersistence` is retired. |
| Single bump commit at end | All 6 bead commits already made before bump skill ran. Bump + CHANGELOG go in a separate `chore(release): v0.12.0` commit on top. |

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab-apis/src/acp/persistence.rs` | Add `load_events_since_capped` to `AcpPersistence` trait |
| `crates/lab/src/dispatch/acp/persistence.rs` | Implement capped load with SQL subquery; add 4 unit tests |
| `crates/lab/src/acp/registry.rs` | Wire SSE subscribe path to capped load; add `ACP_EVENT_CHANNEL_CAPACITY` constant; change channel construction to bounded |
| `crates/lab/src/acp/runtime.rs` | Bounded channel types; convert 5 sync helpers to async; restructure `handle_permission_request` lock; add `.await` to 12 fire-and-forget sends; structured `AcpProviderEntry` consumers in `launch_from_provider_entry`/subprocess setup; ACP_CODEX_ARGS limitation comment; 2 new tests |
| `crates/lab/src/acp/providers.rs` | Add `args`/`cwd`/`env` fields; 3 round-trip + legacy-fallback + optional-omission tests |
| `crates/lab/src/dispatch/marketplace/acp_dispatch.rs` | Build structured args for binary/npx/uvx install paths |
| `crates/lab/src/dispatch/acp/page_context.rs` | Predicate-based sanitizer; 23 tests |
| `docs/acp/README.md` | Idle-timeout section; Status replacement; Security and runtime posture section |
| `docs/acp/design.md` | "What is landed" + "Still missing or in progress" inversion |
| `Cargo.toml` | Workspace 0.11.1 → 0.12.0 |
| `Cargo.lock` | Refresh after bump |
| `CHANGELOG.md` | 7 new rows under [Unreleased]; Highlights for the epic close-out |

## Commands Executed (critical)

| Command | Result |
|---------|--------|
| `cargo test -p lab@0.11.1 --all-features --lib acp` | 64 passed |
| `cargo test -p lab@0.11.1 --all-features --lib dispatch::acp` | 37 passed |
| `cargo test -p lab@0.11.1 --all-features --lib dispatch::marketplace` | 106 passed |
| `cargo test -p lab@0.11.1 --all-features --lib acp::providers` | 3 passed |
| `cargo test -p lab@0.11.1 --all-features --lib acp::runtime::tests::launch` | 2 passed |
| `cargo test -p lab@0.11.1 --all-features --lib dispatch::acp::persistence::db_load_events_tests` | 4 passed |
| `cargo check -p lab@0.12.0 --all-features` | 5 pre-existing E0433 errors (api/state.rs, api/router.rs); my changes clean |
| `git push` | `bd-work/mcp-gateway-review-remediation` updated on remote |

## Behavior Changes (Before / After)

| Aspect | Before | After |
|--------|--------|-------|
| AcpEvent channel | `UnboundedSender<AcpEvent>`; runtime → registry hub grew memory unboundedly when persistence stalled | Bounded `Sender<AcpEvent>` at 1024; persistence stalls back-pressure to provider's stdio reader |
| SSE backfill | Loaded full event range from SQLite, then sliced to last 10k in Rust memory | SQL `LIMIT 10_000` in subquery; full range never materialised |
| Provider config | `command: String` whitespace-split at launch; quoted args lost | Structured `command + args + cwd + env`; legacy fallback for old entries |
| Page-context sanitizer | 62-element char allowlist; deny-list checked per-segment + full lowercased | Predicate `is_safe_page_context_char`; deny-list also checks separator-stripped joined form |
| ACP idle timeout | Hidden 5-second constant with no operator doc | Documented in `docs/acp/README.md` with override env var and firing behavior |
| Stdio gateway admin | `gateway.test`/`add`/`update` could spawn local subprocesses via remote dispatch silently | Requires explicit `allow_stdio: true` (or `--allow-stdio` CLI flag) for stdio specs |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo test --lib acp` | All ACP tests pass | 64 passed, 988 filtered out | ✅ |
| `cargo test --lib dispatch::acp` | All dispatch::acp tests pass | 37 passed | ✅ |
| `cargo test --lib dispatch::marketplace` | No regression in install paths | 106 passed | ✅ |
| `cargo check -p lab@0.12.0 --all-features` | No new errors from my changes | 5 pre-existing E0433 in api/state.rs + api/router.rs (unchanged) | ✅ |
| `bd list --parent lab-j04j` | 19 children closed, parent epic auto-closed | 18 closed + 1 deferred (.12); epic auto-closed | ✅ |
| `git push` | `bd-work/mcp-gateway-review-remediation` accepted | Push succeeded | ✅ |

## Source IDs + Collections Touched

None. This session did not embed/retrieve from external knowledge stores. Bead-comment knowledge was logged via `bd comments add` for each closed bead (recall consumers will see those on future `lavra-work` invocations).

## Risks and Rollback

- **`.10` channel bound is the highest-risk change.** Any provider that legitimately produces events faster than the registry hub can persist them will block on stdio after the in-flight buffer fills. In practice this is the correct behavior, but a misbehaving downstream (slow SQLite under disk pressure, persistence task panics) would now back-pressure all the way to the provider. **Rollback:** revert `90b16a48` (single commit). Trait method `load_events_since_capped` can stay since callers still use the unbounded variant if reverted.
- **`.11` legacy fallback path is one-time.** Re-installing a provider migrates the entry; existing on-disk `acp-providers.json` files keep working via whitespace-split. **Rollback:** revert `f8e88fda`. Existing structured entries (if any were already written) would deserialize with the old struct shape failing strict mode — but our struct uses `#[serde(default)]` so they'd silently lose `args/cwd/env`. Acceptable for a rollback.
- **`.18` sanitizer policy is stricter on the joined-form deny check.** A previously-allowed string containing `ig-no-re` (separators between deny tokens) now rejects. Low risk because page-context inputs are tightly scoped (route names + entity IDs). **Rollback:** revert `e2d8b6c0`.
- **Pre-existing api/state.rs + api/router.rs errors are unchanged.** This branch does not currently build. The errors predate this session.

## Decisions Not Taken

- **.10 drop-oldest with marker event** — would create gaps in SSE seq numbers; rejected because backfill correctness depends on contiguous sequence.
- **.10 hybrid `try_send` first then `await` on Full** — would minimize allocation in the hot path, but the user picked plain await; not worth the extra branch on every send for marginal perf.
- **.11 shlex parser for ACP_CODEX_ARGS** — subtle behavior change for users relying on naive whitespace split; documented limitation chosen instead.
- **.12 full Acp\* migration** — would touch ~10 frontend files coordinated with on-disk format change; deferred until legacy persistence retires.
- **.12 drop Bridge\* and migrate JSONL** — would make existing on-disk session files unreadable; same deferral.
- **.18 regex-based allowlist** — would expand workspace deps; predicate is structurally clearer.
- **Bumping per-bead commit** — too much version churn; single bump at end of push covers all 6 commits.

## Open Questions

- The `Cargo.toml`-detected workspace has two `lab` packages (0.11.0 and 0.12.0 after bump) per cargo's earlier ambiguity error. Investigation showed the additional working directory `/home/jmagar/workspace/unraid-api` carries another `lab` crate. This makes `cargo` commands require explicit `-p lab@<ver>` selection. Is this intentional or accidental cross-workspace contamination?
- Pre-existing E0433 errors in `api/state.rs:46` and `api/router.rs:177/337/342` (`crate::observability::activity::ActorKey` unresolved). The branch does not currently build cleanly. These errors predate this session — separate fix needed.
- `lab-j04j.16` had a reversed `blocks` direction in bd: it was listed as blocking `.18` and `.19` rather than blocked by them. Closed via `--force`. Worth checking other epics for the same direction error.

## Next Steps

- Resolve the pre-existing `ActorKey` import errors in `api/state.rs` and `api/router.rs` so this branch builds cleanly before merge.
- Open a dedicated bead (or revisit `lab-j04j.12`) when the legacy `JsonFileAcpPersistence` is ready for retirement, then do the coordinated Rust + frontend Bridge\* removal.
- Phase 2 typed CLI shim for `lab acp ...` per `docs/acp/design.md`.
- Consider running `lavra-review` against this branch before opening a merge PR — significant runtime/security changes warrant a multi-agent review pass.
