```yaml
date: 2026-07-15 00:39:44 EST
repo: git@github.com:jmagar/labby.git
branch: chore/refresh-token-fk-and-naming-nit
head: ad7f7f56
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/2120645e-b2e9-4faf-8e34-dcb428e9102e.jsonl
working directory: /home/jmagar/workspace/lab
pr: #242 (merged) — fix(auth): scope refresh-token existence check to the requesting client; #243 (open) — fix(auth): FK constraint on refresh_tokens.client_id + EXISTS naming fix
beads: lab-b9lt4 (+ .1, .2, .4, .5, .6, .7, .8, .9, .10, .11), lab-745z6, lab-0tn3a
```

## User Request

The session opened with a pasted error from a browser tab (`labby.tootie.tv/authorize?...`): `{"kind":"validation_failed","message":"resource must be \`https://labby.tootie.tv/mcp\` or a configured protected MCP route"}`, with the instruction to use systematic-debugging to identify and resolve the issue and push to main. This grew, across the session, into: a client-config fix, a full public-hostname migration from `tootie.tv` to `dinglebear.ai` for the whole rmcp/Labby fleet, discovery and fix of a real OAuth authorization-server bug (`labby-auth`), an 8-agent `/lavra-review` of that fix, and a follow-on PR closing two review findings (including one caught by an automated Codex review mid-session).

## Session Overview

Five broad phases: (1) diagnosed the original `resource must be labby.tootie.tv/mcp` error as a stale client config on `steamy`, not a Labby bug, and fixed it client-side; (2) executed a full SWAG/domain migration of ~13 public hostnames from `tootie.tv` to `dinglebear.ai`, including an unplanned same-session fix of an unrelated `axon` production incident exposed by the migration; (3) diagnosed and fixed a real `labby-auth` bug — a gateway-wide "skip Google consent" optimization that let one OAuth client's refresh token mask another's need to force consent, silently breaking new MCP connector setups (Claude.ai, ChatGPT, new machines) — landed as PR #242; (4) ran `/lavra-review` (8 parallel agents) against #242, fixed all 9 actionable findings plus a CI format failure and a CodeRabbit-requested test, and deployed to production; (5) closed the 2 remaining pre-existing findings as PR #243, including a real bug caught by an automated Codex reviewer (orphaned rows silently surviving the new FK migration), and navigated a mid-session upstream rebase (PR #242 merging) without a force-push.

## Sequence of Events

