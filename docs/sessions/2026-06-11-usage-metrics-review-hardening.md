---
date: 2026-06-11 16:22:38 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: b755e4fd
session id: 7e8cae3b-4275-4f88-80f0-f18559958db7
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/7e8cae3b-4275-4f88-80f0-f18559958db7.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab b755e4fd [main]
beads: lab-sohnl, lab-sohnl.1, lab-sohnl.2, lab-sohnl.3, lab-sohnl.4, lab-sohnl.5, lab-sohnl.6, lab-sohnl.7, lab-7dz7w, lab-9y96u, lab-4zb6m, lab-2r5br, lab-2r5br.1, lab-2r5br.2, lab-2r5br.3, lab-2r5br.4
---

# Usage metrics review hardening

## User Request

The user asked to run Lavra review on the open worktree/PR, dispatch an agent to address all findings, rerun another Lavra review, dispatch PR Review Toolkit agents, address all new findings, commit and push directly to `main`, then save the session as markdown.

## Session Overview

The session closed a chain of usage-dashboard and logs-metrics review findings. The final implementation hardened completion-event storage, removed unbounded historical scans from metrics, fixed privacy handling for actor labels and legacy subjects, consolidated frontend surface labels, verified the changes, and pushed them directly to `main`.

Two code commits were made before this session note: `2807e3f9 fix(metrics): close usage dashboard review findings` and `b755e4fd fix(metrics): harden usage metrics storage`. This session note is a separate path-limited documentation artifact.

## Sequence of Events

1. **Initial Lavra review.** The review targeted the already-merged usage metrics work because no open PR existed. It produced `lab-sohnl.*` findings plus several pre-existing related issues.
2. **Worker remediation.** A worker agent implemented the first review set, closed `lab-sohnl`, its seven children, and three pre-existing review beads, then the work was committed and pushed as `2807e3f9`.
3. **Second Lavra review.** A second review of `2807e3f9` found unbounded completion-event scans, unindexed JSON substring filtering, cleanup issues, and historical actor-label privacy concerns. Beads `lab-2r5br.1` through `lab-2r5br.4` were created.
4. **Main implementation pass.** The issues were fixed in the main checkout: completion reads gained `completion_kind`, previous actors moved to a distinct indexed actor query, labels were sanitized, API metadata was simplified, and frontend surface labels were shared.
5. **PR Review Toolkit pass.** Four PR Review Toolkit agents reviewed the dirty worktree and found additional edge cases: partial v3 migration retry, legacy subject IDs leaking as public actor IDs, missing store-level tests, missing v2 migration test, and duplicated `actor_label = actor_key` semantics.
6. **Final hardening and push.** The PR review findings were fixed, all focused checks plus `cargo check --workspace --all-features` passed, and the final code was committed and pushed to `main` as `b755e4fd`.

## Key Findings

- There was no open GitHub PR at the time of the Codex review pass; `gh pr list --state open` returned `[]`.
- The current checkout was the primary `main` checkout, not a linked worktree: `git worktree list --porcelain` showed only `/home/jmagar/workspace/lab`.
- `logs.metrics` had an introduced path that called `completion_events(None, Some(before_ts))`, materializing all prior completion events to classify returning actors.
- Completion filtering via `fields_json LIKE '%"input_tokens"%'` and `LIKE '%"output_tokens"%'` was not indexable and affected several dashboard endpoints.
- Sanitizing `actor_label` was insufficient while `actor_id` still fell back to historical `fields_json.subject`; legacy subjects needed opaque non-PII identities too.
- A migration that only backfilled `completion_kind` when adding the column could silently skip historical rows after a partial migration or schema drift.

## Technical Decisions

