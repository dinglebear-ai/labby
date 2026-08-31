---
title: "Observability"
created: "2026-07-30"
updated: "2026-07-30"
---

# Observability

This document is the canonical observability contract for `lab`.

It defines:

- where instrumentation is mandatory
- which structured fields are required
- how caller context flows across boundaries
- what must never be logged
- what must be verified before a service is considered online

This is not optional guidance. Service integrations and shared infrastructure must conform to it.

## Goal

Every user-visible service action must be traceable end to end across:

- CLI dispatch
- MCP dispatch
- API dispatch
- shared SDK transport
- service health probes

When a request fails, operators must be able to answer:

- which surface invoked it
- which service and action ran
- which instance was targeted
- which outbound request was attempted
- whether the failure happened in validation, auth, transport, or server response handling

## Ownership

Observability is split across two layers:

- `lab` owns caller context and dispatch logging
- `labby-apis` owns outbound request logging and transport failure detail

That means:

- CLI, MCP, and API must log the user-visible action boundary
- `HttpClient` must log every outbound request
- service modules must not invent custom logging formats

## Mandatory Instrumentation Points

The following boundaries must emit structured logs.

### CLI Dispatch

Every CLI service action must emit one dispatch event.

Required fields:

- `surface = "cli"`
- `service`
- `action`
- `elapsed_ms`

Optional when applicable:

- `instance`
- `operation = "health"`
- `kind` on failure

### MCP Dispatch

Every MCP tool action must emit one dispatch event.

If the client has opted into MCP logging notifications, any notification derived
from that dispatch must reuse the same action context and apply the same
redaction rules before shipping error text back to the client.

Required fields:

- `surface = "mcp"`
- `service`
- `action`
- `elapsed_ms`

Optional when applicable:

- `instance`
- `operation = "health"`
- `kind` on failure
- `input_tokens` / `output_tokens` — estimated request/response token counts
  (≈chars/4 heuristic; `output_tokens = 0` on failure) on the dispatch finish event

### API Dispatch

Every product API service action must emit one dispatch event.

Required fields:

- `surface = "api"`
- `service`
- `action`
- `elapsed_ms`
- `request_id`

Optional when applicable:

- `instance`
- `operation = "health"`
- `kind` on failure
- `input_tokens` / `output_tokens` — estimated request/response token counts
  (≈chars/4 heuristic; `output_tokens = 0` on failure) on the dispatch finish event

This same contract applies to auth-adjacent HTTP handlers that are part of the
product surface, including:

- `/auth/session`
- `/auth/logout`
- OAuth authorize/callback/token handlers where `lab` itself is the actor

Those routes must not silently bypass the normal dispatch schema just because
they are not mounted under `/v1/{service}`.

### Actor Correlation

Operator-facing events that have an authenticated subject must use `actor_key`
for activity scoping instead of persisting or exposing the raw subject. The
actor key is:

- `HMAC-SHA256(subject, LABBY_ACTOR_KEY_SECRET)`
- hex encoded as 64 lowercase characters
- stable for one installation as long as `LABBY_ACTOR_KEY_SECRET` is preserved
- intentionally not portable across installations with different secrets

`LABBY_ACTOR_KEY_SECRET` is a secret value stored in `~/.labby/.env`. If absent,
`lab` generates it on first use. Empty or anonymous subjects have no
`actor_key`; `mine_only` style activity queries must exclude those rows rather
than inventing a sentinel actor.

Compute `actor_key` once when binding an authenticated session, then clone that
bound value into later events. Do not derive it inside tracing subscriber
callbacks or per-log-event hot paths.

The raw subject remains a credential-adjacent identifier and must not be stored
in persisted log fields or returned to the Activity UI. A short redacted display
tag is allowed only for human diagnostics and must not be used for
authorization or filtering.

### Shared Outbound Requests

`labby-apis::core::HttpClient` must emit:

- one `request.start` event before every outbound call
- one `request.finish` event on success
- one `request.error` event on failure

This applies to all shared request helpers, including:

