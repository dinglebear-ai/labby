# Research: Labby Tasks Control Plane

## Scope

This research was completed before product implementation. The relevant task, scheduler, MCP Tasks, Code Mode/Snippet, artifact discovery, reliability, observability, UI, test, docs, and configuration surfaces were inspected so the implementation can reuse established Labby patterns instead of adding a competing scheduler.

## Existing durable Agent Task lifecycle

Relevant files: crates/labby-primitives/src/task.rs, crates/labby-runtime/src/task_runtime.rs, crates/labby/src/access/task.rs, crates/labby/src/dispatch/tasks.rs, crates/labby/src/dispatch/tasks/schedules.rs, crates/labby/src/access/task_schedule.rs, docs/services/TASKS.md, docs/services/AGENT_TASKS.md.

The shared state machine already models created, queued, running, cancelling, succeeded, failed, cancelled, and expired. Task execution uses fenced attempts, leases, owner-scoped concurrency, authority safe-boundary checks, cancellation, recovery, and exactly-once settlement. This is the lifecycle to extend, not replace.

TaskStore persists agent_tasks and already writes agent_task_audit transition rows. The database rows carry created_at/updated_at, but the current TaskRecord and list/get projections omit those timestamps. The existing audit ledger and timestamps should power last activity/history instead of a second event database.

## Existing durable schedules

Schedules already support once, interval, daily, weekly, and five-field cron with IANA timezones. The scheduler polls durable state, uses deterministic occurrence/task identities, claim leases, revision fences, creator-bound delegated authority, missed-run coalescing, pause/edit invalidation, and restart recovery. One-time execution can reuse the existing once cadence.

Current retry semantics are fixed-delay: RetryPolicy stores max_retries plus backoff_ms and settlement observation schedules each eligible retry from that same delay. The assignment requires exponential backoff and jitter.

## Reusable backoff/jitter pattern

crates/labby-runtime/src/backoff.rs already provides reprobe_backoff(attempt), jitter_window(delay), and deterministic jitter_delay(delay, seed). crates/labby-gateway/src/net/backoff.rs re-exports them. This pattern should be reused or generalized for schedule retry bases/caps rather than introducing a second random/backoff implementation. Deterministic jitter is particularly appropriate for reproducible scheduler tests and restart-stable decisions.

## Upstream reliability contract

crates/labby-gateway/src/upstream/pool/capability_call.rs is the canonical upstream-call skeleton. It owns concurrency admission/bulkheads, bounded queueing, deadlines, cancellation, response size limits, typed MCP-vs-transport error classification, circuit-breaker accounting, connection eviction, usage recording, and structured request telemetry. A valid MCP application error marks the connection healthy; transport/protocol failures and timeouts feed health/circuit-breaker state.

Routed MCP task get/update/cancel calls already use the same timed capability path. An automated Labby task must execute upstream work through Code Mode/the gateway pool rather than direct networking so these patterns remain in force.

## Existing MCP Tasks proxy

Relevant files: crates/labby-gateway/src/upstream/pool/tasks.rs, task_route.rs, crates/labby/src/mcp/server.rs, crates/labby/src/mcp/bridge.rs.

Labby already preserves upstream task handles by minting gateway-owned IDs, retaining the originating relay connection, binding task access to caller subject and route authorization, forwarding tasks/get, tasks/update, tasks/cancel, rebinding task notifications to downstream peers, and invalidating OAuth-bound task routes on credential lifecycle changes. The server advertises Tasks when the gateway manager is available.

This MCP extension lifecycle is separate from Labby's product tasks.* Agent Task API. They should compose, not be renamed into each other. Product schedules may execute work that internally becomes an MCP Task; downstream MCP clients must continue receiving the official extension behavior.

## MCP 2026-07-28 contract

Official sources:
- https://tasks.extensions.modelcontextprotocol.io/specification/draft/tasks
- https://modelcontextprotocol.io/specification/2026-07-28
- https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/changelog.mdx
- https://github.com/modelcontextprotocol/rust-sdk

The extension identifier is io.modelcontextprotocol/tasks. Task creation is server-directed from eligible tools/call requests. Current methods are tasks/get, tasks/update, and tasks/cancel. The earlier experimental tasks/list and tasks/result wire methods were removed. Clients should honor pollIntervalMs, persist task IDs when crash-resumable polling is required, use tasks/update for in-task input requests, and treat cancellation as cooperative/eventually consistent. notifications/tasks may be delivered through subscriptions/listen. Streamable HTTP routing uses the task ID as Mcp-Name.