- Store `completion_kind` as an internal `INTEGER` discriminator because it is a derived SQLite index key, not a public API enum.
- Keep the no-silent-truncation behavior from the first fix, but make normal reads indexable with partial indexes on `completion_kind`.
- Use `previous_completion_actor_ids()` with `SELECT DISTINCT actor_key` instead of materializing old `LogEvent` rows for returning-agent metrics.
- Convert legacy `subject` values to stable opaque `legacy-actor-<fingerprint>` IDs so historical rows remain distinguishable without exposing raw email or subject values.
- Stop persisting `actor_label` when it is just a duplicate of `actor_key`; consumers fall back to the stable key.
- Keep docs changes limited to this session artifact because no implementation docs were proven stale during the maintenance pass.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `apps/gateway-admin/app/(admin)/usage/page.tsx` | - | First remediation: use backend facets, debounce search, and avoid unsafe empty-IP select values. | `git show --name-only 2807e3f9` |
| modified | `apps/gateway-admin/components/app-command-palette.tsx` | - | First remediation support for usage/dashboard flow. | `git show --name-only 2807e3f9` |
| modified | `apps/gateway-admin/components/dashboard/analysis-panels.tsx` | - | Added safe surface fallback and then shared `surfaceLabel`. | `git show --name-only 2807e3f9 b755e4fd` |
| modified | `apps/gateway-admin/components/dashboard/recent-calls.tsx` | - | Added safe surface fallback and then shared `surfaceLabel`. | `git show --name-only 2807e3f9 b755e4fd` |
| modified | `apps/gateway-admin/components/dashboard/recent-calls.test.tsx` | - | Covered core runtime and unknown surface labels. | `git show --name-only 2807e3f9` |
| modified | `apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx` | - | Targeted frontend verification touched during first remediation. | `git show --name-only 2807e3f9` |
| created | `apps/gateway-admin/lib/dashboard/surface-label.ts` | - | Centralized dashboard surface labels and fallback formatting. | `git show --name-only b755e4fd` |
| modified | `apps/gateway-admin/lib/api/metrics-client.ts` | - | First remediation: support facets from backend tool-call page. | `git show --name-only 2807e3f9` |
| modified | `apps/gateway-admin/lib/api/metrics-client.test.ts` | - | Covered metrics client/facet behavior. | `git show --name-only 2807e3f9` |
| modified | `apps/gateway-admin/lib/types/metrics.ts` | - | Widened metric surface type and added facet contract. | `git show --name-only 2807e3f9` |
| modified | `crates/lab/src/api/router.rs` | - | Skipped `/v1/logs` without API auth and later removed duplicated service detection. | `git show --name-only 2807e3f9 b755e4fd` |
| modified | `crates/lab/src/api/services/helpers.rs` | - | Removed spoofable IP attribution and raw/duplicated actor labels. | `git show --name-only 2807e3f9 b755e4fd` |
| modified | `crates/lab/src/dispatch/logs/ingest.rs` | - | First remediation: preserve new runtime surfaces. | `git show --name-only 2807e3f9` |
| modified | `crates/lab/src/dispatch/logs/metrics.rs` | - | Added facets, safe labels, opaque legacy actor IDs, and completion-only extraction. | `git show --name-only 2807e3f9 b755e4fd` |
| modified | `crates/lab/src/dispatch/logs/metrics/tests.rs` | - | Added regression coverage for truncation, facets, start exclusion, legacy actor privacy, and previous actors. | `git show --name-only 2807e3f9 b755e4fd` |
| modified | `crates/lab/src/dispatch/logs/store.rs` | - | Added `completion_kind`, retry-safe v3 migration, indexed completion reads, and distinct previous-actor query. | `git show --name-only 2807e3f9 b755e4fd` |
| modified | `crates/lab/src/dispatch/logs/store_schema.sql` | - | Added `completion_kind` column and partial completion indexes. | `git show --name-only b755e4fd` |
| modified | `crates/lab/src/dispatch/logs/types.rs` | - | Routed metrics previous-actor lookup through the store-level distinct query. | `git show --name-only b755e4fd` |
| created | `docs/sessions/2026-06-11-usage-metrics-review-hardening.md` | - | Session artifact requested via `vibin:save-to-md`. | This file |

## Beads Activity

