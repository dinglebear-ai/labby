# Activity Contract Design

## Goal

Make `/activity` a complete operator-facing activity feed for the gateway admin app, covering user actions and system status changes across gateways, services, deployments, devices, MCP primitives, chat sessions, artifacts, marketplaces, registry installs, settings, and OAuth.

The feed must stop being a best-effort projection of arbitrary logs. Logs remain the storage and stream substrate, but activity becomes a stable product contract with explicit event shape, actor semantics, action taxonomy, filters, diagnostics, and tests.

## Current State

### Existing Useful Primitives

- `crates/lab/src/dispatch/logs/types.rs` defines `RawLogEvent`, `LogEvent`, `LogQuery`, `LogSystem`, search, stats, and stream types.
- `crates/lab/src/dispatch/logs/ingest.rs` canonicalizes tracing events into `LogEvent`, redacts secrets, stores them, and publishes to SSE.
- `crates/lab/src/api/services/logs.rs` exposes `POST /v1/logs` for `logs.search`, `GET /v1/logs/stream` for SSE, and peer log ingestion.
- `apps/gateway-admin/lib/api/logs-client.ts` and `apps/gateway-admin/lib/api/logs-stream.ts` already provide frontend search and stream clients.
- `apps/gateway-admin/app/(admin)/activity/page.tsx` already renders an activity page and can query log events.
- `apps/gateway-admin/lib/dashboard/admin-insights.ts` already maps `LogEvent` to `ActivityItem`.

### Gaps Found In Review

- `ActivityItem` is frontend-only. There is no backend activity event contract.
- `LogEvent` has no first-class `actor` or `activity_kind`; the UI currently guesses from `fields_json.subject`, `subsystem`, and `action`.
- Generic HTTP service dispatch logs through `handle_action()`, but `handle_action()` does not accept or record `AuthContext`.
- Gateway API routes already extract `AuthContext` and inject an owner into gateway params, but the activity page cannot reliably use that owner as actor metadata.
- Marketplace API routes use `handle_action()` without actor metadata.
- ACP/chat routes in `crates/lab/src/api/services/acp.rs` are bespoke REST endpoints and do not use `handle_action()`.
- Node/device status updates persist in `NodeStore`, but do not emit a normalized activity event such as `device.status.online` or `device.status.offline`.
- Registry browsing uses `/v0.1` REST routes and registry install uses the client path in `mcpregistry-client.ts`; activity does not have an explicit contract for registry metadata changes or installs.
- Artifact editing/deploy flows in `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx` and `apps/gateway-admin/lib/api/marketplace-client.ts` use marketplace actions, but save/edit/deploy semantics are not normalized as activity.
- `/activity` polls every 10 seconds despite an existing log SSE client.
- Empty states cannot distinguish “log store empty,” “filters hide everything,” “backend unavailable,” “mock mode,” or “activity emitter missing.”

## Design Principles

- Activity is product data. Logs are the transport/storage substrate.
- Every event must be safe to show to an operator. No raw params, secrets, request bodies, tokens, cookies, or file contents.
- Every event must be queryable and streamable through the existing log system.
- Existing log APIs remain backward-compatible.
- The first implementation should avoid adding a second database. Store activity metadata in existing `LogEvent.fields_json` and expose a typed frontend model.
- Backend emission owns source-of-truth events when the backend performs the mutation. Frontend emission is only for UI-local actions that never reach backend mutation code.
- “My activity” must use a stable actor field, not ad hoc `fields_json.subject`.

## Activity Event Contract

Activity is represented as a normal `LogEvent` with a required `fields_json.activity` object.

