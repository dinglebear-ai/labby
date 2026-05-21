# Local Master Log Foundation Design

Date: 2026-04-16
Status: Draft for review
Scope: Local-master foundation only, with explicit extension points for future fleet and syslog ingestion

## Goal

Add a shared logging subsystem for the current master `lab` process that:

- persists structured runtime logs across restarts
- supports indexed historical search
- supports true push live streaming
- is accessible consistently from CLI, MCP, API, and WebUI
- provides explicit and documented extension seams for future remote device and syslog ingestion

This design does **not** include fleet-wide aggregation in v1. It lays the local-master foundation that later fleet and syslog ingestion will build on without requiring a breaking schema or API redesign.

## Why This Belongs In `dispatch`

The repository rules are clear: if behavior is shared across product surfaces, it belongs in one shared execution layer.

This work should therefore live under [`crates/lab/src/dispatch/`](/home/jmagar/workspace/lab/crates/lab/src/dispatch/), not in the CLI, MCP, API, or WebUI layers.

That placement gives us:

- one owner for normalization, persistence, retention, querying, and live subscriptions
- thin adapters for CLI, MCP, API, and WebUI
- one contract for operators and LLM consumers instead of four inconsistent ones

## Non-Goals

- fleet-wide device log ingestion
- remote syslog ingestion
- cross-node correlation logic beyond preserving forward-compatible schema fields
- replacing existing observability policy in [`docs/OBSERVABILITY.md`](/home/jmagar/workspace/lab/docs/OBSERVABILITY.md)
- inventing a separate logging product outside the current `lab` surfaces

## Product Shape

The local-master log foundation has two operator-facing paths:

1. historical search over a persistent indexed local store
2. live push streaming of newly ingested events

The WebUI presents these as one unified timeline. CLI, MCP, and API use the same shared query and stream model exposed by dispatch.

## Recommended Architecture

Use an embedded indexed database as the primary searchable history store.

This is preferred over raw append-only files or a dual journal-plus-index design because it:

- directly satisfies the requirement for MCP, CLI, API, and WebUI search
- avoids building a separate indexing subsystem immediately
- keeps one source of truth for historical queries
- leaves room for later fleet and syslog ingestion to land in the same normalized model

## Shared Dispatch Subsystem

Add a dedicated product-local logging subsystem under `crates/lab/src/dispatch/`.

Suggested module layout:

- `logs.rs` or `logs/` as the module entrypoint
- `types.rs` for normalized event, query, retention, and subscription types
- `ingest.rs` for normalization and intake from tracing-aware producers
- `store.rs` for embedded indexed persistence and retention
- `stream.rs` for in-process live subscriber fanout
- `service.rs` or `dispatch.rs` for shared operations consumed by surface adapters

The subsystem should expose a small shared contract:

- `ingest(event)`
- `search(query)`
- `subscribe(subscription)`
- `stats()`
- `run_maintenance()`

Surface adapters remain thin:

- CLI wraps search, stream, and export workflows
- MCP wraps search and bounded stream-oriented actions in MCP-safe shapes
- API exposes search and live stream endpoints
- WebUI talks to API only

## Normalized Event Model

The subsystem should define one normalized `LogEvent` model for all local-master runtime logs.

All surfaces and storage paths should consume this same shape. Do not let CLI, MCP, API, and WebUI each infer their own event schema from ad hoc JSON or raw message strings.

### Stable Indexed Fields

These fields should be first-class and queryable:

- `event_id`
- `ts`
- `level`
- `subsystem`
- `surface`
- `action`
- `message`
- `request_id`
- `session_id`
- `correlation_id`
- `trace_id`
- `span_id`
- `instance`
- `auth_flow`
- `outcome_kind`

### Flexible Structured Payload

The event should also carry a structured payload field for additional context that is useful for rendering and export without forcing every field into the top-level schema.

Example:

- `fields_json`

This should store redacted structured attributes that do not justify their own first-class indexed column.

### Subsystem Taxonomy

`subsystem` must be explicit, not inferred from message text.

Initial local-master taxonomy:

- `gateway`
- `mcp_server`
- `mcp_client`
- `api`
- `web`
- `oauth_relay`
- `auth_webui`
- `auth_mcp`
- `auth_upstream`
- `core_runtime`