| bead | title | action(s) | final status | why it mattered |
|---|---|---|---|---|
| `lab-sohnl` | Backend: usage metrics aggregation + token instrumentation | Worked and closed by dispatched remediation worker. | closed | Parent for the first usage metrics backend review set. |
| `lab-sohnl.1` | logs.metrics silently truncates usage windows at 10k raw log events | Closed after full-window completion event visibility was restored. | closed | P1 correctness issue from initial Lavra review. |
| `lab-sohnl.2` | API metrics persist raw email/sub actor labels into logs | Closed after raw labels were removed from persisted dispatch metadata. | closed | Privacy issue from initial Lavra review. |
| `lab-sohnl.3` | Usage explorer IP select can crash on empty backend IP values | Closed after empty IP facets were skipped/safely rendered. | closed | Frontend crash risk for real backend data. |
| `lab-sohnl.4` | Usage explorer derives filter facets from a paginated 5000-call sample | Closed after backend facets were added to `ToolCallPage`. | closed | Prevented hidden filter options in large windows. |
| `lab-sohnl.5` | Usage explorer search posts a full logs.calls scan on every keystroke | Closed after debounce/fetch changes. | closed | Reduced interactive backend load. |
| `lab-sohnl.6` | Metrics frontend surface type omits backend-valid core_runtime | Closed after open-ended surface typing and fallback labels. | closed | Fixed frontend/backend contract drift. |
| `lab-sohnl.7` | API source IP metrics trust spoofable forwarding headers | Closed after spoofable forwarded IP headers were ignored. | closed | Prevented authenticated callers from poisoning IP metrics. |
| `lab-7dz7w` | Pre-existing review issue | Closed by the worker as part of the all-issues request. | closed | Included in the user's "ALL issues" acceptance bar. |
| `lab-9y96u` | Pre-existing review issue | Closed by the worker as part of the all-issues request. | closed | Included in the user's "ALL issues" acceptance bar. |
| `lab-4zb6m` | Pre-existing review issue | Closed by the worker as part of the all-issues request. | closed | Included in the user's "ALL issues" acceptance bar. |
| `lab-2r5br` | Gateway overview to usage dashboard | Used as active parent for second review findings. | in_progress | The usage dashboard feature still lists live-verification follow-up notes. |
| `lab-2r5br.1` | Bound returning-actor lookup in logs.metrics | Created, commented, marked blocking, fixed, and closed. | closed | Removed unbounded historical materialization on dashboard refresh. |
| `lab-2r5br.2` | Make completion-event reads indexable and bounded | Created, commented, fixed, and closed. | closed | Replaced JSON substring scans with `completion_kind` and partial indexes. |
| `lab-2r5br.3` | Clean up dashboard metrics follow-up code | Created, fixed, and closed. | closed | Consolidated duplicated/no-op code found in second review. |
| `lab-2r5br.4` | Normalize historical actor labels in usage records | Created, fixed, and closed. | closed | Prevented historical PII labels and later legacy subject IDs from leaking. |

## Repository Maintenance

### Plans

- Checked `docs/plans/`. `docs/plans/complete/mcp-streamable-http-oauth-proxy.md` was already under `complete/`; no move was needed.
- Left `docs/plans/fleet-ws-plan-lab-n07n.md` in place because it describes open bead `lab-n07n` and is not completed session work.

### Beads

- Read recent bead state and interactions with `bd list --all --sort updated --reverse --limit 100 --json`, `bd show`, and parent-child list commands.
- No new beads were created during the save pass.
- Relevant session beads were already closed where verified: `lab-sohnl.*`, `lab-sohnl`, and `lab-2r5br.1` through `.4`.
- `lab-2r5br` remains `in_progress`; the note says its next work is flipping mock data off and live-verifying the real gateway.

### Worktrees and branches

- `git worktree list --porcelain` showed exactly one registered worktree: `/home/jmagar/workspace/lab`.
- `git branch -vv` showed only local branch `main`, tracking `origin/main`.
- `git branch -r -vv` showed `origin/HEAD -> origin/main` and `origin/main`.
- `gh pr list --state open` returned `[]`.
- No branch or worktree cleanup was performed because there were no stale branches/worktrees in the current live repo state.

### Stale docs

- No implementation documentation was directly contradicted by observed commands during the save pass.
- The session artifact itself documents the new storage/migration/privacy behavior and the review sequence.

### Transparency

- The Claude transcript path existed and was sampled; it appears to include an earlier Claude session in the same repo and is therefore cited as observed context, not treated as the sole source for the current Codex conversation.
- No destructive cleanup was attempted.

## Tools and Skills Used

