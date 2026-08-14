---
title: "Tool Annotations Contract"
created: "2026-08-05"
updated: "2026-08-13"
---

# Contract — Tool Annotations

The externally observable promise. Anything here is a breaking change to alter
without a deliberate decision; see [SPEC.md](SPEC.md) for rationale and
[SCHEMAS.md](SCHEMAS.md) for the wire shape.

Protocol: MCP `2026-07-28` · SDK: `rmcp =3.1.0` (exact pin)

## C1. Labby annotates every tool it owns

Every tool Labby constructs itself carries an `annotations` object with all four
hint fields present. No Labby-owned tool emits a partial annotations block, and
none omits the block entirely.

`ToolAnnotations.title` is **not** set. Tool display names remain governed by
`name` and `description`.

Normative values: [SPEC.md § Decision table](SPEC.md#6-decision-table-normative).

## C2. `destructiveHint` tracks the action catalog

For the seven service tools, `destructiveHint` is computed as
"any action in this service is destructive", read from the same
`ActionSpec.destructive` flags that drive MRTR elicitation and CLI confirmation.

**Guarantee:** a service tool advertises `destructiveHint: true` if and only if
at least one of its actions would trigger a destructive gate. Adding a
destructive action to a service flips its hint automatically — there is no
second place to update.

## C3. Tool-level hints are the least-safe union of a service's actions

A single MCP tool fronts a whole service. `destructiveHint: true` therefore means
*"at least one action of this tool is destructive"*, **not** *"this call is
destructive"*.

Clients must not infer that a specific `action` is destructive from the tool's
hint.

**Where the per-action truth actually is — and where it isn't.** For the **seven
service tools** (`fs`, `server_logs`, `lab_admin`, `doctor`, `snippets`, `setup`,
`gateway`) it is reachable two ways, in order of availability:

1. `tools/call` with `{"action": "help"}` — always reachable, returns per-action
   `destructive` (`dispatch/helpers.rs:201`), and the shared input schema already
   advertises it. Prefer this: MCP resources are an optional client capability
   and many agent clients implement tools only.
2. The `lab://<service>/actions` and `lab://catalog` resources.

For the **five meta tools** — `codemode`, `codemode_ui`, `mcp_app`, `add_server`,
`gateway_status` — **neither exists.** They are constructed directly in
`handlers_tools.rs`, appear in no catalog, and have no `help` action (built-in
`help`/`schema` interception runs only for registry services). For those five the
coarse tool-level hint is all a client gets. An earlier draft of this contract
promised per-action truth for every tool; that promise was false and is withdrawn.

**Scope note.** Tool visibility and `lab://<service>/actions` are gated by
`route_scope`, **not** by the caller's admin scope. Action *metadata* — names,
descriptions, `requires_admin` — crosses that boundary even though action
*execution* does not. Operators splitting routes for confidentiality should know
this. Per-action detail must stay behind that opt-in resource read; it must not
be hoisted into tool `_meta`, which would put the full admin-action inventory
(61 of 64 for `gateway` alone) into every `tools/list` response.

## C4. Upstream annotations pass through verbatim

For any tool proxied from an upstream MCP server, Labby forwards the upstream's
`annotations` object **unchanged** — including `title`, including unknown or
future fields, and including the absence of the block.

Labby specifically does **not**:

- fill in missing hints with its own judgment,
- overwrite an upstream hint it disagrees with,
- strip hints it does not understand,
- or namespace/rewrite the tool while copying it.

This holds on every listing path: the raw aggregated path, the subject-scoped
OAuth path, the peer-contract path that feeds the hash, and through nested
gateways (labby proxying labby).

**Trust note.** Upstream hints are attacker-controlled data from Labby's
perspective. rmcp says it plainly (`rmcp-3.1.0/src/model/tool.rs:44-49`): clients
must never make tool-use decisions based on annotations from untrusted servers.
Labby forwards them for presentation; it does not vouch for them.

## C5. Hints are advisory to clients — and consumed by Labby at the next hop

This is the clause most likely to be mis-stated. The correct framing:

> Advisory to clients; and at the next hop in a labby → labby chain, an **input
> to an authorization decision** — not merely a confirmation prompt.

Do **not** write "advisory only", and do **not** soften this to "relaxes
elicitation": `UpstreamTool.destructive` gates a hard `forbidden` in Code Mode
(`code_mode_host.rs:90-107`) and the palette (`palette.rs:235-247`), in addition
to MRTR. A `lab:read` caller currently forbidden from a proxied Labby builtin
becomes able to call it once that tool is annotated non-destructive. See
[SPEC § 7 F9](SPEC.md#f9--the-reach-is-authorization-not-just-confirmation-gating-unknown).

Concretely, a downstream Labby gateway derives its destructive gate for a
proxied tool from that tool's annotations (`cached_upstream_tool`,
`crates/labby-gateway/src/upstream/pool/helpers.rs:423`). Because Labby now
annotates its own tools, a Labby proxied behind another Labby will have its six
non-destructive tools (`fs`, `lab_admin`, `gateway_status`, `doctor`, `mcp_app`)
treated as non-destructive downstream — where previously, being unannotated, they
all failed closed to destructive. `server_logs` is deliberately excluded from that
set by the override in [SPEC § 6](SPEC.md#6-decision-table-normative).

Authorization sources of truth are **unchanged**:

| Decision | Source of truth |
|---|---|
| Local elicitation for a Labby action | `ActionSpec.destructive` |
| Gate for a proxied upstream tool | `cached_upstream_tool` fail-closed derivation |
| Client-side pre-warning / UI rendering | `ToolAnnotations` (this contract) |

## C6. Determinism

For **Labby-owned** tools, annotations are a pure function of static catalogs.
For a given build, the annotation block for a given tool is identical across
peers, sessions, requests, and wall-clock time. Verified: no `PeerCatalogAudience`,
`auth`, `route_scope`, or `oauth_subject` reaches the policy functions —
`PeerCatalogAudience` gates only *whether* a tool appears, never what its
annotation values say.

The claim is scoped to Labby-owned tools. Annotations on **proxied** tools are
whatever the upstream returns, and the subject-scoped OAuth path
(`pool/tools.rs:246-274`) legitimately fetches per-identity tool lists — so an
upstream may serve different annotations to different subjects. That is C4
passthrough behavior, not a determinism violation.

This is a hard requirement, not a quality goal: annotations are hashed into the
peer contract (`crates/labby/src/mcp/catalog.rs:125-143`). Keep the derivation a
pure function of its `&RegisteredService` argument — in particular, do **not**
memoize it in a process-global `LazyLock`/`OnceLock` keyed by service name.
`build_default_registry`, `build_docs_registry`, and test registries produce
different service sets, so a global cache would leak one registry's answer into
another's.

## C7. Compatibility and migration

| Aspect | Impact |
|---|---|
| Wire compatibility | **Additive.** `annotations` is an optional object; clients that ignore it are unaffected. |
| Tool names, schemas, descriptions | Unchanged. |
| Error envelopes | Unchanged. |
| Peer contract hash | Value changes with the binary, but **no `tools/list_changed` is emitted**. Peers seed `last_contract` at registration, and a binary change implies a restart that destroys every session — so reconnecting peers seed from post-change descriptors and see no diff. |
| Next-hop reach | Widened for the five remaining non-destructive builtins (C5) — an authorization change, not a prompt change. Gated on F9. |
| Downgrade | Reverting restores the pre-change hash and re-tightens next-hop gating. No persisted state involved. |

## C8. Unknown / future services

A service registered without a row in the hint table advertises the least-safe
shape — `readOnly: false, destructive: true, idempotent: false, openWorld: true`.

Fail-closed by construction: forgetting to add a table row degrades presentation
(an over-warning client) rather than making a mutating tool look safe.