This taxonomy should be used consistently across persistence, querying, API responses, and WebUI filters.

## Explicit Future Extension Points

The local-master foundation must preserve explicit fields for future remote ingest even though v1 does not populate them from remote sources.

Recommended reserved fields:

- `source_kind`
- `source_node_id`
- `source_device_id`
- `ingest_path`
- `upstream_event_id`

These are intentional extension points for future fleet and syslog ingestion into the same searchable model.

This point must be made explicit in both code comments and docs. These fields are **not** accidental over-abstraction. They exist so later remote device and syslog ingestion can reuse the same storage and query contract without a disruptive schema break.

Recommended comment language near these fields:

> Reserved for future remote device and syslog ingestion. Local-master v1 may leave this unset, but the field exists intentionally so later fleet ingest can converge on the same event model and query contract without schema churn.

## Ingestion Model

The new subsystem should ingest structured runtime events from tracing-aware product boundaries rather than scraping terminal output.

High-level flow:

1. a local-master component emits a structured tracing event
2. a tracing-aware ingest layer redacts and converts it into a raw logging payload
3. the payload is enqueued onto a bounded internal channel owned by the logging runtime
4. a dedicated logging worker normalizes it into `LogEvent`
5. the worker writes the event to the indexed local store
6. the worker publishes the event to live in-process subscribers

The live path should not tail the database. Streaming subscribers should receive new events from the ingest worker after normalization.

This keeps live delivery latency low and avoids coupling stream behavior to storage query implementation details.

### Runtime Ownership

The subsystem needs one explicit runtime owner. It should not be implicitly split across `AppState`, CLI shims, and tracing setup.

Recommended shape:

- a single `LogSystem` runtime object owns the bounded ingest queue, background worker, persistent store handle, retention worker, and live subscriber hub
- `main.rs` bootstraps that runtime once during process startup
- long-lived surfaces running inside the process receive an `Arc<LogSystem>`
- process-local one-shot CLI commands open the same on-disk store through a dedicated bootstrap/helper path instead of constructing partial runtime handles
- when a requested capability is unavailable in a one-shot context, the surface returns a clear structured error instead of silently degrading

This owner and lifecycle decision is mandatory. Without it, “shared across CLI, MCP, API, and WebUI” becomes accidental duplication.

### Backpressure And Failure Isolation

The tracing path must not await store writes or subscriber delivery inline.

The local-master foundation should define this behavior explicitly:

- the tracing ingest layer performs a non-blocking enqueue into a bounded channel
- if the queue is full, the system records overflow counters and emits a safe internal health signal without recursively flooding logs
- slow live subscribers are isolated from the ingest path through bounded per-subscriber buffering or a drop-and-resync policy
- persistence failures surface through subsystem health/stats and structured errors without stalling request handling indefinitely

## Persistence and Search

The persistent store is an embedded indexed database used as the source of truth for historical search.

It must support:

- time-range scans
- equality and set filtering on subsystem, level, surface, and action
- targeted lookup by request, session, trace, and correlation identifiers
- text search over `message` and selected structured fields

This is the basis for:

- `lab logs local search ...`
- MCP bounded log query actions
- API historical search endpoints
- WebUI structured filtering

Existing fleet log surfaces remain unchanged:

- CLI: `lab logs search <device> <query>`
- API: `POST /v1/device/logs/search`

## Retention

Retention must enforce both:

- time-based retention, for example `N` days
- size-based retention, for example `N` GB on disk

The effective rule is "whichever limit hits first."

Oldest events should be evicted first. Retention logic belongs to the shared logging subsystem, not to individual API routes or UI views.

Retention behavior should be visible through subsystem stats so operators can inspect current on-disk size, retained time range, and recent eviction activity.

## Redaction and Safety

The new store must obey the observability policy in [`docs/OBSERVABILITY.md`](/home/jmagar/workspace/lab/docs/OBSERVABILITY.md).

The key rule is stronger than "do not display secrets." Secrets must not be persisted in the indexed store at all.

Never store:

- bearer tokens
- auth headers
- cookies
- secret env values
- raw credentials
- unredacted OAuth token material

Normalization and redaction must happen before persistence and before live fanout.

## Surface Contracts

### API

The API should expose separate endpoints for:

- historical log search
- live push subscription

For live browser delivery, prefer SSE over WebSockets for v1.