```json
{
  "event_id": "uuid",
  "ts": 1777100000000,
  "level": "info",
  "subsystem": "gateway",
  "surface": "api",
  "action": "gateway.add",
  "message": "Added gateway github",
  "request_id": "req-...",
  "session_id": null,
  "outcome_kind": "ok",
  "fields_json": {
    "activity": {
      "schema_version": 1,
      "kind": "gateway",
      "verb": "added",
      "title": "Added gateway",
      "summary": "github was added over streamable HTTP",
      "actor": {
        "subject": "google-oauth-sub-or-static-bearer",
        "email": "operator@example.com",
        "issuer": "browser-session",
        "via_session": true
      },
      "target": {
        "type": "gateway",
        "id": "github",
        "name": "github"
      },
      "scope": {
        "surface": "api",
        "service": "gateway",
        "action": "gateway.add"
      },
      "result": {
        "status": "success"
      },
      "links": {
        "logs_request_id": "req-..."
      }
    }
  }
}
```

### Required Activity Fields

- `schema_version`: integer, initially `1`.
- `kind`: one of `gateway`, `service`, `device`, `tool`, `resource`, `prompt`, `chat`, `artifact`, `marketplace`, `plugin`, `registry`, `oauth`, `settings`, `system`.
- `verb`: lowercase past-tense or state verb, for example `added`, `removed`, `enabled`, `disabled`, `deployed`, `online`, `offline`, `called`, `read`, `created`, `edited`, `forked`, `patched`, `started`, `completed`, `failed`.
- `title`: short operator-facing string.
- `summary`: one-sentence display detail.
- `actor`: object or `null`. Required keys when known: `subject`, `email`, `issuer`, `via_session`.
- `target`: object with `type`, `id`, and optional `name`.
- `scope`: object with `surface`, `service`, and `action`.
- `result.status`: one of `success`, `warning`, `error`, `pending`.

### Optional Activity Fields

- `target.parent_type`, `target.parent_id`, `target.path`, `target.node_id`.
- `metadata`: non-secret structured fields needed for filtering and display, for example `tool_name`, `resource_uri`, `prompt_name`, `gateway_id`, `device_id`, `plugin_id`.
- `links`: stable correlation links such as `logs_request_id`, `session_id`, `gateway_id`, `plugin_id`, `registry_server_name`.

## Backend Activity Emission

### New Helper

Create `crates/lab/src/activity.rs` with:

- `ActivityKind`, `ActivityVerb`, `ActivityStatus`, `ActivityActor`, `ActivityTarget`, `ActivityScope`, `ActivityRecord`.
- `ActivityRecord::to_fields_json()` for embedding in `RawLogEvent.fields_json`.
- `emit_activity(record)` that calls `LogSystem::try_ingest(RawLogEvent)` when available and degrades to `tracing::info!` if not.
- `actor_from_auth_context(auth: Option<&AuthContext>) -> Option<ActivityActor>`.

This helper must reuse existing redaction guarantees from `LogIngestLayer` and must not log raw params.

### HTTP Dispatch Integration

Modify `crates/lab/src/api/services/helpers.rs`:

- Add a sibling helper, not a breaking replacement:
  - `handle_action_with_context(service, surface, request_id, auth, req, actions, dispatch)`.
- Keep existing `handle_action()` as a wrapper passing `auth = None`.
- Emit generic activity for successful API mutations when the action is activity-worthy.
- Do not emit activity for high-volume reads by default unless the action is MCP primitive usage or explicitly configured.
- Preserve existing dispatch logs exactly enough not to break current tests.

### Generic Action Classification

Create a backend mapping module, likely `crates/lab/src/activity/catalog.rs`, that maps actions to activity fields:

