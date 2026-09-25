# Implementation Plan

Each wave is sized for an implementation agent to complete, review, and test in roughly 10-15 minutes. Execute all waves in this session unless genuinely blocked or unsafe. TDD is mandatory: add/adjust the failing contract test before product code in each behavior wave.

## Wave 1 - task activity contract
- Add failing store/dispatch/frontend tests for task created_at, updated_at, last activity and activity history projection.
- Pin backward compatibility of existing task/schedule JSON.
Gate: failures are only the intended missing fields/history.

## Wave 2 - durable activity and audit projection
- Decode/project existing agent_tasks created_at/updated_at.
- Add bounded read access to existing agent_task_audit rows.
- Surface activity/history through shared dispatch/API adapters without exposing protected payload bytes.
- Add store/dispatch tests.
Gate: focused Rust tests green; no new event database.

## Wave 3 - exponential retry backoff + deterministic jitter
- Add failing tests for increasing retry delays, cap, deterministic jitter, restart stability and eligibility.
- Reuse/generalize labby_runtime::backoff helpers.
- Keep existing retry_policy inputs backward compatible while interpreting backoff_ms as base delay.
- Trace safe retry decision fields.
Gate: focused schedule/backoff tests green.

## Wave 4 - executable task target model
- Add backward-compatible target representation supporting existing Agent targets and saved Snippet targets.
- Preserve existing schedule/task action shapes where possible; defaults decode as Agent.
- Validate target references and authority ceilings in shared dispatch.
- Add schema/migration/serialization tests.
Gate: old schedules still load/run unchanged; Snippet target persists.

## Wave 5 - Snippet execution adapter
- Add failing in-process test for a scheduled executable Snippet.
- Execute via existing snippets dispatch/CodeModeBroker/GatewayManager, never direct HTTP.
- Preserve task cancellation, runtime timeout, fenced settlement, output CAS/result mapping, caller/ToolScope intersection, and structured Code Mode errors.
- Capture safe Code Mode execution metadata for audit.
Gate: fake MCP upstream execution settles through durable task lifecycle.

## Wave 6 - MCP Tasks 2026-07-28 regression audit
- Cover capability negotiation, normal vs task result, tasks/get, tasks/update, tasks/cancel, subject/route auth, retained connection, OAuth invalidation, polling/cancellation, and task notifications/subscription behavior supported by current RMCP.
- Fix gaps without reintroducing removed experimental wire tasks/list or tasks/result.
- Validate clients without Tasks support get synchronous result or spec-defined missing-capability behavior.
Gate: protocol regression tests green.

## Wave 7 - artifact-backed task authoring
- Reuse Artifact control-plane local/personal library plus provider-backed Depot discovery.
- Add task-authoring selector/search with explicit partial-coverage/failure state.
- Label/filter executable artifacts; saved Snippets are the first executable type.
- Keep provider credentials host-side.
Gate: frontend tests prove searchable/selectable executable artifacts.

## Wave 8 - Tasks UI views, filters and sorts
- Reuse Tool Browser/Loadouts Aurora Table/List/Cards toggle pattern.
- Add Table, List and Cards presentations preserving actions/detail.
- Add name search; running/not-running filter; enabled/disabled filter; success/fail filter.
- Add sort by name, last activity, current state/result and next run where meaningful.
- Keep responsive/accessibility behavior.
Gate: component tests cover all requested toggles/filters/sorts with mixed states.

## Wave 9 - task observability surface
- Surface creation/mutation/scheduling/admission/start/retry/cancel/settlement history and safe Code Mode/upstream linkage.
- Add task execution detail/timeline UI using existing audit data.
- Preserve mandatory secret redaction/security boundaries.
- Add tests proving lifecycle coverage and that credentials cannot appear in log/audit projections.
Gate: activity timeline has complete safe lifecycle evidence.

## Wave 10 - production Gotify + Cortex audit Snippet
- Discover exact live Gotify/rgotify/Cortex/1Password tool schemas through Labby.
- Create saved Snippet with exact declared upstream dependencies.
- Acquire credentials from secure existing configuration/1Password; never copy raw token into task definition.
- Exhaustively paginate Gotify messages and Cortex content for trailing 24 hours, track stable IDs/cursors and coverage counts, and return structured issues.
- If credentials cannot be retrieved, send requested rgotify blocker notification.
- Add fixture/unit coverage for pagination logic where practical.
Gate: Snippet validates and real safe manual smoke completes.

## Wave 11 - create hourly Labby task and live smoke
- Create Labby task definition using existing interval cadence every_ms=3600000.
- Enable/arm it and run once immediately through Labby's runtime.
- Verify durable state/activity, result, Code Mode trace linkage, Gotify/Cortex coverage and next scheduled run.
- Validate run-now and cancellation do not corrupt recurrence.
Gate: real Labby task is armed, first run succeeds and next hourly run exists. No ChatGPT/native automation.

## Wave 12 - stale docs/config/env/generated docs and full verification
- Update TASKS.md, AGENT_TASKS.md, relevant UI/config/env docs and comments.
- Regenerate action docs.
- Review touched config/env/test fixtures for drift.
- Run format/focused suites, just check, just test, just lint, just docs-generate, just docs-check, just rustdoc-check, just web-build as applicable.
- Run live MCP task and real-service smokes.
Gate: green checks, or each external blocker captured exactly in issues.md.

## Wave 13 - final evidence/publication
- Review diff for unrelated files/secrets.
- Complete progress/changelog/issues/reflection.
- Commit and push final changes.
- Retry PR creation through authorized path; do not merge.
- Send Gotify completion notification with status/evidence.
Gate: branch current, PR created if authorization permits, completion notification sent.
