---
title: "Contract: Gateway Schema Resources"
created: "2026-07-30"
updated: "2026-07-30"
---

# Contract: Gateway Schema Resources

Status: implemented
Surfaces: MCP, HTTP API
Related: `docs/dev/ERRORS.md`, `docs/design/SERIALIZATION.md`

This contract pins the wire shape of the synthetic `lab://gateway/*`
resources and their HTTP mirror. Any change to field names, field
presence, or status mapping is a contract change and must be reflected
here and in surface code in the same PR.

## URI scheme

```
lab://gateway/servers
lab://gateway/<name>/schema
```

`<name>` matches one registered upstream as keyed in `UpstreamPool::upstreams`.
Names are opaque strings; the gateway does not normalize case or punctuation.

## MCP

### list_resources

When the upstream pool is present, `list_resources` includes:

- One entry for `lab://gateway/servers` with `mime_type: "application/json"`.
- One entry per route-visible registered upstream at `lab://gateway/<name>/schema` with
  `mime_type: "application/json"`.

These entries appear in addition to the existing `lab://catalog`,
`lab://<service>/actions`, and `lab://upstream/...` entries. Order is not
contractual.

### read_resource

A read of `lab://gateway/servers` returns one `ResourceContents::text`
item whose body is the pretty-printed JSON document defined below.

A read of `lab://gateway/<name>/schema` returns the per-upstream JSON
document defined below.

A read of any other `lab://gateway/...` URI, or of `lab://gateway/<name>/schema`
where `<name>` is unknown, returns `ErrorData::resource_not_found`. This
maps to MCP error `RESOURCE_NOT_FOUND`.

If `current_upstream_pool().await` is `None`, all `lab://gateway/*` reads
return `ErrorData::resource_not_found`. Callers must not assume a
distinct error kind for "pool not configured".

## HTTP

Mirrored via the existing action-dispatched gateway route. Both calls
require the `lab:admin` scope, consistent with other gateway actions.

```
POST /v1/gateway
{ "action": "gateway.servers" }

POST /v1/gateway
{ "action": "gateway.schema", "params": { "name": "<upstream>" } }
```

Status codes (assigned by `ToolError::into_response()`):

- `200` — body is the document itself; the action helper does not add a
  `result` wrapper.
- `404` — `kind: "not_found"` when `<name>` is unknown or pool not
  configured.
- `422` — `kind: "missing_param"` when `gateway.schema` is called
  without `params.name`.
- `403` — `kind: "forbidden"` when caller lacks `lab:admin`.

The two new actions appear in `gateway.help` output alongside the
existing `gateway.discovered_*` actions.

Listing and direct reads both enforce route visibility. OAuth schema reads
also require the request's verified OAuth subject and can return typed
authentication, timeout, capacity, or upstream-discovery errors.

## Document shapes

### `lab://gateway/servers` (MCP) / `gateway.servers` (HTTP action)

```jsonc
{
  "servers": [
    {
      "name": "string",          // upstream name
      "tool_count": 0,                  // number of tools the agent will see in the schema doc
      "prompt_count": 0,                // cached count from last successful prompts/list
      "resource_count": 0,              // cached count from last successful resources/list
      "tool_health": "healthy",         // see "Health values" below
      "tool_last_error": "string|null"  // most recent tools-capability failure detail, null when healthy
    }
  ]
}
```

Stability rules:

- `name`, `tool_count`, `tool_health`, and `tool_last_error` are stable
  required fields. `tool_last_error` is `null` (not absent) when the
  upstream has no recorded failure.
- `prompt_count` and `resource_count` are stable required fields; they
  may be `0` when the upstream has never been probed.
- New fields may be added without a breaking change. Removing or
  renaming any of the listed fields is a breaking change.
- Element order is not stable. Callers must key by `name`.

OAuth upstream rows instead have `tool_count: null`,
`tool_health: "not_probed"`, and `discovery_mode: "request_scoped"`.
They do not represent a shared cached OAuth tool catalog; use a subject-scoped
schema read to discover tools.

### `lab://gateway/<name>/schema` (MCP) / `gateway.schema` (HTTP action)

```jsonc
{
  "name": "string",
  "tools": [
    {
      "name": "string",                       // tool name as exposed by the upstream
      "description": "string or null",        // upstream-supplied; absent fields become null
      "input_schema": { /* JSON Schema */ },  // discovered schema; see sanitization below
      "meta": { /* upstream _meta verbatim */ } // null when upstream did not provide _meta
    }
  ],
  "health": "healthy",
  "last_error": "string|null",
  "catalog_source": "shared_cache" // or "subject_scoped_live" for OAuth
}
```

Stability rules:

- `name`, `tools`, `health`, and `last_error` are stable required fields.
- Each tool element has stable required fields `name`, `description`,
  `input_schema`, `meta`. `description` and `meta` are `null` (not
  absent) when the upstream does not provide them.
- `input_schema` and `description` reflect the discovered catalog. Shared
  discovery applies the gateway's presentation sanitization; this is not a
  byte-for-byte copy guarantee. An unavailable schema is represented by `null`.
- `meta` is passed through verbatim from the upstream tool definition's
  `_meta` field (renamed without the leading underscore to comply with
  JSON style). The gateway does not strip, filter, or rewrite its
  contents.
- Tools hidden by the upstream's `ToolExposurePolicy` MUST NOT appear.
- Tool element order is not stable. Callers must key by `name`.

## Health values

`tool_health` and `health` use the following stable string set, derived
from `UpstreamHealth` plus the OAuth index's discovery state:

| String       | Meaning                                                          |
|--------------|------------------------------------------------------------------|
| `"healthy"`  | `UpstreamHealth::Healthy`                                        |
| `"degraded"` | `Unhealthy { consecutive_failures < CIRCUIT_BREAKER_THRESHOLD }` |
| `"open"`     | `Unhealthy { consecutive_failures >= CIRCUIT_BREAKER_THRESHOLD }` |
| `"not_probed"` | OAuth index row; discovery requires a request subject |

New variants may be added without a breaking change. Renaming or removing
any of the above is a breaking change.

## Caching and staleness

Regular upstream schema reads use the shared discovery cache. Its freshness
depends on discovery, enabled reprobes, reloads, and upstream notifications;
there is no unconditional session-long freshness guarantee. OAuth schema
reads perform bounded live discovery using the request's subject and return
`catalog_source: "subject_scoped_live"`.

Clients must keep these documents isolated by caller and route and re-read
after configuration or authorization changes. Do not reuse one subject's
OAuth schema as another subject's catalog.

## Error envelope (HTTP)

HTTP errors use the canonical lab envelope from `docs/dev/ERRORS.md`:

```jsonc
{ "kind": "not_found", "message": "unknown upstream: foo" }
```

The MCP surface uses `ErrorData::resource_not_found` for the equivalent
condition; the two surfaces are not byte-identical here because MCP
error shape is fixed by the protocol.

## Non-contractual

The following are explicitly **not** part of this contract and may change
without notice:

- Pretty-printing of MCP resource bodies.
- The exact log fields emitted by `list_resources` / `read_resource`.
- The internal pool method names that produce these documents.
- Whether the synthetic schema entry for an unhealthy upstream is listed
  or hidden (today: listed; the spec reserves the right to change this).