- `get_json`
- `get_json_query`
- `get_void`
- `post_json`
- `post_void`
- `put_json`
- `patch_json`
- `delete`
- `delete_query`

The upstream OAuth HTTP client follows the same request event contract for
discovery, registration, token exchange, and refresh requests.

`HttpClient` logs must inherit the caller span from CLI, MCP, or HTTP dispatch.

### Outbound RMCP Client Requests

Outbound RMCP client operations are part of the same observability contract as
shared HTTP requests.

Every proxied upstream RMCP operation must emit:

- one start event before the outbound RPC
- one finish event on success
- one error event on failure or timeout

Required fields:

- `upstream`
- `capability`
- `operation`
- `elapsed_ms` on finish/error

When the call originates from API or HTTP MCP, the RMCP events must inherit the
surrounding caller context, including `request_id` when present. Timeouts must
be logged as explicit failures rather than disappearing into generic disconnect
noise.

SEP-2243 tool-header recovery has its own structured sub-events. A typed rmcp
`HEADER_MISMATCH` emits `action = "tool.header_mismatch"`, `event = "detected"`,
`upstream`, and `mismatch_count`. The same-peer `tools/list` refresh emits
`action = "tool.header_cache.refresh"` with `event = "start|finish|error"` and
`schema_refresh_count`; a replay emits `action = "tool.header_cache.retry"` with
`event = "finish|error"` and the corresponding retry counter. Gateway runtime
status exposes these cumulative per-upstream values as `header_recovery`, omitted
while all counts are zero. Never emit tool arguments or synthesized
`Mcp-Param-*` values in these events.

Resource catalog fan-out uses `operation = "resources.list"`. Each upstream
emits `upstream.request.start` followed by `upstream.request.finish` or
`upstream.request.error`, including `subject_scoped = true` for OAuth resource
passes. This makes a slow catalog refresh attributable to the exact upstream
and distinguishes connection acquisition, timeout, upstream error, and success.
Each shared or subject-scoped fan-out phase has a 10-second ceiling (or the
smaller configured upstream request timeout), and preserves partial results.
The combined MCP `prompts/list` path is stricter: raw and subject-scoped OAuth
passes share one absolute upstream request deadline, including cold connection
acquisition, concurrency-gate wait, and cursor pagination. Deadline warnings
use `kind = "timeout"`, identify the phase, and set `partial_result = true`;
the request returns all prompts completed before the deadline.

Cold subject-scoped tool, prompt, and resource discovery shares the configured
fleet-wide discovery concurrency gate. Resource aggregation additionally emits
a warning when its global item or serialized-byte cap truncates the partial
catalog; the event includes `limit` and the accepted item or byte count. These
events must not include the OAuth subject.

Resource subscription reconciliation is not part of the caller's
`resources/list` latency budget. It runs as a coalesced background batch with:

- `action = "subscription.refresh.schedule"`, `phase = "scheduled|coalesced"`
- `action = "subscription.refresh.batch"`, `phase = "start|finish|cancelled"`
- `upstream_count` on every event and `elapsed_ms` on batch completion

Pool drain cancels pending reconciliation so an obsolete pool cannot recreate
subscriptions after replacement.

The 2026-07-28 MCP surface does not advertise the removed legacy logging
capability. Required observability is emitted through local structured tracing,
not `logging/setLevel` or `notifications/message`.

### Gateway usage telemetry (`UsageStore`)

Every upstream tool/resource/prompt call outcome recorded by `upstream.request.finish`/`upstream.request.error` (above) is also durably persisted to a small SQLite store at `~/.labby/usage.db`, via `UpstreamPool`'s `timed_capability_call` choke point (`crates/labby-gateway/src/upstream/pool/capability_call.rs`). This is a fire-and-forget write (`tokio::spawn`) — it never adds latency or failure risk to the call it's observing, and a write failure is logged (`usage store record_call failed`) and dropped, never surfaced to the caller.