- `gateway.add` -> `kind=gateway`, `verb=added`.
- `gateway.remove` -> `kind=gateway`, `verb=removed`.
- `gateway.update` -> `kind=gateway`, `verb=updated`.
- `gateway.mcp.enable` -> `kind=gateway`, `verb=enabled`.
- `gateway.mcp.disable` -> `kind=gateway`, `verb=disabled`.
- `gateway.virtual_server.enable` -> `kind=service`, `verb=enabled`.
- `gateway.virtual_server.disable` -> `kind=service`, `verb=disabled`.
- `gateway.virtual_server.set_surface` -> `kind=service`, `verb=updated`.
- `gateway.virtual_server.set_mcp_policy` -> `kind=service`, `verb=updated`.
- `gateway.service_config.set` -> `kind=settings`, `verb=changed`.
- `gateway.server.install` -> `kind=registry`, `verb=installed`.
- `marketplace.sources.add` / `sources.add` -> `kind=marketplace`, `verb=added`.
- `marketplace.sources.remove` / `sources.remove` -> `kind=marketplace`, `verb=removed`.
- `marketplace.plugin.install` / `plugin.install` -> `kind=plugin`, `verb=installed`.
- `marketplace.plugin.uninstall` / `plugin.uninstall` -> `kind=plugin`, `verb=removed`.
- `marketplace.plugin.save` / `plugin.save` -> `kind=artifact`, `verb=edited`.
- `marketplace.plugin.deploy` / `plugin.deploy` -> `kind=artifact`, `verb=deployed`.
- `marketplace.plugin.cherry_pick` / `plugin.cherry_pick` -> `kind=artifact`, `verb=forked`.
- `mcp.call_tool` / `call_tool` -> `kind=tool`, `verb=called`.
- `mcp.read_resource` / `read_resource` -> `kind=resource`, `verb=read`.
- `mcp.get_prompt` / `get_prompt` -> `kind=prompt`, `verb=used`.

The catalog should be data-driven enough that UI and tests can assert expected coverage.

## Actor Contract

### Backend Source

Use `AuthContext` from `crates/lab/src/api/oauth.rs`:

```rust
pub struct AuthContext {
    pub sub: String,
    pub scopes: Vec<String>,
    pub issuer: String,
    pub via_session: bool,
    pub csrf_token: Option<String>,
    pub email: Option<String>,
}
```

`csrf_token` must never be copied into activity.

### Required Route Changes

- Update `crates/lab/src/api/services/gateway.rs` to call `handle_action_with_context`.
- Update `crates/lab/src/api/services/marketplace.rs` to extract optional `Extension<AuthContext>` and call `handle_action_with_context`.
- Update generated/standard service routes only if they need visible activity beyond dispatch logs. The initial scope should focus on gateway, marketplace, logs, and ACP.
- Update `crates/lab/src/api/services/acp.rs` to extract `Extension<AuthContext>` and emit activity in create, prompt, cancel, and stream subscription paths where appropriate.
- Update `/v0.1` registry REST routes only for metadata write routes if they exist; current reviewed file is read-only. Registry install activity should come from the action path used by `installServer()`.

### Frontend Source

`apps/gateway-admin/lib/auth/session-store.ts` remains the browser source for showing/toggling “My activity,” but filtering must read:

- `fields_json.activity.actor.subject`
- fallback: `fields_json.subject`
- fallback: none

## Frontend Activity Model

Create `apps/gateway-admin/lib/activity/types.ts`:

- `ActivityKind`
- `ActivityVerb`
- `ActivityStatus`
- `ActivityActor`
- `ActivityTarget`
- `ActivityRecord`
- `ActivityItem`
- `ActivityFilterState`

Move current `ActivityItem` out of `admin-insights.ts`.

Create `apps/gateway-admin/lib/activity/normalize.ts`:

- `activityRecordFromLogEvent(event: LogEvent): ActivityRecord | null`
- `buildActivityItem(event: LogEvent): ActivityItem`
- `buildActivityItems(events: LogEvent[]): ActivityItem[]`
- fallback classification for legacy logs without `fields_json.activity`

Fallback classification should remain, but display should prefer contract data.

## Frontend Activity API

Create `apps/gateway-admin/lib/api/activity-client.ts`:

- `fetchActivity(query, options)` wraps `fetchLogs()` with activity defaults.
- `connectActivityStream(handlers, options)` wraps `connectLogStream()` and filters/normalizes events.
- `recordActivity(input, options)` posts a frontend-originated activity event.

### `recordActivity`

Add backend action `logs.activity.record` or a new route `POST /v1/activity`.

