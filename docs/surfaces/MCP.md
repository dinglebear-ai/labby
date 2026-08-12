---
title: "MCP Surface"
created: "2026-07-30"
updated: "2026-08-08"
---

# MCP Surface

Labby exposes the supported gateway product through stdio and Streamable HTTP
MCP. The same service dispatch layer backs MCP, CLI, and HTTP.

## Entry Points

- Local stdio: `labby mcp`
- Hosted Streamable HTTP: `labby serve`, endpoint `/mcp`
- Protected MCP routes: route-specific paths configured through the gateway

See [TRANSPORT.md](./TRANSPORT.md) for transport and authentication boundaries.

## Services

The generated [service catalog](../generated/service-catalog.md) is authoritative.
The current services are:

- `gateway`
- `doctor`
- `server_logs`
- `setup`
- `snippets`
- `fs` when the feature is enabled
- `lab_admin` when runtime-enabled

Each service tool accepts:

```json
{
  "action": "service.action",
  "params": {}
}
```

Every service also supports shared `help` and `schema` discovery. Generated
MCP help lives in [../generated/mcp-help.md](../generated/mcp-help.md).

## Tool Results And `outputSchema`

Every builtin service tool returns the dispatch envelope as
`structuredContent` on success, mirrored by one JSON text block:

```json
{ "ok": true, "service": "gateway", "action": "gateway.list", "data": {} }
```

Builtin service tools — plus the `add_server` and `gateway_status` admin app
tools — advertise this envelope as their MCP `outputSchema`. The normative
contract is [mcp-tool-output.md](../contracts/mcp-tool-output.md) and the
published schema is
[dispatch-envelope.schema.json](../contracts/schemas/dispatch-envelope.schema.json);
a drift test binds the runtime schema to the published file. `data` is
deliberately unconstrained: one tool serves many actions, so a tool-level
schema cannot describe per-action payloads.

Scope and caveats:

- **This is Raw-mode-only in practice.** Builtins are suppressed from
  `tools/list` whenever Code Mode is enabled, so under Code Mode the only
  builtin schema a client sees is `server_logs`. The `codemode*` tools
  advertise their own execution-trace schema instead.
- **Error envelopes are outside `outputSchema`.** An `isError: true` result
  carries the `{ "ok": false, … }` error envelope
  ([agent-error-contract.md](../contracts/agent-error-contract.md)). The
  exemption of error results from `outputSchema` conformance is converged
  ecosystem convention, not explicit MCP spec text.
- **No protocol-version gating.** The schema is serialized regardless of the
  negotiated protocol version; older clients ignore the unknown field.
- **`mcp_app` advertises no schema** — its control payload is
  `{"kind": "mcp_app_control", …}`, not the envelope, and an inaccurate
  schema is a hard client-side error in strict SDKs.
- Upstream tools relay their own `outputSchema` (and results) byte-identically;
  only documentation-bearing metadata is sanitized.

## Gateway And Code Mode

Without Code Mode, eligible upstream tools are projected into the downstream
catalog subject to route scopes and exposure filters. With Code Mode enabled,
raw upstream tools are hidden from normal `tools/list`. The synthetic surface
provides two text entry points:

- `codemode_read` is available to `lab:read`, `lab`, and `lab:admin`. It is
  annotated read-only and can discover or invoke only upstream tools whose live
  descriptor explicitly sets `readOnlyHint: true` without a contradictory
  `destructiveHint: true`. Missing or ambiguous annotations fail closed.
- `codemode` is the full execution surface for `lab` and `lab:admin`. The
  optional `codemode_ui` tool has the same execution authority and adds the
  Lab-owned trace inspector.

The full-execution tools are annotated as write-capable and potentially
destructive. Their annotations describe the approval boundary; upstream tool
authorization is still enforced again at dispatch time.

Approval-facing Code Mode descriptors include enabled, route-scoped upstream
names and normalized operator hints. They change when those configuration
determinants change, but remain stable across runtime health and discovered-tool
churn. Call `codemode.search(...)` and `codemode.describe(...)` inside a run to
inspect the current route-scoped tool catalog.

Synthetic Code Mode advertises only the fixed Lab-owned UI action surface. It
does not add or remove raw upstream MCP App tools as upstream health changes.
An upstream widget returned by a Code Mode call may still render through its
resource URI, but its raw callback tools are not added to the approval-facing
`tools/list` contract.

Code Mode may call exposed upstream MCP tools only. Lab actions are not callable
from inside its sandbox. Large upstream results must be projected or sliced
inside the sandbox before return.

## Authentication And Routes

The root administrative MCP endpoint uses the configured bearer or OAuth mode.
Public protected routes validate route-scoped Lab OAuth JWTs and their configured
resource/scope contract. A static operator bearer token is not a public resource
credential.

## Destructive Actions

When the client supports elicitation, destructive service actions use the shared
confirmation flow. Headless callers pass the explicit confirmation field required
by the action contract. Authorization scope and confirmation are separate checks.

## Notifications

Catalog notifications are evaluated against each peer's visible contract,
coalesced, and held until in-flight tool calls drain. Do not restore global
broadcast semantics or notification delivery during an open turn.

`tools/list` assembles the complete visible contract, sorts it globally by tool
name, and then paginates it. Continuation cursors are bound to that contract's
revision; a cursor from a changed catalog is rejected instead of being resumed
at an unsafe offset. A session's notification baseline advances only after it
receives the final page of a complete listing. Subscribing before that point
keeps the baseline unpublished so the next relevant catalog trigger emits
`notifications/tools/list_changed`.

## Supported Product Boundary

The MCP server does not expose ACP, Marketplace, Registry-browser, Fleet/node,
Deploy-product, or Stash tools. Historical contracts are preserved only under
[../references/retired-labby](../references/retired-labby/).