Labby pins rmcp 3.3.0 to dinglebear-ai/rust-sdk revision b19cfc03025047153fa283c0064c651b6e2f623e. RMCP 3.x implements stable 2026-07-28 Tasks and TaskManager. The upstream pool exposes both a high-level final CallToolResult path and a call_tool_once_classified path preserving CallToolResponse task/MRTR outcomes for proxying.

## Saved Snippets / Code Mode are the existing executable workflow primitive

crates/labby/src/dispatch/snippets/dispatch.rs resolves durable saved Snippets and executes them through CodeModeBroker. Snippet execution preserves caller identity/surface, intersects declared tool scope with caller authority, can declare exact upstream dependencies, uses the live GatewayManager, and preserves Code Mode structured errors and bounded outputs.

This is the best first executable artifact target for scheduled work because it automatically reuses Labby's upstream auth, OAuth subject routing, bulkheads, deadlines, rate/circuit controls, MCP task behavior, error contracts, and trace capture.

## Existing scheduled Agent executor is insufficient for the requested audit

crates/labby/src/dispatch/agent_llm.rs is a plain OpenAI-compatible chat executor. It loads pinned Agent instructions/input, opens a provider session, performs one chat call, stores output, and closes the session. It does not expose MCP tools or perform a tool loop. Therefore the hourly Gotify+Cortex audit cannot be implemented correctly as a natural-language Agent schedule alone.

## Artifact discovery pattern

docs/services/DEPOT_PROVIDERS.md documents authority-bound provider discovery that combines the Public/configured Depot providers and models partial coverage/failures. crates/labby/src/dispatch/artifacts.rs composes local durable Artifact actions with remote provider actions. Task authoring should consume this Artifact control plane rather than call Depot directly. Personal/local library and provider/team discovery must remain under the same authorization and pagination contract.

## UI patterns

Current Tasks UI files include apps/gateway-admin/components/depot/task-schedules-page.tsx, task-schedule-form.tsx, and lib/agent-tasks/{client,schedules}.ts. It already supports schedule inventory, arm/pause, run-now, edit/delete, latest run, hero metrics, polling, and All/Armed/Paused filtering. It lacks requested list/card/table views and full status/state/name/activity/result filtering/sorting.

Canonical Aurora view toggle patterns already exist in apps/gateway-admin/components/tools/tool-browser.tsx and loadouts/loadouts-page-content.tsx using Table2/List/Grid2X2 with aria-pressed. gateway-list-content.tsx also provides an established list/cards/table family. Tasks should reuse these exact styling/accessibility conventions.

## Observability/security invariant

Root CLAUDE.md and crates/labby/src/dispatch/CLAUDE.md explicitly forbid secrets, auth values, OAuth material, and raw sensitive parameters from entering logs/traces/errors. The assignment requests completely unredacted observability. Those requirements conflict at the credential boundary. The implementation will provide exhaustive lifecycle, scheduling, admission, retry, execution, upstream-call, MCP-task, cancellation, settlement, and result-reference evidence while retaining mandatory secret protection. Raw bearer tokens/1Password values/OAuth secrets will not be logged. Protected task payload storage should be linked by IDs/digests rather than copied into telemetry.

## Gotify official API findings

Official sources:
- https://gotify.net/api-docs
- https://gotify.net/docs/pushmsg
- https://gotify.net/docs/migrate-to-3

Gotify distinguishes client tokens (receive/manage/list messages/applications) from application tokens (send messages). Preferred token transport is a header such as X-Gotify-Key or Authorization Bearer. GET /message is paginated. Gotify 3 no longer reveals existing application/client tokens on normal GET responses; tokens are shown on creation/rotation. Paging next links are relative. The recurring task therefore must rely on secure existing credentials/1Password or create a dedicated credential through an approved MCP integration, never persist the raw token in task definitions or logs.

## Architecture decision

Extend the existing task/schedule product rather than add a scheduler. Add a backward-compatible executable task target alongside Agent targets, with saved Snippet/Code Mode as the first executable artifact target. Reuse existing once cadence for one-time definitions. Reuse existing task state/audit/authority/recovery semantics. Execute Snippet targets through existing Snippet dispatch/CodeModeBroker and GatewayManager. Reuse artifact-control-plane discovery for authoring. Preserve official MCP Tasks proxy semantics as a distinct wire layer that nested task executions can use.

## Rejected anti-patterns

- second cron daemon or per-task Tokio timers: duplicates durable scheduler/restart behavior;
- direct HTTP from task code to Gotify/Cortex: bypasses auth/reliability/observability/MCP-task routing;
- bespoke retry randomness/backoff: existing runtime helpers exist;
- raw secret logging: violates repository security contract;
- direct Depot-specific search in task UI: duplicates Artifact control plane;
- pretending Labby product tasks.* and MCP io.modelcontextprotocol/tasks are the same protocol: they are separate layers and must compose.