Reasoning:

- logs are append-only server-to-client data
- SSE is simpler to run through existing HTTP infrastructure
- reconnect semantics fit log streaming well
- control operations can stay on ordinary HTTP endpoints

Hosted same-origin browser session auth is the clean v1 fit for SSE.

If standalone bearer-auth browser mode later needs live streaming, that requires an explicit transport decision because `EventSource` cannot attach arbitrary `Authorization` headers like the current fetch client does.

### CLI

CLI must preserve existing fleet log behavior and add local-master logging as a separate nested surface.

Examples:

- existing fleet query remains: `lab logs search <device> <query>`
- local historical query: `lab logs local search ...`
- local live stream: `lab logs local stream ...`

### MCP

MCP should expose search and bounded tail/poll actions using the same underlying dispatch query types, shaped for MCP transport constraints.

MCP should not attempt to transport a long-lived live subscription object through the ordinary one-request/one-response action contract.

Recommended MCP shape:

- `logs.search`
- `logs.tail` or `logs.poll` with cursor inputs such as `since_event_id` or `after_ts`
- `logs.stats`

### WebUI

The gateway admin app should present one log console page for the current master process.

The default experience should be:

- one unified timeline
- historical results plus live events
- multi-select subsystem filters
- structured search fields
- copyable query state that can map to CLI, MCP, and API representations

The UI should distinguish:

- live auto update: append newly pushed events automatically
- historical refresh: re-run the active query when the user changes filters, time range, or explicitly refreshes

When the operator scrolls away from the live edge, the UI should pause auto-scroll while continuing to buffer incoming events. It should offer a clear "jump to newest" action.

## Query Model

The system should use one shared serializable query model across dispatch, API, CLI, MCP, and WebUI.

At minimum it should support:

- time range
- free text
- level set
- subsystem set
- surface set
- action
- request ID
- session ID
- correlation ID
- cursor fields such as `after_ts` or `since_event_id`

This matters for LLM-assisted debugging. Query state must be portable and inspectable rather than trapped in UI-only form state.

## WebUI Layout Direction

The WebUI should follow the existing gateway-admin visual language rather than inventing a separate log product look.

Recommended layout:

- one primary timeline pane
- one filter and search rail
- explicit live state controls
- operator actions such as copy query and export result window

The page should treat the log console as a first-class operator surface rather than burying it as an incidental activity feed.

## Verification Requirements

The local-master foundation is not complete until the following are verified:

- normalization preserves required indexed fields
- redaction strips secrets before persistence and before live fanout
- historical search works across restarts
- SSE reconnect behavior works correctly
- retention works for time-only, size-only, and combined pressure cases
- CLI, MCP, API, and WebUI all use the same underlying query semantics
- future-ingest extension fields remain present in serialization and docs

## Testing Strategy

Required test coverage:

- unit tests for normalization and redaction
- store tests for search and identifier lookup
- retention tests for age and size boundaries
- stream tests for subscriber behavior and reconnect expectations
- adapter tests proving CLI, API, and MCP preserve query semantics

Add explicit tests around the future-ingest fields so they are not removed later by "cleanup" refactors.

## Implementation Notes

When implementation starts, code and docs must include short comments explaining future-ingest seams.

That documentation should answer two questions clearly:

1. what field or interface exists today
2. why it exists for future fleet or syslog ingestion even if local-master v1 does not yet populate it

Do not leave those seams implicit. Make them obvious enough that a later maintainer understands they are deliberate and supported.

## Open Choices Deferred To Implementation Planning

- exact embedded database choice
- exact on-disk location and configuration knobs
- exact SSE endpoint path and payload envelope
- exact MCP action naming for search and stream workflows
- exact WebUI route and component breakdown

Those should be resolved in the implementation plan, not improvised during coding.

## Summary

The recommended design is:

- a new dispatch-owned logging subsystem
- one explicit `LogSystem` runtime owner
- one normalized event model
- an embedded indexed persistent store
- SSE for live push streaming
- one shared query model across CLI, MCP, API, and WebUI
- bounded MCP tail/poll actions instead of true live MCP streaming
- a unified WebUI timeline for the current master process
- explicit commented extension points for future fleet and syslog ingestion

This gives the current master a useful operator and LLM-facing logging foundation now while preserving a clean path to the later central homelab log platform.
