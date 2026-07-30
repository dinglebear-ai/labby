---
title: "MCP Surface"
created: "2026-07-30"
updated: "2026-07-30"
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

## Gateway And Code Mode

Without Code Mode, eligible upstream tools are projected into the downstream
catalog subject to route scopes and exposure filters. With Code Mode enabled,
raw upstream tools are hidden from normal `tools/list` and the synthetic
`codemode` tool provides catalog search, description, snippets, and sandboxed
execution.

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

## Supported Product Boundary

The MCP server does not expose ACP, Marketplace, Registry-browser, Fleet/node,
Deploy-product, or Stash tools. Historical contracts are preserved only under
[../references/retired-labby](../references/retired-labby/).