Query it via `gateway.usage.metrics` and `gateway.usage.calls` — both admin-gated, same as `gateway.enrich.*`. `gateway.usage.metrics` computes complete-window totals, failures, latency percentiles, top/least/slow targets, actors, upstream distribution, throughput, hourly activity, time buckets, and optional stable filter facets from the durable store. Exact aggregation is limited to 250,000 matching rows; broader queries return `invalid_param` and must be narrowed. Optional facets also return `invalid_param` when any facet exceeds 1,000 distinct values rather than silently returning an incomplete inventory. Hourly activity accepts an IANA timezone for DST-correct local-hour buckets, with `timezone_offset_minutes` retained as a fixed-offset compatibility fallback in the inclusive range -1440 to 1440. `gateway.usage.calls` is the bounded keyset-paginated event explorer and supports the same upstream/target/capability/operation/subject-scope/actor/outcome/search filters. Metrics facets enumerate capability, operation, and shared-versus-subject scope alongside targets, actors, upstreams, and outcomes; slowest-target rows preserve that complete dimensional identity. CLI: `labby gateway usage metrics` / `labby gateway usage calls`; the CLI exposes the same filters and aggregate bucket/facet options as the shared action contract. Both actions enforce the same route-scope restriction as `gateway.enrich.*` — a route-scoped caller only sees usage data for the upstreams visible on their route.