- **Skills.** `lavra:lavra-review`, `vibin:quick-push`, `vibin:save-to-md`, `superpowers:receiving-code-review`, `superpowers:requesting-code-review`, and `superpowers:finishing-a-development-branch`.
- **Subagents.** Lavra review agents, a remediation worker, and PR Review Toolkit agents (`Reviewer`, `Hunter`, `PTA`, `Analyzer`) reviewed or fixed scoped work.
- **Shell and Git.** Used `git`, `gh`, `bd`, `cargo`, `pnpm`, and `tsx` for repository state, issue tracking, verification, commits, and pushes.
- **File editing.** Used patch-based edits for Rust, TypeScript, SQL schema, tests, and this session artifact.
- **MCP tools.** Used Lumen semantic search once; it failed with an embedding HTTP 413, so the workflow fell back to exact local file reads.
- **External CLIs.** `gh` verified open PR state; `bd` managed beads; `cargo` and `pnpm` verified backend/frontend behavior.

## Commands Executed

| command | result |
|---|---|
| `gh pr list --state open --json number,title,headRefName,baseRefName,url --limit 20` | Returned `[]`; no open PRs. |
| `git status --short --branch` | Confirmed dirty work during review, then clean after pushing `b755e4fd`. |
| `git worktree list --porcelain` | Confirmed only `/home/jmagar/workspace/lab` was registered. |
| `bd create ... --parent lab-2r5br --labels ...` | Created `lab-2r5br.1` through `.4` during the second Lavra review. |
| `bd close lab-2r5br.1 ... lab-2r5br.4 ...` | Closed the second review beads after verified fixes. |
| `cargo fmt --all` | Formatted Rust changes; first run caught a raw-string fixture issue, later runs passed. |
| `cargo check --workspace --all-features` | Passed after final hardening. |
| `cargo test -p labby --all-features dispatch::logs::store::tests` | Passed; covered fresh schema, v1 migration, and v2/partial migration repair. |
| `cargo test -p labby --all-features dispatch::logs::metrics::tests` | Passed; covered completion reads, previous actors, start exclusion, facets, and privacy. |
| `cargo test -p labby --all-features api::services::helpers::tests::dispatch_meta_does_not_persist_raw_email_or_subject_as_actor_label` | Passed; API metadata no longer writes raw or duplicate labels. |
| `cargo test -p labby --all-features api::router::tests::logs_routes_are_not_mounted_without_api_auth` | Passed; logs routes remain auth-gated. |
| `pnpm exec tsc --noEmit` | Passed in `apps/gateway-admin`. |
| `pnpm exec tsx --test lib/api/metrics-client.test.ts components/dashboard/recent-calls.test.tsx components/gateway/gateway-form-dialog.test.tsx` | Passed 26 targeted frontend tests. |
| `git commit -m "fix(metrics): close usage dashboard review findings"` | Created `2807e3f9`. |
| `git commit -m "fix(metrics): harden usage metrics storage"` | Created `b755e4fd`. |
| `git push origin main` | Pushed both code commits to `origin/main`. |

## Errors Encountered

