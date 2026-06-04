# Spec: Gateway Schema Resources

Status: draft
Owner: lab gateway
Surfaces: MCP, HTTP API

## Motivation

The gateway proxies many upstream MCP servers behind two synthetic tools
(`code_mode`, `tool_execute`). Today, an agent that already knows which
upstream it wants must still issue a `code_mode` round-trip to recover
the tool's input schema before calling `tool_execute`.

Exposing each upstream's cached tool catalog as an MCP **resource** removes
that round-trip without bloating the default tool list. Resources are pull-
on-demand and cacheable on the client, which matches the access pattern.

## Goals

- Let an agent list connected upstreams in one read.
- Let an agent fetch the full tool catalog (name, description, input schema)
  for one upstream in one read.
- Honor the existing per-upstream `ToolExposurePolicy` — hidden tools never
  appear in the synthetic schema.
- Keep the default `tools/list` and `code_mode` flow unchanged.

## Non-goals

- Aggregating prompts or resources from upstreams into the synthetic doc.
  Upstream prompts and resources continue to surface through their existing
  proxy paths (`lab://upstream/<name>/...`, `prompts/*`).
- Live invalidation on upstream-side changes. Staleness is bounded by the
  pool's existing discovery + reprobe cadence.
- New caching layer. The pool already caches `input_schema` per tool on
  `UpstreamEntry`.

## URI scheme

A new `lab://gateway/` namespace, owned by lab (synthetic), distinct from
the existing `lab://upstream/` namespace (proxied content owned by the
upstream itself).

| URI                              | Content                                   |
|----------------------------------|-------------------------------------------|
| `lab://gateway/servers`          | Index of connected upstreams              |
| `lab://gateway/<name>/schema`    | Flattened tool catalog for one upstream   |

Adding a sibling `lab://gateway/<name>/resources` index is allowed in a
follow-up but is out of scope for this spec.

## Behavior

### `lab://gateway/servers`

Read returns a JSON document listing every upstream currently registered
with the pool, regardless of health. Each entry reports the exposure-
filtered tool count (i.e. the count an agent would see in the corresponding
schema doc), the cached prompt and resource counts, and the current tools-
capability health string.

### `lab://gateway/<name>/schema`

Read returns a JSON document containing only tools whose names match the
upstream's `ToolExposurePolicy`. For each tool the document includes the
tool name, description (when set by the upstream), and the cached MCP
input schema as supplied by `tools/list`.

If `<name>` is not a registered upstream, the read fails with
`not_found` (MCP `resource_not_found` / HTTP 404).

If the upstream pool is not configured at all (no `current_upstream_pool`),
all `lab://gateway/*` reads fail with `not_found` rather than a generic
internal error.

### Listing

`list_resources` is extended to include the synthetic entries above. The
servers index is always listed when the pool is present. One schema entry
is listed per registered upstream, irrespective of tool-capability health,
so that an agent can still inspect an unhealthy upstream's last-known
schema.

### Auth and visibility

- MCP: the existing per-subject visibility filters apply. The gateway
  schema doc never includes a tool the subject could not call.
- HTTP: the mirrored endpoints (see contract) run under the standard
  `/v1/*` auth middleware. There is no separate public surface.

## Resolved design decisions

- **Health fields in the servers index:** `tool_health` plus optional
  `tool_last_error` for triage. Other capability buckets (prompts,
  resources) are not surfaced in v1 to keep the doc small; they can be
  inspected via existing `gateway.discovered_*` actions.
- **`_meta` passthrough on tool entries:** verbatim. The gateway does not
  strip or rewrite `_meta` — agents see the upstream's own annotations
  (UI hints, deprecation flags, etc.).
- **Architecture test:** an integration test pins the `lab://gateway/*`
  URI scheme, document shape, and exposure-policy filtering as part of
  this work.

## HTTP surface shape

The HTTP mirror uses the existing action-dispatched gateway route
(`POST /v1/gateway`), not bespoke REST endpoints. Two new actions:

- `gateway.servers` — no params; returns the servers index document.
- `gateway.schema` — `{ "params": { "name": "<upstream>" } }`; returns
  the per-upstream schema document.

This keeps parity with the rest of the gateway surface and avoids adding
a second mounting path for the same data.

## Out-of-scope follow-ups

- `lab://gateway/<name>/resources` synthetic index of proxied resources.
- ETag / `If-None-Match` support on the HTTP mirror.
- Structured staleness metadata (`discovered_at`, `last_reprobed_at`).
