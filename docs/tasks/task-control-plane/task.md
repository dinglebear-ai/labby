# Labby Tasks Control Plane

## Assignment

Build a first-class Labby Tasks system. This is explicitly **not** ChatGPT/native scheduled tasks. Labby must own task definition, scheduling, one-time execution, recurrence, execution state, MCP Task interoperability, observability, persistence, UI, and runtime behavior.

## Required capabilities

### Task lifecycle and scheduling

- Create Labby tasks from the Labby UI/API/tool surface.
- Support one-time tasks.
- Support scheduled tasks.
- Support recurring tasks, including hourly recurrence.
- Support enable/disable state independent from whether a task is currently running.
- Expose run state, last-known activity, and success/failure outcome.
- Permit manual execution without destroying a recurring schedule.
- Persist task definition, scheduling metadata, runtime state, attempt history, and execution evidence.
- Recover gracefully across process restarts and missed/overdue schedules according to established repository conventions.

### Artifact-backed task authoring

Task creation must be able to search and use artifacts available from both team Depot and personal Labby.

### Tasks UI

Provide toggleable list, card, and table views.

Provide filtering and sorting by at least:
- Status: running / not running;
- State: enabled / disabled;
- Name;
- Activity: last known activity;
- Result: success / fail.

Reuse existing Labby UI/UX components, responsive behavior, accessibility conventions, and loading/empty/error patterns.

### Observability

There must be observability into every meaningful task lifecycle and execution event: creation, mutation, scheduling decisions, dispatch, MCP task handoff, upstream/tool calls, retries, backoff, circuit-breaker decisions, rate limits, failures, completions, cancellations, progress, result materialization, notification delivery, and state transitions.

The user explicitly requested no Labby-side redaction or sanitization in this task observability surface. During implementation, reconcile that requirement with any existing repository-wide security or secret-handling invariants. Any mandatory safety boundary that prevents literally raw secret logging must be documented with code/config evidence in research.md and issues.md rather than silently bypassed.

### MCP Tasks interoperability

Labby Tasks must be fully intertwined with MCP Tasks. Audit current MCP Task support and ensure complete support for both roles:
- Labby acting as an MCP client to upstream MCP servers that implement Tasks; and
- Labby acting as an MCP server to downstream MCP clients that implement Tasks.

Fill all gaps in capability negotiation, creation, progress/status/result retrieval, cancellation, errors, persistence, lifecycle propagation, and graceful fallback. Labby-owned scheduled/recurring tasks should reuse the shared MCP task lifecycle/execution layer rather than a disconnected scheduler-only execution model.

### Reliability and repository-pattern requirements

Exhaustively inspect and reuse established patterns for:
- UI/UX;
- logging;
- observability/tracing;
- error handling;
- graceful degradation;
- rate limiting;
- circuit breakers;
- exponential backoff;
- jitter;
- persistence;
- background workers;
- cancellation;
- health reporting;
- config/env;
- testing/smokes.

Any hand-rolled behavior must be justified in research.md with concrete file/symbol/config evidence showing the existing pattern is absent or insufficient.

## First production task

Create and validate an hourly Labby task that:
1. Checks Gotify at https://gotify.tootie.tv for any and all issues.
2. Systematically, exhaustively, and completely searches Cortex for everything ingested in the preceding 24 hours.
3. Relentlessly reviews every retrieved Cortex item for any and all issues.
4. Records complete execution evidence in Labby Tasks observability.

Credential/key acquisition options:
- 1Password MCP;
- 1Password CLI on macpoo;
- rgotify MCP via Labby, including key creation if supported.

If Gotify credentials cannot be retrieved, send the user a Gotify notification through rgotify explaining the credential-retrieval blocker.

## Required engineering process

1. Work only in the dedicated worktree for this slice.
2. Create this task.md before implementation/research, commit it, push it, and open a PR.
3. Conduct exhaustive repository review of relevant code, docs, tests, config, and env.
4. Log every reusable pattern and supporting evidence in research.md.
5. Thoroughly review all relevant official web documentation and upstream repositories, including MCP Tasks, selected scheduling/runtime primitives, Gotify APIs, Cortex/Labby/Depot integration surfaces, and adjacent reliability patterns.
6. Update research.md with external findings and links.
7. Create plan.md with a complete implementation plan divided into roughly 10-15 minute agent-sized waves.
8. Create progress.md before implementation, commit, and push.
9. Implement the entire plan in one pass using TDD.
10. Update progress.md after every wave.
11. Run focused tests throughout and full relevant suites before completion.
12. Run live smoke tests against real services wherever safe.
13. Document every issue in issues.md with exact logs/errors, context, retries, alternatives, searches, investigations, root-cause hypothesis, and proof.
14. Review and update every stale doc/test/config/env item needed to align the environment with current code.
15. Maintain changelog.md covering config/env, docs, tests, and code files added/removed/changed.
16. Create reflection.md covering session progression, rationale, alternatives, tools/skills used and omitted, what to do differently, and follow-up slices.
17. Verify all markdown accurately reconstructs the session; use Cortex/session history if evidence is missing.
18. Commit and push all final work. Do not merge.
19. Send a Gotify completion notification with status and concise evidence.

## Completion gates

The task is complete only when all implementation waves are done; MCP client/server Task support is verified end-to-end; scheduling/recurrence/persistence/UI/filtering/sorting/execution/observability are tested; the hourly Gotify+Cortex task exists and successfully completes a safe real smoke; relevant live smokes pass; all required docs are complete; tests/checks are green; branch and PR are current; Gotify completion notification is sent; and nothing is merged.