Recommendation: add `logs.activity.record` first because it reuses `LogSystem`, auth, retention, and existing routing.

Rules:

- Only accepts `fields_json.activity`, not arbitrary log fields.
- Server stamps `ts`, `event_id`, `surface`, `actor`, and `request_id`.
- Client may suggest `kind`, `verb`, `title`, `summary`, `target`, `metadata`.
- Server validates enum values and strips secrets.

Frontend should use `recordActivity` only for UI-local state changes:

- opening a new chat session when ACP backend cannot infer browser intent
- UI settings changes that only affect local state
- client-only artifact draft/fork/patch events before save
- filter/exposure changes only if product wants them as operator activity, not telemetry

Backend mutations should not be double-recorded by frontend.

## `/activity` Page Design

### Data Loading

Replace polling-only `useActivityFeed()` with:

- initial `fetchActivity({ limit: 100 })`
- live `connectActivityStream()`
- `mergeTimelineEvents()`-style dedupe by `event_id`
- fallback polling only when SSE is unavailable

### Filters

Add visible filters:

- Category: all activity kinds.
- Severity/status: success, warning, error, pending.
- Actor: mine/all.
- Text search.
- Source: api, web, mcp, cli, core_runtime.

### Summary Cards

Replace fixed `Events / Tool calls / OAuth events` with:

- Events
- Issues
- Gateways/services
- Tools/resources/prompts
- Devices
- Auth/OAuth

Cards should reflect active filters.

### Empty-State Diagnostics

Fetch `logs.stats` alongside activity.

Show differentiated empty states:

- Backend unavailable: show error and request id when available.
- Log store empty: “No retained log events yet.”
- Activity contract absent: “Logs exist, but no activity events matched. Older logs may not contain activity metadata.”
- Filters hide results: “No events match these filters.”
- Mock mode: clearly label “Mock activity preview.”

## Event Coverage Requirements

### Gateway And Services

Must emit activity for:

- Adding/removing custom gateways.
- Updating gateway config.
- Enabling/disabling gateway MCP runtime.
- Enabling/disabling virtual service servers.
- Enabling/disabling service surfaces: `cli`, `api`, `mcp`, `webui`.
- Changing tool/resource/prompt exposure policy.
- Saving service settings.
- Server install from registry.

Primary files:

- `crates/lab/src/api/services/gateway.rs`
- `crates/lab/src/dispatch/gateway/dispatch.rs`
- `crates/lab/src/dispatch/gateway/manager.rs`
- `apps/gateway-admin/lib/api/gateway-client.ts`
- `apps/gateway-admin/lib/hooks/use-gateways.ts`
- `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- `apps/gateway-admin/components/gateway/gateway-detail-content.tsx`

### Devices

Must emit activity for:

- Device/node online.
- Device/node offline.
- Device status push.
- Device metadata upload only when meaningful, not every heartbeat unless state changed.
- Agent/plugin deployment to device.

Primary files:

- `crates/lab/src/node/store.rs`
- `crates/lab/src/api/nodes/status.rs`
- `crates/lab/src/api/nodes/fleet.rs`
- `crates/lab/src/api/nodes/syslog.rs`
- `crates/lab/src/dispatch/node/send.rs`

Device status activity should be edge-triggered: emit only when `connected` changes or important status fields change. Avoid heartbeat spam.

### MCP Tools, Resources, Prompts

Must emit activity for:

- Tool call.
- Resource read.
- Prompt get.
- Optional low-priority events for list operations.

Primary files:

- `crates/lab/src/mcp/server.rs`
- `crates/lab/src/dispatch/upstream/pool.rs`

List operations can be noisy. Default contract should include `call_tool`, `read_resource`, and `get_prompt`; `list_tools`, `list_resources`, and `list_prompts` may be filterable but not featured in default feed.

### Chat Sessions

Must emit activity for:

- New session in `/chat`.
- Prompt sent.
- Session cancelled.
- Permission requested/resolved if backend receives those events.

Primary files:

- `crates/lab/src/api/services/acp.rs`
- `crates/lab/src/dispatch/acp/dispatch.rs`
- `crates/lab/src/dispatch/acp/persistence.rs`
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`
- `apps/gateway-admin/lib/chat/use-session-events.ts`
- `apps/gateway-admin/lib/chat/session-events.ts`