1. Investigated the pasted `resource must be labby.tootie.tv/mcp` error using `superpowers:systematic-debugging` — checked live `journalctl` logs on the `labby` Incus container on dookie, found the actual rejected request (`requested_resource=https://mcp.tootie.tv/mcp`), traced the registered DCR client's redirect URI to a public-relay callback for machine `steamy`, and via `steamy-wsl`'s `/mnt/c/Users/jmaga/.codex/config.toml` mount found Codex on native Windows `steamy` was configured with the wrong domain (`mcp.tootie.tv` instead of `labby.tootie.tv`). Fixed the client config directly; no Labby code change needed.
2. User requested a domain migration: primary Labby MCP endpoint `labby.tootie.tv/mcp` → `mcp.dinglebear.ai`, protected routes `mcp.tootie.tv/<path>` → `mcp.dinglebear.ai/<path>`. Entered plan mode; investigated SWAG (squirts) config structure, confirmed `*.dinglebear.ai` was already covered by an existing Cloudflare wildcard cert (`EXTRA_DOMAINS`), and mapped the gateway-managed-protected-route model.
3. User expanded scope mid-plan to migrate the whole rmcp fleet (apprise, gotify, ts, unraid, arcane, soma, ytdl, quick-shell, yarr, axon, cortex, synapse) under `dinglebear.ai`, and corrected several plan assumptions (rename SWAG confs in place rather than dual-alias; no new files for services that already had a direct domain; `ytdl`/`shell` needed brand-new confs since they're local-stdio-only upstreams with no existing domain; exhaustive per-domain verification required, not sampling).
4. Executed the approved plan: renamed 10 SWAG proxy confs (`labby`, `mcp`, `apprise`, `gotify`, `tailscale`→`ts`, `unraid`, `arcane`, `yarr`, `axon`, `synapse`) and renamed `syslog.subdomain.conf` → `cortex.subdomain.conf`; created new `ytdl.subdomain.conf` and `shell.subdomain.conf`; updated Labby's `~/.labby/.env` (`LABBY_PUBLIC_URL`, dropped `LABBY_MCP_GATEWAY_URL`, updated redirect URI allowlist) and `config.toml` (deleted a stale duplicate `protected_mcp_routes` entry, added `ytdl`/`shell` routes, updated 7 `[[upstream]]` URLs — caught a missed `yarr` entry via a full-sweep grep before declaring done).
5. Confirmed with the user that the Google OAuth Console redirect URI was updated, then restarted the `labby` systemd service on the `labby` Incus container and verified clean startup.
6. Restarting `labby` surfaced (unrelated) that `axon-native` was crash-looping (~95 restart attempts) on a pre-existing "incompatible_store" schema-cutover guard. First remediation attempt (`axon reset --yes`) targeted the wrong store — a stale local data path from `AXON_HOME`/`AXON_DATA_DIR` mismatch between the systemd unit's explicit `Environment=` and a stale value in `.env` — caught by the user ("that's not what we have our qdrant url set to"); corrected by exporting the exact same env the systemd unit uses, re-running the reset against the real `/mnt/axon-data` store, and directly dropping the stale `axon` Qdrant collection via the Qdrant REST API on the real instance (`100.120.242.29:53333`). `axon-native` confirmed stable afterward.
7. Verified all 13 new/changed domains individually via `curl` (correct 401/200/JSON per domain) and confirmed the 9 renamed `*.tootie.tv` domains now fall through to SWAG's generic landing page rather than leaking the old backend. Updated Labby MCP client configs across dookie, `vivobook-wsl`, `steamy-wsl`, native `steamy`, and `squirts` for Codex, Claude Code, OpenCode, Gemini CLI, and GitHub Copilot CLI as installed per host.
8. User reported `dinglebear.ai/mcp` failing with a ChatGPT-style "no MCP server found" error — root-caused to `dinglebear.ai` having no `/mcp` route at all (it only served the Aurora app). Added `/mcp` + session + `.well-known` locations to `dinglebear.subdomain.conf`; hit a self-inflicted `nginx -t` duplicate-`location` error from a redundant explicit block that collided with the shared `mcp-server.conf` include, fixed by removing the duplicate; later added the missing origin-check parity block after comparing against `lab`/`labby.subdomain.conf`.
9. User reported a second, different failure — "Authorization with Labby failed" from Claude.ai specifically. Live-log investigation showed the full OAuth flow (registration → authorize → Google callback → token mint) completing successfully, but **no subsequent request ever reaching Labby's `/mcp` endpoint** — and a direct authenticated `curl` proved the endpoint itself worked. Both attempts showed `refresh_token_issued=false`.
10. User pushed back on the "Claude requires a refresh token" theory and asked for a comparison against `axon`'s vendored (older, known-working) copy of `lab-auth`. Found the real root cause: `crates/labby-auth/src/authorize.rs`'s `has_any_refresh_token()` checked the entire `refresh_tokens` table for *any* client's valid refresh token, so an already-authorized client (dookie's Codex) silently let a brand-new client (Claude's fresh DCR registration) skip Google's forced-consent screen — and Google only re-issues a refresh token on a forced-consent round trip. Confirmed via `git log -S"force_consent"` that this optimization was a `2026-07-07` regression relative to `axon`'s always-`prompt=consent` behavior.
11. Fixed by scoping the check to `has_refresh_token_for_client(client_id)`. Verified live against production (fresh DCR client → `prompt=consent` present; existing client → absent). Built, deployed to production, committed to a new branch `fix/oauth-refresh-token-scope-per-client`, opened **PR #242**.
12. User ran `/lavra-review` on PR #242: dispatched 8 parallel review agents (architecture-strategist, security-sentinel, performance-oracle, pattern-recognition-specialist, data-integrity-guardian, agent-native-reviewer, git-history-analyzer, code-simplicity-reviewer). Concurrently, CI reported a `Format` failure and CodeRabbit requested a handler-level cross-client regression test; both fixed immediately. Discovered mid-fix that `labby-auth`'s `default = []` features meant every prior "tests pass" claim for `authorize.rs`/`token.rs` had silently excluded those modules — corrected to `--all-features` (64 → 172 tests) and documented the trap in a new `crates/labby-auth/CLAUDE.md`.
13. Synthesized all 8 agents' findings into a full inventory; filed 10 introduced-code beads under parent `lab-b9lt4` (9 fixed and closed: multi-account consent gap, missing crate `CLAUDE.md`, missing `force_consent` observability, the CI format failure, a missing index, a doc-comment convention fix, test-factory duplication, and the CodeRabbit-requested handler test; 1 — `lab-b9lt4.11`, increased exposure to an impatient-client retry race — deliberately left open with a documented, reasoned trade-off) and 2 pre-existing standalone beads (`lab-745z6` FK constraint, `lab-0tn3a` naming nit). Implemented all fixes, pushed, all CI green, replied to and resolved CodeRabbit's thread, closed `lab-b9lt4`, redeployed to production.
14. User invoked `/lavra:lavra-work lab-745z6 lab-0tn3a`. The skill's detailed instructions weren't present in context (likely dropped by compaction); proceeded with the established claim → implement → verify → close pattern instead. Branched `chore/refresh-token-fk-and-naming-nit` off PR #242's (still-unmerged) branch rather than `main`, specifically to avoid a schema-migration-version collision.
15. Fixed the naming nit (`count` → `exists`). For the FK constraint, verified zero orphaned `refresh_tokens.client_id` rows in the live production database and that `registered_clients` is append-only, then implemented a schema migration v4 (SQLite has no `ALTER TABLE ADD CONSTRAINT`, so the table is recreated with the constraint and existing rows copied across inside one transaction). Fixed 4 existing tests that had been inserting refresh tokens for never-registered clients (previously silently permitted); added a `register_test_client()` helper and 2 new tests (constraint enforcement, migration data-preservation). Opened **PR #243** against #242's branch.
16. CI on #243 surfaced a real finding from an automated Codex reviewer: the v4 migration copied data with `PRAGMA foreign_keys` off, so any pre-existing orphaned row would have silently survived (SQLite doesn't retroactively validate rows when the pragma is re-enabled) — defeating the point of the constraint for any other database that happened to have orphans. Fixed by deleting orphaned rows inside the same migration transaction before the copy, with a warning log; added a dedicated test proving orphans are dropped while valid rows survive.
17. While pushing that fix, discovered PR #242 had been merged to `main` during the session (GitHub auto-retargeted #243's base and rebased the branch remotely). Rather than force-pushing over it, verified the rebased commit's tree content was byte-identical to the local one, then used a surgical `git rebase --onto` to replay only the new commit onto the updated remote tip, moved the branch pointer, and pushed cleanly. Replied to and resolved the Codex review thread. All CI green.
18. `/vibin:save-to-md` invoked. Repository maintenance pass: checked `docs/plans/` (nothing session-related to move), verified all touched beads' final states, reviewed worktrees/branches (flagged the now-fully-merged `fix/oauth-refresh-token-scope-per-client` branch as a safe-but-undeleted cleanup candidate; left 4 unrelated worktrees untouched), and found + fixed two stale `labby.tootie.tv` references in `docs/runtime/INCUS.md` (verified both endpoints live on `labby.dinglebear.ai` before editing), landed directly on `main` since it isn't branch-protected.

## Key Findings

- `crates/labby-auth/src/authorize.rs` (pre-fix): `force_consent = !state.store.has_any_refresh_token().await?` checked the *entire* `refresh_tokens` table, not the requesting client — introduced in commit `ee004161` (2026-07-07) to fix a different, real bug (impatient DCR clients timing out on a hardcoded `prompt=consent`), but broadened the check to gateway-global instead of per-client.
- `crates/labby-auth/src/sqlite.rs`: `has_any_refresh_token()` → `has_refresh_token_for_client(client_id)`, adding `AND client_id = ?2` to the SQL predicate — the core fix.
- `crates/labby-auth/Cargo.toml`: `default = []`, and `authorize.rs`/`token.rs`/`metadata.rs`/`middleware.rs`/`routes.rs` are gated behind the `http-axum` feature — meaning a plain `cargo test -p labby-auth` silently skips those modules with no warning. This caused an inaccurate "all tests pass" claim earlier in the session; documented in the new `crates/labby-auth/CLAUDE.md`.
- `axon`'s vendored `lab-auth` copy (`/root/axon-src/vendor/lab-auth/src/google.rs` inside the `axon` Incus container) unconditionally sends `prompt=consent` with no `force_consent`/`AuthorizeUrlRequest.force_consent` field at all — confirmed the current optimization is strictly newer than, and a regression relative to, that always-correct baseline.
- Architecture review (`lab-b9lt4.1`) found the per-client fix was still unsound when more than one Google account is allowed (`resolve_allowed_emails().len() > 1`) sharing one local `client_id` — closed by forcing consent unconditionally in that case.
- Codex review on PR #243 (`crates/labby-auth/src/sqlite.rs:1438`) found the v4 migration's bulk copy ran with `PRAGMA foreign_keys` off and never revalidated existing rows, so pre-existing orphans would silently survive — closed by deleting orphans inside the same migration transaction before the copy.
- `docs/runtime/INCUS.md:126,166` referenced the retired `labby.tootie.tv` domain for the installer/gateway-check URLs — stale relative to this session's `dinglebear.ai` migration; fixed and verified both endpoints live on `labby.dinglebear.ai`.

## Technical Decisions

- Kept `LABBY_MCP_GATEWAY_URL`/`mcp.tootie.tv` reserved for gateway-managed protected routes (per `docs/deploy/README.md`'s documented deployment shape) rather than treating it as an alias for Labby's own primary `/mcp`, after tracing that architecture explicitly through the docs and the host+path-aware `resolve_protected_route` code path.
- Chose to scope `force_consent` by `client_id` (and, later, by allowed-account-count) rather than reverting to the older unconditional-`prompt=consent` behavior — preserves the legitimate UX fix (avoiding a slow round trip for already-consented clients) while closing the cross-client leak.
- For the FK constraint, chose to delete orphaned `refresh_tokens` rows rather than fail the migration outright — an orphaned row is already permanently unusable (`find_client()` rejects that `client_id` on any real auth attempt), so dropping it loses no working state, whereas failing migration would break startup entirely for any instance with legacy orphans.
- Branched PR #243 off PR #242's (then-unmerged) branch instead of `main`, specifically because both touch the same `run_migrations`/`SCHEMA_VERSION` machinery and stacking avoided a migration-version collision (`v3` used twice).
- When PR #242 merged mid-session and the remote branch was rebased, chose a surgical `git rebase --onto` (after verifying tree-identical content) over a force-push, to avoid discarding or duplicating any history.
- Landed the `docs/runtime/INCUS.md` fix directly on `main` via an isolated worktree/temp branch rather than bundling it into PR #243 (wrong scope) or leaving it uncommitted, since `main` has no branch protection and the change was a verified, zero-risk factual correction.

## Files Changed

| status | path | purpose | evidence |
|---|---|---|---|
| modified | `crates/labby-auth/src/authorize.rs` | Scope `force_consent` per-client, then per-allowed-account-count; add observability logging; add 3 new tests | commits `3e80fdf6`, `fcb7e29f` |
| modified | `crates/labby-auth/src/sqlite.rs` | Rename `has_any_refresh_token`→`has_refresh_token_for_client`; add schema migrations v3 (index) and v4 (FK constraint + orphan cleanup); fix `count`→`exists` naming; add/update tests | commits `3e80fdf6`, `fcb7e29f`, `d3957cdd`, `ad7f7f56` |
| created | `crates/labby-auth/CLAUDE.md` | Document the axon/cortex vendoring model and the `force_consent` invariant | commit `fcb7e29f` |
| modified | `docs/runtime/INCUS.md` | Fix 2 stale `labby.tootie.tv` → `labby.dinglebear.ai` references | commit `e6756a1b` (pushed directly to `main`) |
| modified (infra, not in this repo) | 10 SWAG proxy confs on squirts (`labby`, `mcp`, `apprise`, `gotify`, `tailscale`, `unraid`, `arcane`, `yarr`, `axon`, `synapse`) | Renamed `server_name` from `*.tootie.tv` to `*.dinglebear.ai` | live verification via `curl` per domain |
| renamed (infra) | `syslog.subdomain.conf` → `cortex.subdomain.conf` on squirts | Dropped legacy `syslog` naming, single `cortex.dinglebear.ai` server_name | live verification |
| created (infra) | `ytdl.subdomain.conf`, `shell.subdomain.conf` on squirts | New gateway-routed domains for local-stdio-only upstreams | live verification |
| modified (infra) | `~/.labby/.env`, `~/.labby/config.toml` on the `labby` Incus container | `LABBY_PUBLIC_URL`, redirect URIs, protected routes, 7 upstream URLs | `incus exec labby -- journalctl` clean-startup check |
| modified (infra) | `/mnt/axon-data/.env` on the `axon` Incus container | `AXON_MCP_ALLOWED_ORIGINS`/`AXON_ALLOWED_ORIGINS` add `axon.dinglebear.ai` | `axon-native` clean-restart confirmation |
| modified (infra) | `~/.codex/config.toml` (dookie, native steamy, `steamy-wsl`, `vivobook-wsl`, squirts), `~/.claude.json` (same hosts), `~/.config/opencode/opencode.json(c)` (dookie, squirts), `~/.gemini/settings.json` (dookie, `steamy-wsl`, squirts), `~/.copilot/mcp-config.json` (dookie) | Point/add `labby` MCP entries at `mcp.dinglebear.ai/mcp` | per-host verification during the sweep |

## Beads Activity

- `lab-b9lt4` (parent, P1 bug) — created to anchor the `/lavra-review` process for PR #242 (not originally tracked as a bead); claimed, closed with a full outcome summary after all children resolved.
- `lab-b9lt4.1` (P2 bug) — multi-account consent gap; closed, fixed by forcing consent when `resolve_allowed_emails().len() > 1`.
- `lab-b9lt4.2` (P2 task) — missing `crates/labby-auth/CLAUDE.md`; closed, file created.
- `lab-b9lt4.4` (P2 task) — duplicate of `.5`, created by a shell `!`-character quoting bug that caused a client-side JSON-parse failure while the server-side `bd create` had actually succeeded; closed as duplicate with an explanatory note.
- `lab-b9lt4.5` (P2 task) — missing `force_consent` observability; closed, field added to the existing `info!` log.
- `lab-b9lt4.6` (P3 task) — missing index on `refresh_tokens(client_id, expires_at)`; closed, added as schema migration v3.
- `lab-b9lt4.7` (P1 bug) — `cargo fmt --check` CI failure; closed, fixed.
- `lab-b9lt4.8` (P3 task) — doc-comment convention; closed, restructured.
- `lab-b9lt4.9` (P3 task) — `RefreshTokenRow` test-literal duplication; closed, added `sample_refresh_token()` factory.
- `lab-b9lt4.10` (P3 task) — missing handler-level cross-client regression test (also requested by CodeRabbit); closed, test added.
- `lab-b9lt4.11` (P2 task) — increased exposure to an impatient-DCR-client retry race, a side effect of narrowing the consent-skip scope; **left open** — partially mitigated via the new logging and documented trade-off in `crates/labby-auth/CLAUDE.md`, but the full architectural mitigation (state-supersession-on-retry) was judged out of scope for this fix.
- `lab-745z6` (P3 task, standalone/pre-existing) — missing FK constraint on `refresh_tokens.client_id`; claimed, closed in PR #243.
- `lab-0tn3a` (P4 task, standalone/pre-existing) — `count`→`exists` naming nit; claimed, closed in PR #243.

## Repository Maintenance

- **Plans**: checked `docs/plans/` — `docs/plans/complete/mcp-streamable-http-oauth-proxy.md` already correctly filed, `docs/plans/fleet-ws-plan-lab-n07n.md` is an unrelated open brainstorm (bead `lab-n07n`, never touched this session) and was left alone. No session-specific plan file existed in the repo to move (the domain-migration plan lived outside the repo at `/home/jmagar/.claude/plans/sequential-purring-babbage.md`, via the harness's plan-mode system, not `docs/plans/`).
- **Beads**: all beads touched this session are accounted for above; no stale in-progress beads left behind (verified via `bd list --parent lab-b9lt4 --status all`).
- **Worktrees/branches**: `git worktree list` shows 4 worktrees unrelated to this session (`claude/codemode-lazy-describe-types`, `claude/gateway-unraid-plugin-454fe2`, `codex/mise-toolchain-lab-20260713110254`, `marketplace-no-mcp`) — left untouched, unclear/other ownership. `fix/oauth-refresh-token-scope-per-client` (local + `origin`) is fully squash-merged into `main` as `84752e7c` (verified: not a literal ancestor due to squash-merge, but its tree content is byte-identical to what's on `main`) — **safe to delete but not deleted**, flagging for the user per the "confirm before destructive git operations" policy rather than acting unilaterally.
- **Stale docs**: searched all of `docs/*.md` for `labby.tootie.tv`/`mcp.tootie.tv` references outside `docs/sessions/` (historical session logs are intentionally left as point-in-time records, not updated). Found and fixed 2 in `docs/runtime/INCUS.md` (installer URL, `incus sync --check-url`), verified both endpoints live on the new domain before editing, committed and pushed directly to `main`.

## Tools and Skills Used

- **Skills**: `superpowers:systematic-debugging` (initial resource-mismatch diagnosis), plan mode (`EnterPlanMode`/`ExitPlanMode`, domain-migration planning), `lavra:lavra-review` (8-agent PR review), `lavra:lavra-work` (invoked but its detailed instructions weren't present in context this time — likely dropped by compaction; improvised the same claim→implement→verify→close pattern manually and flagged this explicitly to the user), `vibin:save-to-md` (this document).
- **Subagents**: 8 parallel `lavra:review:*`/`lavra:research:git-history-analyzer` agents dispatched via the `Agent` tool for the PR #242 review — all completed successfully with substantive, non-overlapping findings; no failures or retries needed.
- **MCP/tooling**: `bd` (beads) CLI extensively for issue tracking — hit two rough edges: `--tags` is not a valid flag (`--labels` is), and shell-embedded `!` characters in `-d` description strings broke JSON parsing on the client side even though the server-side create succeeded, producing one duplicate bead (`lab-b9lt4.4`/`.5`) that was caught and closed as a duplicate. Switched to `--body-file` for all subsequent bead descriptions to avoid the issue.
- **Infra access**: `incus exec`/`incus file push` (labby and axon containers on dookie), direct SSH to `steamy-wsl` (with `/mnt/c/` Windows-filesystem access for native `steamy` config), `squirts` (SWAG configs), and `qdrant` REST API calls directly against the production instance (`100.120.242.29:53333`) to inspect and repair a collection.
- **GitHub**: `gh` CLI and `gh api`/GraphQL for PR creation, CI-check polling, inline-comment replies, and review-thread resolution (`resolveReviewThread` mutation) for both CodeRabbit and Codex automated reviewers.
- No browser/UI automation tools were used this session.

## Commands Executed

| command | result |
|---|---|
| `incus exec labby -- journalctl -u labby --since "10 minutes ago" ...` | Found the exact rejected OAuth request and, later, the full successful-then-silent OAuth trace that led to the root cause |
| `git log -S"force_consent" --all` (via git-history-analyzer agent) | Confirmed the optimization was introduced in `ee004161` (2026-07-07), one week before this fix |
| `cargo nextest run -p labby-auth --all-features` | 64 → 172 → 173 → 175 → 176 tests as fixes/tests were added; all green at each final check |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | Clean at every checkpoint |
| `cargo fmt --all -- --check` | Failed once (CI-caught), fixed with `cargo fmt --all`, clean thereafter |
| `curl` per-domain checks (13 domains) | All returned expected status/body shapes; 9 renamed `*.tootie.tv` domains confirmed falling through to SWAG's generic page, not leaking the old backend |
| `git rebase --onto origin/chore/refresh-token-fk-and-naming-nit d3957cdd HEAD` | Cleanly replayed only the new orphan-cleanup commit onto the remote's post-merge rebased tip, after verifying tree-identical content between `d3957cdd` and `b1bb9516` |
| `gh api graphql ... resolveReviewThread` | Resolved both the CodeRabbit thread (PR #242) and the Codex thread (PR #243) |

## Errors Encountered

- **Wrong-store `axon reset`**: first remediation attempt for the crash-looping `axon-native` service ran against a stale local data path (default `AXON_HOME=/home/jmagar/.axon` from the manual shell invocation) instead of the real `/mnt/axon-data` the systemd unit's explicit `Environment=` override actually uses. Caught by the user, not self-detected. Root cause: `.env`'s own `AXON_HOME`/`AXON_DATA_DIR` values were stale relative to the systemd unit's overrides, and a manual `incus exec` shell doesn't inherit systemd's `Environment=` lines. Resolved by explicitly exporting the correct values before rerunning, and separately fixing the real Qdrant collection via direct REST API calls.
- **Incomplete "all tests pass" claim**: `labby-auth`'s `default = []` Cargo features meant `cargo test -p labby-auth` (no `--all-features`) silently compiled out `authorize.rs`, `token.rs`, and other `http-axum`-gated modules with zero warning — an earlier "64/64 tests pass" claim in this session never actually exercised those modules. Self-caught while investigating why a newly-added test wasn't appearing in `--list` output; documented permanently in the new `crates/labby-auth/CLAUDE.md`.
- **`bd create` shell-quoting failure with duplicate side effect**: an `!` character inside a `-d` description string broke local JSON parsing of `bd create`'s response, but the underlying create had already succeeded server-side — leading me to retry and create a genuine duplicate bead (`lab-b9lt4.4`), caught by checking `bd list --parent` and closed as a duplicate of `.5`.
- **Rejected `git push` after upstream rebase**: pushing the orphan-cleanup fix was rejected because PR #242 had merged to `main` mid-session and GitHub had auto-rebased the `chore/refresh-token-fk-and-naming-nit` branch remotely. Resolved without a force-push: aborted an initial messy full rebase (which tried to replay already-squashed commits and hit conflicts), verified the superseded commit's tree content was identical, then used a targeted `git rebase --onto` to move only the genuinely new commit.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Labby/rmcp fleet public hostnames | Split across `tootie.tv` (`labby.`, `mcp.`, `apprise.`, etc.) with an ad-hoc mix of direct and gateway-routed domains | Consolidated under `dinglebear.ai`: `mcp.dinglebear.ai` (primary MCP + protected routes), `labby.dinglebear.ai` (web UI), 9 services on direct `<service>.dinglebear.ai` domains, `ytdl`/`shell` newly exposed via the gateway |
| OAuth `force_consent` decision | Gateway-wide: any client's existing refresh token silently let *any other* client skip Google's consent screen, sometimes leaving a brand-new client with no refresh token and a silently-broken connector setup | Scoped per requesting `client_id`, and forced unconditionally whenever more than one Google account is allowed; the decision is now logged |
| `refresh_tokens.client_id` referential integrity | No FK constraint; a token could in principle reference a never-registered client with no DB-level enforcement | `FOREIGN KEY` constraint enforced (schema v4); a bulk-recreate migration also proactively drops any pre-existing orphaned rows rather than silently carrying them forward |
| `labby-auth` crate documentation | No crate-level `CLAUDE.md` | `crates/labby-auth/CLAUDE.md` documents the axon/cortex vendoring model, the `force_consent` invariant, and the `--all-features` testing trap |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo nextest run -p labby-auth --all-features` (final, PR #243) | All pass | 176/176 passed | pass |
| `cargo clippy -p labby-auth --all-features --all-targets -- -D warnings` (final) | Clean | Clean | pass |
| `cargo fmt --all -- --check` (final) | Clean | Clean | pass |
| `cargo check --workspace --all-features` (final) | Clean | Clean | pass |
| `gh pr checks 242` (final) | All green | All green including `ci-gate` | pass |
| `gh pr checks 243` (final, post-rebase) | All green | All green including `ci-gate` | pass |
| `curl` to all 13 new/changed `dinglebear.ai` domains | Correct per-domain status/shape | Confirmed individually, no domain skipped | pass |
| Live `codex mcp login labby` against `mcp.dinglebear.ai` | Correct discovery → registration → authorize URL chain | Confirmed, URL included correct `resource=` and `prompt=consent` | pass |
| `incus exec labby -- journalctl` after each deploy | Clean startup, no errors | Confirmed at each of 3 production deploys this session | pass |
| `incus exec axon -- journalctl -u axon-native` after fix | Stable, no crash loop | Confirmed stable 44s+ with no further failures (vs. ~95 prior restart attempts) | pass |

## Risks and Rollback

- The `refresh_tokens` schema v4 migration is irreversible in place (SQLite table recreate) but is additive/safe: verified zero orphaned rows in production before writing it, and the migration itself now also defensively drops any orphans found elsewhere. Rollback would require restoring from a pre-migration database backup; no such rollback was needed or performed.
- Domain migration changes (`LABBY_PUBLIC_URL`, redirect URIs) invalidated every previously-issued OAuth token/DCR registration tied to the old `labby.tootie.tv` resource — expected and communicated; all known client configs across the fleet were updated in the same session to avoid stranding any of them.
- The `fix/oauth-refresh-token-scope-per-client` branch (local + remote) is confirmed safely merged but was **not deleted**, per the policy against unilateral destructive git operations — flagged to the user in this doc instead.

## Decisions Not Taken

- Did not revert `force_consent` to the older unconditional-`prompt=consent` behavior, despite it being simpler and provably correct (per `axon`'s vendored copy) — the scoped-check approach preserves a real, intentional UX fix from a prior commit (`ee004161`) for the common case.
- Did not implement the full architectural mitigation for `lab-b9lt4.11` (retry-race exposure from forcing consent on every new client's first attempt) — judged as meaningfully larger scope than this bug fix, with real risk of its own regressions if rushed; left open with a documented partial mitigation (logging) instead.
- Did not force-push to reconcile the PR #243 branch after the upstream rebase, even though it would have been faster — used a verified, non-destructive `git rebase --onto` instead.
- Did not delete the now-redundant `fix/oauth-refresh-token-scope-per-client` branch despite confirming it's safe — flagged rather than acted, per the standing policy on destructive git operations.

## References

- PR #242: `fix(auth): scope refresh-token existence check to the requesting client` — https://github.com/jmagar/labby/pull/242 (merged)
- PR #243: `fix(auth): FK constraint on refresh_tokens.client_id + EXISTS naming fix` — https://github.com/jmagar/labby/pull/243 (open, all CI green)
- `docs/deploy/README.md`, `docs/runtime/OAUTH.md` — consulted for the gateway-managed-protected-route and OAuth-flow architecture during the domain migration and bug investigation
- `crates/labby-auth/CLAUDE.md` — new doc written this session capturing the `force_consent` invariant and testing trap

## Open Questions

- Whether `lab-b9lt4.11`'s retry-race exposure will materialize in practice for new-client first-time authorizations is unconfirmed — no live occurrence observed yet; the bead is left open for future triage if it does.
- Whether the user wants `fix/oauth-refresh-token-scope-per-client` (confirmed safely merged) deleted, locally and on `origin`.

## Next Steps

- Merge PR #243 once reviewed (all CI green, Codex's finding already addressed and thread resolved).
- Redeploy the production `labby` service after #243 merges — it wasn't redeployed for the orphan-cleanup fix specifically since production's migration already ran cleanly with zero orphans, but staying on the reviewed tip is still worth doing.
- Decide on deleting the merged `fix/oauth-refresh-token-scope-per-client` branch (local + `origin`).
- Consider `lab-b9lt4.11` (retry-race mitigation) as a future, separately-scoped piece of work if the underlying timeout/retry race is ever observed live.
