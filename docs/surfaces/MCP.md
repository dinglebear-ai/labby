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

## Agent Skills (SEP-2640)

Labby implements the draft MCP Skills extension behind the `skills` cargo
feature. The pinned draft revision, URI grammar, and verification requirements
live in [`docs/contracts/skills-extension.md`](../contracts/skills-extension.md),
which is also published in-band as the `lab://contracts/skills-extension`
resource so a client that does not speak the extension can still discover it.

Labby declares `io.modelcontextprotocol/skills` with an empty settings object —
supported, with no optional features. It does **not** declare `directoryRead`,
and a client must not call `resources/directory/read` against it.

### Methods

| Method | Behavior |
|--------|----------|
| `skills/list` | First-party skills plus every enabled, skills-proxying upstream the caller's route can reach |
| `skills/get` | One entry by URI. `-32602` means the URI is not a skill this server serves |
| `resources/read` | Serves `skill://` files. Skill URIs do **not** appear in `resources/list`; the manifest is the discovery surface |

### Authorization

`skills/list`, `skills/get`, and `skill://` reads require the same scope as
listing resources or prompts (`lab:read` and up) — **not** admin. Agents are the
intended consumers. The operator-facing `gateway.skills.list` action is separate
and does require admin, because it reports configuration state (which upstreams
opted in, what was excluded and why) rather than skill content.

Skills methods inherit the same per-client throttling posture as every other MCP
method. Labby has no generic MCP rate limiter, so they are not specially
protected — nor specially exposed.

### Operator-provided skills

Drop a skill directory into `$LABBY_HOME/skills/<name>/` and it is served under
the same reserved `labby` origin as the bundled ones — one first-party namespace
from a client's point of view. A bundled skill wins a name collision, so a
dropped-in directory cannot redefine what an existing `skill://labby/…` URI
means.

The tree is read **once at startup**, and each file's digest is computed from
the bytes read in that same pass. Adding or editing a skill therefore needs a
restart. That is deliberate: re-reading per request would let a file change
between publishing a digest and serving the file it describes, which is exactly
the mismatch a conforming client must refuse.

A skill is skipped, with a logged reason, when it contains a symlink at any
depth (the target could sit outside the root and would be served as first-party
content), has no `SKILL.md`, has a directory name disagreeing with its
frontmatter `name`, holds a file over 1 MiB, or holds more files than the
per-skill manifest cap. One bad directory never costs an operator their other
skills.

### Origin namespacing

Proxied skills are relabelled as `skill://<upstream-name>/…`. The label is
host-assigned, which is what the threat model requires: a skill from one origin
must never shadow a same-named skill from another. Nothing is deduplicated by
name — names are labels, not identifiers, and one server may legitimately serve
two skills sharing a final segment. `labby` is reserved for first-party skills
and an upstream with `proxy_skills` enabled may not claim it.

### Exposure and the manifest-bound read gate

A proxied skill is visible when the upstream sets `proxy_skills` (opt-in,
unlike `proxy_resources`/`proxy_prompts`), the skill passes `expose_skills`, and
the caller's route may reach that upstream.

Skill-file reads are **manifest-bound**: a read is granted because the URI
appears in a verified skill manifest, not because `expose_resources` allows it.
The two gates are independent — `expose_resources` neither grants nor blocks a
skill-file read, and a skill manifest cannot make an ordinary upstream resource
readable.

### What digest verification does not prove

The SEP is explicit that digests are unsigned, come from the same server as the
content, and that *"[a]ny intermediary on the path, such as a gateway, can
rewrite both the listing and the content together. Hosts MUST NOT treat a digest
match as a security boundary."*

Labby is exactly that intermediary. Verification here is a **consistency check**
— it catches corruption, truncation, and staleness after a skill is updated. It
is not tamper detection. A digest match proves the bytes are the ones the entry
described; it proves nothing about whether those bytes are safe.

### `allowed-tools` through a gateway

A skill's `allowed-tools` frontmatter names tools in its *origin's* namespace.
Downstream of Labby the catalog is aggregated, so those names could otherwise
resolve against a different server's tools — or against Labby's own privileged
ones.

Every aggregated entry therefore carries an `_meta` block under
`ai.dinglebear.labby/skillOrigin` describing what the origin actually accounts
for downstream:

| Field | Meaning |
|-------|---------|
| `label` | The host-assigned origin label, also the first URI segment |
| `toolAccess` | `direct` or `code_mode_only` |
| `reachableTools` | Present only when `direct`: the downstream tool names this origin accounts for |
| `note` | Present only when `code_mode_only`: why there is nothing to scope against |

Under Code Mode, raw upstream tools are hidden from `tools/list`, so
`reachableTools` is deliberately **omitted** rather than emitted empty —
publishing downstream names that do not exist would be a worse answer than
saying so plainly.

This lives in `_meta`, never in `frontmatter`. The SEP requires frontmatter to
be the author's YAML verbatim and requires a host to refuse the skill on any
field-by-field discrepancy against the fetched `SKILL.md`, so anything Labby
adds has to sit outside it.

Every value here is a fact about Labby's own catalog, never an interpretation of
skill content — the skill remains data, not directives.

### Enabling skills proxying is a trust decision

`proxy_skills` is flipped through `gateway.update`, which is `destructive: false`
and therefore not elicitation-gated the way `gateway.remove` is. Config mutation
is reversible and backup-first, so that classification stands — but enabling
skills aggregation means an upstream's instructions reach agents through Labby.
`gateway.skills.list` shows which upstreams have it on, along with each
catalog's cache age and what was excluded or truncated.