ACP provider stream events should remain session-local; only summarized milestones become global activity.

### Artifacts

Must emit activity for:

- Created.
- Removed.
- Edited.
- Forked/cherry-picked.
- Patched.
- Deployed to remote device or local plugin target.

Primary files:

- `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx`
- `apps/gateway-admin/lib/api/marketplace-client.ts`
- `crates/lab/src/dispatch/marketplace/dispatch.rs`
- `crates/lab/src/dispatch/marketplace/package.rs`
- `crates/lab/src/dispatch/deploy/runner.rs`

Do not record full file content or diffs in activity. Store path, plugin id, counts, and target only.

### Marketplace And Plugins

Must emit activity for:

- Marketplace added/removed.
- Plugin installed/uninstalled.
- ACP agent installed.
- Plugin deployed.

Primary files:

- `crates/lab/src/api/services/marketplace.rs`
- `crates/lab/src/dispatch/marketplace/dispatch.rs`
- `apps/gateway-admin/lib/hooks/use-marketplace.ts`
- `apps/gateway-admin/lib/api/marketplace-client.ts`
- `apps/gateway-admin/lib/marketplace/api-client.ts`

### Settings

Must emit activity for:

- Service settings changed.
- Gateway exposure settings changed.
- UI-local settings changed, if persisted.

Primary files:

- `apps/gateway-admin/app/(admin)/settings/page.tsx`
- `apps/gateway-admin/lib/hooks/use-gateways.ts`
- `crates/lab/src/api/services/gateway.rs`

Do not emit activity for purely ephemeral UI layout toggles unless explicitly desired.

### OAuth

Must emit activity for:

- Browser session login/logout.
- Upstream OAuth probe/start/status/callback/clear.
- OAuth failures such as state mismatch or callback failure.

Primary files:

- `crates/lab/src/api/browser_session.rs`
- `crates/lab/src/api/upstream_oauth.rs`
- `crates/lab/src/oauth/local_relay.rs`
- `crates/lab/src/oauth/upstream/manager.rs`
- `apps/gateway-admin/lib/api/upstream-oauth-client.ts`

OAuth activity must not include codes, tokens, refresh tokens, cookies, or state secrets.

## API Contract

### Search

Existing:

```json
POST /v1/logs
{ "action": "logs.search", "params": { "query": { "limit": 100 } } }
```

Add query conveniences:

- `activity_only: bool`
- `activity_kinds: string[]`
- `actor_subject: string`
- `target_type: string`
- `target_id: string`

These can initially be implemented as JSON filtering in Rust after SQL query, then migrated to indexed columns if needed.

### Stream

Existing:

```text
GET /v1/logs/stream
```

Add query params:

- `activity_only=true`
- `subsystems=gateway,api,...`
- `levels=info,warn,error`

The current backend `StreamSubscription` already supports filters but the HTTP route does not parse query params. Add query parsing to `stream_logs()`.

### Record

Add:

```json
POST /v1/logs
{
  "action": "logs.activity.record",
  "params": {
    "activity": {
      "kind": "settings",
      "verb": "changed",
      "title": "Changed UI setting",
      "summary": "Activity filter default changed to All users",
      "target": { "type": "ui_setting", "id": "activity.default_filter" },
      "metadata": { "setting": "activity.default_filter" }
    }
  }
}
```

Server response:

```json
{
  "event_id": "uuid",
  "accepted": true
}
```

## Storage And Indexing

Phase 1:

- Store activity data in `fields_json.activity`.
- Search by existing subsystem/action/time filters plus frontend filtering.
- Do not add a migration unless performance requires it.