- **Lumen semantic search failed.** The embedding service returned HTTP 413 for a large batch. The workflow used exact file reads for known review files instead.
- **Cargo multi-filter commands failed.** `cargo test` accepts one test filter; combined filters produced `unexpected argument` errors. The commands were rerun separately.
- **Rust format initially failed.** A test SQL fixture used a raw string that contained embedded JSON quotes. Switching to a `r#"... "#` raw string resolved it.
- **Cargo lock waits occurred.** Several concurrent verification commands waited on package cache or artifact directory locks, then completed successfully.
- **No open PR existed.** The user expected a PR, but live `gh pr list` showed none; review agents were dispatched against the dirty worktree diff from `origin/main`.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Metrics window completeness | Some usage paths relied on capped/raw log sampling or expensive full materialization. | Completion events are selected by `completion_kind`, preserving full-window semantics without the old raw 10k cap. |
| Returning actors | `logs.metrics` could materialize all historical completion events before the window. | `logs.metrics` uses a distinct indexed actor-key lookup. |
| Completion filtering | SQLite searched `fields_json` with leading-wildcard `LIKE` predicates. | SQLite uses `completion_kind = 1` with partial indexes. |
| Historical actor privacy | Old `actor_label` and `subject` values could appear in labels or IDs. | Email-like labels are sanitized and legacy subjects become opaque `legacy-actor-<fingerprint>` IDs. |
| API dispatch metadata | Actor label could duplicate actor key or previously raw user identity. | Actor key remains stable identity; actor label is omitted unless there is real safe display text. |
| Surface labels | Surface label maps were duplicated in dashboard components. | `surfaceLabel()` is shared in `apps/gateway-admin/lib/dashboard/surface-label.ts`. |
| Logs API auth | `/v1/logs` could be mounted without API auth in prior work. | `/v1/logs` is not mounted without API auth. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo check --workspace --all-features` | Workspace compiles with all features. | Finished successfully. | pass |
| `cargo test -p labby --all-features dispatch::logs::store::tests` | Store schema/migration tests pass. | 3 tests passed. | pass |
| `cargo test -p labby --all-features dispatch::logs::metrics::tests` | Metrics aggregation/privacy tests pass. | 15 tests passed. | pass |
| `cargo test -p labby --all-features api::services::helpers::tests::dispatch_meta_does_not_persist_raw_email_or_subject_as_actor_label` | API metadata privacy test passes. | 1 test passed. | pass |
| `cargo test -p labby --all-features api::router::tests::logs_routes_are_not_mounted_without_api_auth` | Logs route auth gate test passes. | 1 test passed. | pass |
| `pnpm exec tsc --noEmit` | Gateway admin TypeScript typecheck passes. | No errors. | pass |
| `pnpm exec tsx --test lib/api/metrics-client.test.ts components/dashboard/recent-calls.test.tsx components/gateway/gateway-form-dialog.test.tsx` | Targeted frontend tests pass. | 26 tests passed. | pass |
| `git diff --check` | No whitespace/path diff errors. | No output, exit 0. | pass |
| `gh pr list --state open --json ...` | Confirm open PR state. | `[]`. | pass |

## Risks and Rollback

- **Migration risk.** `completion_kind` is a derived column backfilled from JSON text. Rollback path: revert `b755e4fd`; existing databases will keep the added column but code will stop reading it.
- **Legacy identity change.** Historical rows without `actor_key` now show opaque `legacy-actor-<fingerprint>` IDs instead of raw subject values. This is intentional for privacy but changes visible dashboard identifiers for old rows.
- **Metrics scalability.** `logs.calls` still filters/paginates in Rust after fetching window completion events. The immediate review issues are closed, but deeper SQL-side pagination/aggregation could still be future scale work.
- **Parent feature state.** `lab-2r5br` remains `in_progress` for live verification and mock-data follow-up.

## Decisions Not Taken

- Did not create a new PR branch because the user explicitly requested committing and pushing directly to `main`.
- Did not delete branches or worktrees because live evidence showed no extra local/remote branches or registered worktrees.
- Did not move `docs/plans/fleet-ws-plan-lab-n07n.md` to `complete/` because it is tied to an open fleet WebSocket plan.
- Did not add a public Rust/API enum for `completion_kind`; it remains an internal SQLite discriminator.
- Did not update broad implementation docs because no stale doc was proven by the maintenance pass.

## References

- Commit `2807e3f9`: `fix(metrics): close usage dashboard review findings`
- Commit `b755e4fd`: `fix(metrics): harden usage metrics storage`
- Prior session artifact: `docs/sessions/2026-06-11-gateway-usage-dashboard-and-metrics-backend.md`
- Active plan left in place: `docs/plans/fleet-ws-plan-lab-n07n.md`
- Transcript sampled: `/home/jmagar/.claude/projects/-home-jmagar-workspace-lab/7e8cae3b-4275-4f88-80f0-f18559958db7.jsonl`

## Open Questions

- Whether `lab-2r5br` should be closed now or only after its documented live-verification and mock-data follow-up are completed.
- Whether deeper SQL-side filtering/pagination for `logs.calls` should become a future scale bead.
- Whether the older Claude transcript path should be treated as part of this Codex session history or only as same-repo historical context.

## Next Steps

1. Continue `lab-2r5br`: flip `NEXT_PUBLIC_MOCK_DATA` off and live-verify the real gateway dashboard.
2. Consider a follow-up bead for SQL-side `logs.calls` pagination/filtering if production volume warrants it.
3. If another review is desired, run it against `main` at or after `b755e4fd`.
4. Keep this session artifact commit path-limited so code history remains separate from documentation history.
