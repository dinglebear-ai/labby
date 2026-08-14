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

- **On `tools/list` this is Raw-mode-only.** Builtins are suppressed from
  `tools/list` whenever Code Mode is enabled, so under Code Mode the only
  builtin schema a client sees there is `server_logs`. The `codemode*` tools
  advertise their own execution-trace schema instead. Under Code Mode,
  builtin services instead join the **Code Mode catalog** as in-process
  peers (`__in_process__<service>` namespaces, root scope only), so the
  envelope schema and the callable capability arrive together through
  `codemode.search` / `codemode.describe`.
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
- Upstream tools relay their own `outputSchema` **shape** unchanged and their
  **result payloads** byte-identically. Documentation strings inside those
  schemas (`description`, `title`, `$comment`) are sanitized; schema-semantic
  keywords (`enum`, `const`, `default`, `examples`, `pattern`, `format`,
  `$ref`, property names) are not.

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
2026-07-28 MRTR confirmation flow: the dispatcher returns `input_required` and
validates the answer from the retried request's `inputResponses`.

When the client does **not** support form elicitation, the dispatcher executes
normally. There is no `params.confirm`, `--yes`, or header equivalent on the MCP
path — request params are payload, not authorization. MCP is therefore the one
surface that fails **open** without elicitation support; CLI bails without `-y`
and the palette defaults its confirmation to false.

`ActionSpec.destructive` is the single source of truth for this gate.
Authorization scope and confirmation are separate checks.

## Tool Annotations

Labby forwards each upstream tool's `annotations` object **verbatim** — including
`title`, unknown or future fields, and the absence of the block. It does not fill
in missing hints, overwrite hints it disagrees with, strip fields it does not
understand, or rename the tool while copying it. This holds on every listing path:
the aggregated path, the subject-scoped OAuth path, and through nested gateways.

Upstream hints are attacker-controlled data from Labby's perspective. Per the MCP
spec, clients must not make tool-use decisions based on annotations from untrusted
servers. Labby relays them for presentation; it does not vouch for them.

Independently of what an upstream claims, Labby derives its own fail-closed
`destructive` judgement for gating a proxied tool (`cached_upstream_tool`): a tool
is treated as destructive unless its annotations explicitly say otherwise. That
value never reaches the wire.

Annotations on Labby's **own** tools are implemented by the shared
`PermanentToolRegistry` descriptor builders and specified in
[../design/tool-annotations/](../design/tool-annotations/). Two properties from
that spec matter to clients: a Labby tool fronts
a whole service, so a tool-level hint is the least-safe **union** of that
service's actions and must not be read as a claim about a specific `action`; and
in a labby → labby chain these hints feed the next hop's own gate, so they are
advisory to clients but not inert.

Per-action truth (`destructive`, `requires_admin`) is available for the seven
registered service tools via `{"action": "help"}` or the `lab://<service>/actions`
resource. It is **not** available for `codemode`, `codemode_ui`, `mcp_app`,
`add_server`, or `gateway_status`, which are not registry services.

Note that tool visibility and `lab://<service>/actions` are scoped by
`route_scope`, **not** by the caller's admin scope: action metadata crosses that
boundary even though action execution does not.

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