Set `LABBY_GATEWAY_USAGE_DISABLED=1` to disable capture entirely (no store is opened at startup). Retained rows are pruned on a 6-hour cycle to a 30-day retention window; `labby serve` starts the loop but the batched deletion logic (`UsageStore::spawn_prune_loop`/`prune_older_than`, deleting up to 5,000 rows per statement so a large backlog never holds SQLite's writer lock for long) lives entirely in `UsageStore`.

In-flight fire-and-forget writes are capped by a semaphore (`WRITE_SEMAPHORE_PERMITS`, 64 permits) — a saturated burst drops the write and logs a warning rather than queuing unboundedly or spawning an unbounded number of tasks. `~/.labby/usage.db` is created with owner-only (`0600`) permissions since `actor` is a stable per-user identifier, even though nothing in the store is a credential.

This store intentionally does not capture CLI/HTTP/MCP dispatch-level events for the `gateway` service's own actions (e.g. `gateway.add`, `gateway.enrich.preview`) — only calls proxied through to upstreams. Schema version 2 records `ts_unix`, `upstream_name`, `tool_name`, `capability`, `operation`, `subject_scoped`, `actor`, `outcome`, `elapsed_ms`, and nullable `response_bytes` (see `crates/labby-gateway/src/usage/types.rs`). Existing version-1 databases migrate in place. `response_bytes` is present only when an upstream returned a complete response; queue, connection, upstream-error, and timeout outcomes keep it null.

The operator UI deliberately separates these two retention shapes:

- **Usage** reads the 30-day SQLite store for durable upstream volume, latency, outcome, actor, capability, operation, OAuth-scope, and response-size analysis.
- **Traces** reads a bounded admin-only `server_logs.query` window with `correlated_only` and `stop_after_limit` enabled, then groups emitted `trace_id`, `request_id`, or `execution_id` fields into request timelines. The log normalizer promotes only those correlation identifiers from tracing span context into the normalized event fields, so nested upstream events inherit the outer request identity without flattening arbitrary span metadata. Root request terminal events determine success/failure; child upstream finishes or warnings cannot complete or fail the parent request. When the retained query is truncated, the oldest correlation group is discarded because it may have been cut at the sample boundary.
- **Overview** combines the durable Usage totals with a bounded retained-log sample for dispatch-by-surface, estimated tokens-by-tool, and Code Mode fan-out. Its log query stops after the retained-entry limit and uses a small scan budget; the dashboard refreshes on a slower cadence than the live trace view so observability does not become a sustained log-scanning workload. Those panels must be labeled as retained samples; token values are the `chars / 4` estimates emitted at dispatch boundaries, not provider billing totals. A successful empty log query is a collected zero, while an unavailable log query leaves only those three dimensions uncollected.

Raw source IP is not a Usage or Traces metric. Do not add it merely to populate an operator card; retain the privacy-safe `actor_key` contract above unless a separately reviewed security requirement calls for network-source retention.

### Health Probes

Health probes are not normal business actions and must be distinguishable in logs.

When a health check runs, logs must include:

- `operation = "health"`

Health probes must also preserve the normal dispatch and request fields for their surface.

### Destructive Actions

Destructive actions must log:

- intent before execution
- outcome after execution

Intent logs must make it clear which action is about to mutate state. Outcome logs must indicate success or failure.

Gateway reconcile actions must log their mutation intent and outcome:

- `gateway.add`
- `gateway.update`
- `gateway.remove`
- `gateway.reload`

Those actions must also log reconcile phase transitions and outcome details
without exposing credential-bearing URLs, commands, tokens, or secret env
values.

### Catalog Change Notifications

`notifications/tools/list_changed` is a client-visible side effect, not an
internal event: clients discard and rebuild their connector namespace when they
receive one. A burst of them invalidates tool bindings mid-turn, so calls fail
*before* reaching Labby and carry no dispatch trace of their own. That makes the
notification path the only place the failure is observable, and it is therefore
instrumented as a first-class boundary.

**One choke point.** Every emitter funnels through
`mcp/catalog_notifications.rs::notify_catalog_peers`. Churn accounting happens
there and nowhere else — recording at the individual emitters would count one
diff once per connected peer. New emitters must route through it rather than
calling `peer.notify_*_list_changed()` directly.

The sole exception is subscription catch-up, which also lives in
`mcp/catalog_notifications.rs`. A catalog change can finish before a session
registers its notification sink. Immediately after registration, Labby compares
the current visible tool contract with the last **complete** `tools/list`
contract that session received. A mismatch emits one
`action = "catalog.notify.catchup"` event and one `tools/list_changed` signal.
It may race with normal fanout and produce a harmless duplicate; losing the only
signal is forbidden. A failed or timed-out catch-up send emits
`action = "peer.disconnect"`, `phase = "tools.catchup"`, and prunes the sink.

Only the final page of a revision-bound `tools/list` publishes that session's
baseline. An interrupted or partial pagination sequence remains unpublished,
so subscribing afterward receives the conservative catch-up signal. Continuation
cursors are bound to the descriptor-set revision; stale or unversioned tool-list
cursors fail instead of silently joining pages from different contracts.

**Every emission must be attributed.** `notify_catalog_peers` takes a `source`
label from `labby_runtime::catalog_notify`, which is the single vocabulary
shared by the gateway and MCP crates:

| `source` | Emitted by |
|---|---|
| `gateway.reload.selective` | reconcile that kept the live pool and selectively reconciled added upstreams |
| `gateway.reload.full` | reconcile that rebuilt the upstream pool |
| `gateway.code_mode.set` | Code Mode contract update that did not rebuild the upstream pool, including the legacy Code Mode inspector switch |
| `gateway.mcp_apps.set` | Labby-owned MCP App visibility update that did not rebuild the upstream pool |
| `gateway.enrich.hint_apply` | `gateway.enrich.hint.apply` writing a `code_mode_hint` |
| `mcp.call.codemode` | post-run catalog delta observed by a `codemode` call |
| `mcp.call.mcp_app` | Labby-owned MCP App visibility change made through the always-available `mcp_app` control tool |
| `mcp.call.upstream` | post-call catalog delta observed by a raw upstream proxy call |
| `upstream.subscription` | scoped list-change signal from one live upstream subscription |
| `upstream.notification_lag` | bounded authoritative catalog reconciliation after the subscription receiver skipped events |
| `coalesced` | several emitters converged on one net change; see the `catalog.notify.flush` event for the contributors |
| `unknown` | unattributed — means a new emitter shipped without a label |

Adding or renaming a label is a change to this table in the same commit.

**The fanout is per peer, not a broadcast.** `tools/list_changed` is a claim
about one session's tool list, and two sessions can hold different contracts
over the same gateway state — `McpRouteScope` restricts which upstreams and
services a route exposes, and a protected route may set
`expose_code_mode = false`, which shows that session raw upstream tools while
everyone else sees the constant `codemode` tool. So `tools_changed` reaching
`notify_catalog_peers` is a **hint** ("something happened that could move a tool
list"), and the verdict is computed per peer by re-deriving that peer's
`PeerContract` and comparing it to the contract the peer was last told about.
Resource and prompt signals from a named upstream are filtered by each peer's
route scope. A protected route never receives list-change timing from an
upstream it cannot list or call. Global reconcile signals remain conservative
and reach every accepting peer because the skipped upstream identity is unknown.

When the upstream broadcast receiver reports lag, Labby emits
`action = "catalog.reconcile.start"`, refreshes tools, resources, and prompts
from authoritative upstream state under a 30-second bound, then emits
`action = "catalog.reconcile.finish"` with `outcome`, `skipped`, `elapsed_ms`,
and either `refreshed_tools`, `resource_count`, and `prompt_count`, or
`timeout_ms`. Both success and timeout schedule one coalesced global signal with
`source = "upstream.notification_lag"`: refresh futures may have mutated only
part of the caches before the outer deadline. Timeout uses
`outcome = "timeout_partial_unknown"` and `convergence_scheduled = true` so an
operator can distinguish bounded partial recovery from a clean refresh.

A trigger that moves nobody's contract emits `action = "catalog.notify.skipped"`
at `DEBUG` and is **not** counted as a notification — the healthy outcome for
raw upstream churn under Code Mode.

**Required fields on `action = "catalog.notify"`** (`surface = "mcp"`,
`service = "peers"`):

| Field | Meaning |
|---|---|
| `source` | emitting site, from the table above |
| `peer_count` | connected peers considered |
| `peers_notified` | peers whose contract actually moved |
| `peers_skipped` | peers left alone because their contract was unchanged |
| `notify_total` | notifications since process start |
| `since_last_ms` | gap since the previous notification; absent for the first |
| `window_count` / `window_secs` | notifications within the recent window |
| `in_flight_tool_calls` | tool calls open at emission time |
| `during_tool_call` | `in_flight_tool_calls > 0` |

`during_tool_call = true` is the field that matters: the notification landed
while a caller's turn was open, so it can invalidate a binding that caller is
using. It is the difference between catalog movement and the flapping clients
actually feel.

**Notifications are coalesced and never delivered mid-turn.** Emitters call
`catalog_coalesce::schedule_catalog_notification` rather than the fanout
directly. A trigger starts a settle window (restarted by each new trigger), so
a burst — a reload plus its follow-on enrichment and per-call triggers — is
delivered as one notification instead of one per trigger. The flush then waits
for in-flight tool calls to drain, because a notification delivered while a
call is open invalidates the binding that call is using; that is the failure
clients report, and it leaves no trace on the dispatch path because the call
dies before reaching Labby. Deferral is bounded by `max_hold` — a late
notification is a nuisance, a lost one is a bug — and a flush forced by that
bound logs a non-zero `in_flight_tool_calls`.

The batch is logged at `DEBUG` as `action = "catalog.notify.flush"` with
`sources` (every contributing emitter, not just the last), `source_count`, and
`deferred_for_calls_ms`. When more than one emitter contributed, the fanout's
`source` field becomes `coalesced` and the flush event is where the real
attribution lives. What is finally sent is recomputed per peer at flush time,
so the delivered notification reflects settled state, never a stale
intermediate.

Code Mode's synthetic descriptors include enabled, route-scoped upstream names
and normalized operator hints from gateway configuration. A name, hint, enabled
state, or route-scope change is a real descriptor change and must produce a
`tools/list_changed` notification. Runtime health and discovered-tool churn do
not change the host-cached `codemode`, `codemode_read`, or `codemode_ui`
descriptors by themselves. Reconcile logs therefore separate namespace
determinants from suppressed raw-tool churn. Final Code Mode responses are also
capped at the documented byte budget after trace composition, so truncation is
deterministic and visible instead of surfacing as a client transport failure.
Code Mode result-ack reserve activation logs once per execution at DEBUG as
`action = "codemode.result_ack.reserve"`, `event = "armed"`, with `reserve_ms`
and monotonic `result_ack_reserve_use_count`. Only a genuine full settlement
grace expiry logs at WARN as `action = "codemode.settlement"`,
`event = "watchdog_expired"`, with `settlement_watchdog_expiry_count`; an outer
execution timeout must not increment or masquerade as that watchdog signal.

- `LABBY_MCP_CATALOG_COALESCE_MS` — settle window (default `250`, clamped 1–10000)
- `LABBY_MCP_CATALOG_MAX_HOLD_MS` — total deferral bound (default `5000`, clamped 100–120000)

**Churn is a `WARN`, not an inference.** When the window count reaches the
threshold, the fanout also emits `action = "catalog.notify.churn"` at `WARN`
with the same fields plus `threshold`. Operators should not have to count
`INFO` lines to notice a burst. Both knobs are env-tunable, read once per
process:

- `LABBY_MCP_CATALOG_CHURN_WINDOW_SECS` — window length (default `60`, clamped to 5–3600)
- `LABBY_MCP_CATALOG_CHURN_THRESHOLD` — notifications per window that count as churn (default `4`, minimum 2)

**Gateway reconcile must report what moved and what it withheld.** The
`event = "catalog.refresh.finish"` log on both reconcile paths carries, beyond
the existing counts:

| Field | Meaning |
|---|---|
| `projection` | `code_mode_visible` or `raw` — which contract the diff measured |
| `tools_added` / `tools_removed` | changed tool names, capped at 20 per list |
| `delta_truncated_count` | names dropped by the cap |
| `raw_tools_changed` | whether the raw upstream tool set moved |
| `suppressed_raw_churn` | raw set moved but the visible contract did not — a notification correctly withheld |
| `suppressed_raw_churn_total` | process-lifetime count of the above |

`suppressed_raw_churn` exists because a working filter is otherwise invisible: a
quiet log looks identical whether nothing happened or everything was correctly
filtered. A climbing `suppressed_raw_churn_total` is the healthy signal that raw
upstreams are flapping and clients are being shielded from it.

**Diagnosing reported flapping:**

1. Filter for `action = "catalog.notify"` and group by `source` — that names the
   emitting site. `peers_notified` vs `peers_skipped` shows how many connected
   MCP peers actually received the notification.
2. Check `during_tool_call` on those events. `true` means bindings were
   invalidated mid-turn, which is the reported symptom rather than a correlate.
3. Check `suppressed_raw_churn_total` on the reconcile logs. Climbing means raw
   upstream churn is being filtered correctly; flat while notifications continue
   means the churn is a genuine visible-contract change.
4. `since_last_ms` and `window_count` bound how fast it is happening.

Notification field values include upstream-controlled tool names, so they are
subject to the sanitization rule in **Redaction Rules** below. Code Mode
namespace names and hints are not part of the host-facing descriptor snapshot
and therefore do not appear in these catalog delta fields.

## Required Fields

### Dispatch Events

All dispatch events must include:

- `surface`
- `service`
- `action`
- `elapsed_ms`

Failure events must also include:

- `kind`

Additional fields when applicable:

- `instance`
- `request_id`
- `operation`
- `upstream`
- `capability`

### Request Events

All `HttpClient` request events must include:

- `method`
- `path`
- `host`

`request.finish` must also include:

- `status`
- `elapsed_ms`

`request.error` must also include:

- `elapsed_ms`
- `kind`
- `message`

If the implementation logs a URL, it must be redacted and must not contain secrets or embedded credentials.

## Correlation Rules

Caller context must flow downward.

Rules:

- CLI spans must wrap SDK calls
- MCP spans must wrap SDK calls
- HTTP spans must wrap SDK calls
- `HttpClient` request events must inherit those spans rather than creating detached logs

The practical result must be:

- outbound request logs can be tied back to the invoking surface
- HTTP-originated requests can be tied back to a `request_id`
- multi-instance requests can be tied back to an `instance`
- outbound RMCP proxy activity can be tied back to the invoking surface and
  request when one exists

For device-runtime uploads, operators must be able to correlate:

- the non-master startup or flush attempt
- the outbound request to the master
- the master-side device ingest handler

## Error Classification

The public error taxonomy remains the stable contract.

Relevant kinds include:

- `auth_failed`
- `not_found`
- `rate_limited`
- `validation_failed`
- `network_error`
- `server_error`
- `decode_error`
- `internal_error`

Dispatch layers may also emit:

- `unknown_action`
- `unknown_subaction`
- `missing_param`
- `invalid_param`
- `unknown_instance`

Transport failures must preserve enough message detail to distinguish likely classes such as:

- DNS resolution failure
- TCP connection failure
- TLS certificate validation failure
- timeout

Those details may live in the error message while still mapping to the stable `network_error` kind.

## Redaction Rules

The following data must never be logged:

- API keys
- bearer tokens
- passwords
- cookies
- authorization headers
- secret env values

Additional rules:

- do not log full request headers unless explicitly sanitized
- do not log request bodies by default
- do not log query parameters when they contain secrets
- do not echo secrets in doctor output, prompts, logs, generated docs, or UI flows
- do not log raw discovered MCP config file contents; only metadata such as path, source, and hash are acceptable
- do not persist bearer tokens, cookies, authorization headers, or raw secret material in the local log store
- do not fan out unredacted structured fields to live SSE subscribers
- upstream-controlled field values (tool names, prompt names, resource URIs from external MCP servers)
  must be sanitized before rendering in human log output — strip Unicode control characters except
  tab and newline to prevent ANSI escape injection. `sanitize_field_value()` in
  `log_fmt/formatter.rs` is the canonical implementation; apply it before any terminal styling.
- `resource_uri` field values must have query strings and fragments stripped before logging
  (`redact_resource_uri_for_logging()` in `dispatch/upstream/pool.rs`). Pre-signed S3 tokens,
  OAuth params, and similar credential-bearing query parameters must not appear in log output.
- upstream URL values must have userinfo (username:password) stripped before logging
  (`upstream_target_redacted()` in `dispatch/upstream/pool.rs`).

Shell wrapper boundary: the user-installed `lab` shell wrapper emits CLI-PREFLIGHT output via `printf` to
stderr before the Rust binary starts. This output is pre-binary and therefore not processed by
`init_tracing()` or any redaction rules. Treat it as an unstructured stderr boundary — it must not emit credential-bearing content.

### Upstream OAuth Redaction

The outbound upstream OAuth flow (see [UPSTREAM.md](../services/UPSTREAM.md)) adds the following fields to the never-log list. They must not appear at any level, in dispatch events, request logs, tracing spans, error messages, or MCP notifications:

- OAuth `code` (authorization code from the callback)
- OAuth `state` (CSRF token)
- PKCE `code_verifier`
- `access_token`, `refresh_token`, and `id_token` from any token response
- the raw `token_response_json` payload
- `token_blob` ciphertext and `token_blob_nonce`
- `client_secret` (from the `*_CLIENT_SECRET` env var named by `client_secret_env`)
- `Authorization` headers constructed from upstream OAuth tokens
- `LABBY_OAUTH_ENCRYPTION_KEY`

Credential and state row types implement `Debug` manually to enforce this; never `#[derive(Debug)]` on them.

### Central Google Credential Broker

Shared Google credential lifecycle events should include `upstream`,
`credential_source`, `provider_generation`, fingerprinted `subject_id`, scope
counts, stable `kind`, invalidation counts, and `elapsed_ms` when applicable.

They must never include the raw Google `sub`, verified email, configured account
selector, access token, refresh token, ID token, authorization URL, OAuth code,
PKCE verifier, or client secret. Scope names are configuration metadata and may
be emitted for a targeted diagnostic event, but routine events should prefer
`required_scope_count`, `granted_scope_count`, and `missing_scope_count`.

A terminal refresh failure must log whether compare-and-delete invalidation
succeeded and the counts of dependent Labby refresh tokens and authorization
codes revoked. It must not log the deleted row. Explicit shared revocation must
log the same count-only audit shape.

Credential replacement, proactive refresh, clear, and shared-provider
revocation must also emit `action = "session.invalidate"` after the live
gateway runtime has been invalidated. Required fields are `upstream`, a bounded
non-secret `reason`, `subject_connections`, `relay_connections`, `task_routes`,
`generic_connections`, and `invalidated_total`. This event must never contain the raw OAuth subject;
when caller correlation is available at the surface, use only its bound
`actor_key`. A successful credential mutation is not complete until every
matching initialized MCP peer and task-retained peer has been closed.

## Level Rules

Use these level conventions consistently:

- `INFO` for successful dispatch and successful request completion
- `WARN` for expected caller or service failures such as validation, auth, or not found
- `ERROR` for unhandled or internal failures

Do not use ad hoc `println!` debugging in place of structured logs.

## Verification Requirements

A service is not considered online until observability is verified.

Minimum verification:

1. one successful action shows a dispatch event and downstream request events
2. one failing action shows a dispatch failure with a stable `kind`
3. the failing path preserves enough transport or response detail to diagnose the class of failure
4. logs do not expose secrets

Verification may use:

- unit tests for shared helpers
- mock-server tests for request behavior
- live read-only smoke tests against a real service when available

Destructive actions do not need live verification by default, but their intent and outcome logging must follow the same contract.

## Onboarding Gate

When bringing a new service online, observability is required before the service is complete.

That means the service must have:

- dispatch logging at every public surface it exposes
- shared `HttpClient` request logging for its outbound calls
- correct error kind mapping
- redaction compliance
- verification evidence that the request path is traceable end to end

If those conditions are missing, the service is not fully online even if the CLI, MCP, or HTTP action itself works.

## Example Shapes

Illustrative success fields:

```json
{
  "surface": "api",
  "service": "gateway",
  "action": "gateway.list",
  "request_id": "req-123",
  "method": "GET",
  "path": "/v1/gateway/upstreams",
  "host": "lab.example.com",
  "status": 200,
  "elapsed_ms": 42
}
```

Illustrative failure fields:

```json
{
  "surface": "cli",
  "service": "gateway",
  "action": "gateway.list",
  "method": "GET",
  "path": "/v1/gateway/upstreams",
  "host": "lab.example.com",
  "kind": "network_error",
  "message": "gateway request failed",
  "elapsed_ms": 311
}
```

## Skills (SEP-2640)

Skills events follow the standard dispatch field set. Two rules are specific to
this surface:

- **URIs are redacted before they reach a log line.** A skill URI is
  path-shaped, exactly like a resource URI, and goes through
  `redact_resource_uri_for_logging` at every site. Skill *content* is never
  logged at any level — only bounded identifiers (URI, name, digest, counts).
- **Ingest exclusions are `WARN` with a stable reason code**
  (`invalid_frontmatter`, `manifest_uri_out_of_namespace`, `missing_manifest`,
  …) plus the upstream and the redacted skill URI. One malformed skill never
  fails an upstream, so the log line is the only place the *cause* is visible;
  the count reaches operators through `gateway.skills.list` and agents through
  the listing's `_meta.excludedSkills`.

| Event | Level | Notes |
|-------|-------|-------|
| upstream skills discovery start/finish | `INFO` | per upstream, with count and `elapsed_ms` |
| skill excluded at ingest | `WARN` | reason code + redacted URI |
| snapshot truncated by a budget | `WARN` | which cap engaged |
| `skills/list`, `skills/get`, skill `resources/read` | `INFO` | one dispatch event each |
