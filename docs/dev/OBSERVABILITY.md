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
- `lab-apis` owns outbound request logging and transport failure detail

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

`lab-apis::core::HttpClient` must emit:

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

The 2026-07-28 MCP surface does not advertise the removed legacy logging
capability. Required observability is emitted through local structured tracing,
not `logging/setLevel` or `notifications/message`.

### Gateway usage telemetry (`UsageStore`)

Every upstream tool/resource/prompt call outcome recorded by `upstream.request.finish`/`upstream.request.error` (above) is also durably persisted to a small SQLite store at `~/.labby/usage.db`, via `UpstreamPool`'s `timed_capability_call` choke point (`crates/labby-gateway/src/upstream/pool/capability_call.rs`). This is a fire-and-forget write (`tokio::spawn`) — it never adds latency or failure risk to the call it's observing, and a write failure is logged (`usage store record_call failed`) and dropped, never surfaced to the caller.

Query it via the `gateway.usage.metrics` (aggregated totals/top-tools/top-actors) and `gateway.usage.calls` (raw paginated records) actions — both admin-gated, same as `gateway.enrich.*`. CLI: `labby gateway usage metrics` / `labby gateway usage calls`. Both actions enforce the same route-scope restriction as `gateway.enrich.*` — a route-scoped caller only sees usage data for the upstreams visible on their route.

Set `LABBY_GATEWAY_USAGE_DISABLED=1` to disable capture entirely (no store is opened at startup). Retained rows are pruned on a 6-hour cycle to a 30-day retention window; `labby serve` starts the loop but the batched deletion logic (`UsageStore::spawn_prune_loop`/`prune_older_than`, deleting up to 5,000 rows per statement so a large backlog never holds SQLite's writer lock for long) lives entirely in `UsageStore`.

In-flight fire-and-forget writes are capped by a semaphore (`WRITE_SEMAPHORE_PERMITS`, 64 permits) — a saturated burst drops the write and logs a warning rather than queuing unboundedly or spawning an unbounded number of tasks. `~/.labby/usage.db` is created with owner-only (`0600`) permissions since `actor` is a stable per-user identifier, even though nothing in the store is a credential.

This store intentionally does not capture CLI/HTTP/MCP dispatch-level events for the `gateway` service's own actions (e.g. `gateway.add`, `gateway.enrich.preview`) — only calls proxied through to upstreams. The recorded schema is intentionally minimal: `ts_unix`, `upstream_name`, `tool_name`, `actor`, `outcome`, `elapsed_ms` (see `crates/labby-gateway/src/usage/types.rs`). See `docs/superpowers/plans/2026-07-09-gateway-usage-telemetry.md` for the original design rationale — note the shipped schema diverges from that plan (the `capability`/`operation`/`subject_scoped`/`error_kind`/`response_bytes` fields proposed there were dropped during review as unused).

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

**Every emission must be attributed.** `notify_catalog_peers` takes a `source`
label from `labby_runtime::catalog_notify`, which is the single vocabulary
shared by the gateway and MCP crates:

| `source` | Emitted by |
|---|---|
| `gateway.reload.selective` | reconcile that kept the live pool and selectively reconciled added upstreams |
| `gateway.reload.full` | reconcile that rebuilt the upstream pool |
| `gateway.enrich.hint_apply` | `gateway.enrich.hint.apply` writing a `code_mode_hint` |
| `mcp.call.codemode` | post-run catalog delta observed by a `codemode` call |
| `mcp.call.upstream` | post-call catalog delta observed by a raw upstream proxy call |
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
Resources and prompts remain global signals and are forwarded unchanged.

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
| `namespaces_added` / `namespaces_removed` | changed Code Mode namespaces, rendered as bare upstream names |
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
subject to the sanitization rule in **Redaction Rules** below; the namespace
sentinel tokens used internally by the reconcile snapshot are decoded to bare
upstream names before logging rather than being emitted raw.

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
  "surface": "http",
  "service": "marketplace",
  "action": "mcp.list",
  "request_id": "req-123",
  "method": "GET",
  "path": "/v0.1/servers",
  "host": "registry.modelcontextprotocol.io",
  "status": 200,
  "elapsed_ms": 42
}
```

Illustrative failure fields:

```json
{
  "surface": "cli",
  "service": "marketplace",
  "action": "mcp.list",
  "method": "GET",
  "path": "/v0.1/servers",
  "host": "registry.modelcontextprotocol.io",
  "kind": "network_error",
  "message": "registry request failed",
  "elapsed_ms": 311
}
```