Phase 2 if needed:

- Add SQLite columns: `activity_kind`, `actor_subject`, `target_type`, `target_id`.
- Populate columns from `fields_json.activity` on insert.
- Add indexes:
  - `(activity_kind, ts DESC)`
  - `(actor_subject, ts DESC)`
  - `(target_type, target_id, ts DESC)`

## Testing Contract

### Rust Tests

Add tests for:

- `ActivityRecord` serialization excludes secrets and csrf.
- `handle_action_with_context()` records actor subject/email on success.
- Destructive authorized intent still logs and records safe activity.
- Gateway add/remove emits activity metadata.
- Service enable/disable emits activity metadata.
- Marketplace plugin install/uninstall emits activity metadata.
- ACP session create emits activity metadata.
- Node status emits online/offline only on transition.
- `logs.activity.record` rejects unknown enum values and strips forbidden fields.
- `logs.search` with `activity_only` returns only activity events.
- SSE stream query filters activity events.

Likely files:

- `crates/lab/src/activity.rs`
- `crates/lab/src/api/services/helpers.rs`
- `crates/lab/tests/logs_api.rs`
- `crates/lab/tests/logs_dispatch.rs`
- `crates/lab/tests/device_api.rs`
- `crates/lab/tests/nodes_api.rs`
- `crates/lab/tests/acp_backend_contract.rs`

### Frontend Tests

Add tests for:

- Contract activity normalizer prefers `fields_json.activity`.
- Legacy log fallback still works.
- Actor filter uses `activity.actor.subject`.
- Category filters work.
- SSE merge dedupes stream and initial search events.
- Empty-state diagnostics distinguish backend error, empty store, filters, and mock mode.
- Mock activity data includes at least one event for each major category.

Likely files:

- `apps/gateway-admin/lib/activity/normalize.test.ts`
- `apps/gateway-admin/lib/api/activity-client.test.ts`
- `apps/gateway-admin/app/(admin)/activity/page.test.tsx` if page-level tests exist; otherwise test extracted hook/model.
- `apps/gateway-admin/lib/dashboard/admin-insights.test.ts` should be narrowed or migrated.
- `apps/gateway-admin/lib/api/logs-client.test.ts`
- `apps/gateway-admin/lib/api/logs-stream.ts`

### Browser Verification

Verify:

- Live mode with real backend shows gateway add/remove activity.
- Mock mode clearly labels mock data.
- “My activity” shows events for authenticated browser subject.
- SSE delivers a new event without waiting for 10-second polling.
- Console has no errors and no failed network requests.

## Rollout Plan

1. Introduce backend `ActivityRecord` and frontend activity types with tests.
2. Add `logs.activity.record`, search filters, and stream filters.
3. Add actor-aware `handle_action_with_context()`.
4. Wire gateway and marketplace API routes to actor-aware dispatch.
5. Emit device online/offline transitions.
6. Emit ACP/chat global session activity.
7. Replace `/activity` polling with initial search plus SSE.
8. Add filters, summaries, and diagnostics.
9. Expand mock activity data to cover every category.
10. Add browser verification script or documented DevTools verification checklist.

## Non-Goals

- No compliance-grade immutable audit log in this phase.
- No raw request body capture.
- No file content or diff capture in activity.
- No long-term analytics warehouse.
- No high-volume telemetry such as every hover, tab switch, filter change, or layout preference unless explicitly promoted to a meaningful setting change.

## Product Decision

`/activity` is an operator convenience feed, not a compliance-grade audit trail.

Implementation should optimize for fast operational visibility, useful summaries, filtering, diagnostics, and near-real-time updates. Activity metadata should still be audit-conscious: include actor and target information when available, avoid secrets, and preserve enough context to explain what happened.

Do not add compliance-only requirements in this phase. If compliance-grade audit is required later, it should be designed as a separate hardening layer with append-only storage, stronger retention controls, immutable event IDs, tamper-evidence, and stricter actor requirements.
