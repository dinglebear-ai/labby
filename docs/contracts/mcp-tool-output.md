---
title: "Contract: MCP Tool Output Schema and Structured Content"
created: "2026-08-05"
updated: "2026-08-11"
---

# Contract: MCP Tool Output Schema and Structured Content

Status: **active** (implemented on current main)
Surfaces: MCP
Related: [ERRORS.md](../dev/ERRORS.md), [agent-error-contract.md](./agent-error-contract.md), [SERIALIZATION.md](../design/SERIALIZATION.md)

Normative contract for `outputSchema` and `structuredContent` on Labby MCP surfaces, and for
the in-sandbox Code Mode `callTool` boundary. Keywords per RFC 2119.

- **Contract version:** 1
- **MCP revisions:** 2025-06-18 → 2026-07-28 (three revisions; rmcp 3.1.0 additionally
  negotiates 2024-11-05 and 2025-03-26)

---

## C1. Tool classes

| Class | Examples | Result shape owner | `outputSchema` |
|---|---|---|---|
| **Builtin service** | `gateway`, `doctor`, `setup`, `server_logs`, `snippets`, `fs`, `lab_admin` | Labby (`format_dispatch_result`) | Dispatch envelope (C2) |
| **Code Mode** | `codemode`, `codemode_ui` | Labby (`code_mode_execute_trace`) | Trace schema (C4) |
| **Synthetic / app** | `mcp_app`, `add_server`, `gateway_status`, `settings` | Labby, per-tool | Per audit — accurate schema or none |
| **Upstream (proxied)** | any `<upstream>::<tool>` | The upstream server | Relayed (C5) |

> **Visibility caveat.** Under `hide_raw_tools` (whenever Code Mode is enabled), every builtin
> service tool except `server_logs` is suppressed from `tools/list`
> (`handlers_tools.rs:135-137`, `peer_contract.rs:201-203`). The builtin contract below
> therefore applies **only in Raw mode** for all classes but `server_logs`. See SPEC §2.1.

---

## C2. Dispatch envelope

### C2.1 Success

A builtin service tool call that succeeds MUST return a `CallToolResult` with:

- `content[0]` — text block containing the serialized envelope JSON;
- `structuredContent` — the same envelope as a JSON object;
- `isError` — absent or `false`.

```json
{ "ok": true, "service": "<service>", "action": "<dotted action>", "data": <any JSON value> }
```

`build_success` (`crates/labby/src/mcp/envelope.rs:42-49`); attached by `format_dispatch_result`
(`crates/labby/src/mcp/result_format.rs:111-114`).

**Invariants**

- `ok` MUST be `true`; `service` MUST equal the answering tool name; `action` MUST be the
  resolved action, including built-ins (`help`, `schema`).
- The text block and `structuredContent` MUST be serializations of **one** value, built once
  and serialized twice. Never parse text back into structure.
- `structuredContent` MUST be **present** on every success path of a tool that declares
  `outputSchema` — not merely well-shaped. See C3.3.

`data` is unconstrained. Consumers MUST NOT infer a `data` shape from the tool schema.

> **Limitation (was previously stated as a mitigation).** There is currently **no** mechanism
> by which a consumer can learn a specific action's result shape. The `schema` built-in action
> returns `ActionSpec.returns`, a `&'static str` documented as *"not a runtime contract —
> purely informational"* (`crates/labby-primitives/src/action.rs:42-44`), with values such as
> `"DoctorReport"` that resolve to no definition. Earlier drafts of this contract directed
> consumers there with a SHOULD; that guidance was unsatisfiable and has been withdrawn.

### C2.2 Error

A failing call MUST return `isError: true`, the serialized error envelope in `content[0]`, and
the same envelope in `structuredContent`:

```json
{ "ok": false, "service": "<service>", "action": "<action>",
  "error": { "kind": "<stable kind>", "message": "<text>", "…": "recovery contract fields" } }
```

The error envelope is **outside** the advertised `outputSchema` (C3.2). Its `error` object is
owned by `docs/dev/ERRORS.md` and `docs/contracts/agent-error-contract.md`; this contract MUST
NOT diverge from them.

### C2.3 Elicitation

A destructive action awaiting MRTR confirmation returns an `input_required` response — a
distinct variant, not a `CallToolResult` — and is outside this contract. A declined or invalid
retry returns a normal C2.2 error with kind `confirmation_required`.

---

## C3. `outputSchema` advertisement

### C3.1 What is advertised

Builtin service tools MUST advertise the success envelope schema
([`schemas/dispatch-envelope.schema.json`](./schemas/dispatch-envelope.schema.json)) via
`Tool::with_raw_output_schema`, serialized as `outputSchema`.

A tool MUST NOT advertise it unless **every** success path that can answer it produces a C2.1
envelope **and** sets `structuredContent`. Tools with bespoke shapes advertise a bespoke schema
or none.

### C3.2 Success-only — and why

The advertised schema describes **only** the success envelope.

**The MCP specification does not contain an explicit exemption for `isError` results.** The
normative sentence is unchanged across 2025-06-18, 2025-11-25, and 2026-07-28:

> "If an output schema is provided: Servers MUST provide structured results that conform to
> this schema. Clients SHOULD validate structured results against this schema."

Treating `isError` results as exempt is a **converged ecosystem convention**, not spec text.
Implementations are still being corrected toward it — the official TypeScript SDK *client*
validates error envelopes against the success schema and throws `-32602`
([typescript-sdk#1945](https://github.com/modelcontextprotocol/typescript-sdk/pull/1945) is the
open fix; its server side already guards).

The decision to exclude errors nonetheless stands, for two reasons:

1. Widening a tool's schema to also cover errors was explicitly rejected upstream as violating
   settled semantics ([cyanheads/mcp-ts-core#241](https://github.com/cyanheads/mcp-ts-core/issues/241)).
2. **Repo-local, decisive:** `enrich_completed_tool_error_result`
   (`crates/labby-gateway/src/upstream/tool_error.rs:148-180`, from `3e5ab3df`) rewraps error
   `structuredContent` into `{"error": <contract>, "upstream_structured_content": <original>}`
   — a published, schema-locked contract. One schema cannot describe both.

### C3.3 Conformance obligation

Once advertised:

- every **success** `structuredContent` MUST validate against it;
- `structuredContent` MUST be present on every success path — **the Python SDK raises a hard
  client-side error** ("Tool X has an output schema but did not return structured content"),
  which already broke Claude Code's own Bash tool in production
  ([anthropics/claude-code#14465](https://github.com/anthropics/claude-code/issues/14465));
- error results are exempt per C3.2 but MUST still carry `isError: true` and their envelope;
- Labby does not validate its own output at runtime; neither does rmcp 3.1.0 (verified).
  Conformance is enforced by tests. **MCP Inspector is a weaker validator than production
  clients** ([inspector#1005](https://github.com/modelcontextprotocol/inspector/issues/1005)) —
  passing it is not evidence of conformance.

### C3.4 Version gating

`outputSchema` MUST be advertised unconditionally. rmcp serializes it regardless of negotiated
version; pre-2025-06-18 clients ignore the unknown field. Deliberate, not an omission.

**Note:** SEP-2106 (2026-07-28) loosened `structuredContent` to any JSON value and the schemas
to any JSON Schema 2020-12 keywords. This relaxes rather than constrains Labby's object
envelope.

### C3.5 Stability

Contract version 1. Changing required fields, or narrowing `data`, is a breaking change
requiring a version bump plus updates to `docs/surfaces/MCP.md` and generated docs in the same
change.

**`additionalProperties` is `true`.** Decided (SPEC §5.2), not left open. Closing the envelope
would make any future `build_success` field invalidate all seven builtins' advertised schemas
simultaneously and client-side, *and* move `descriptor_contract_hash` in the same stroke — and
this envelope family demonstrably grows (the error envelope already gained a versioned recovery
contract). The detectability that `false` would buy is obtained internally instead, by the
"exactly four keys" assertion in the conformance test. A comment at `envelope.rs:42` MUST bind
`build_success` and this schema to the same commit.

---

## C4. Code Mode trace

`codemode` and `codemode_ui` advertise `code_mode_trace_output_schema`
(`handlers_tools.rs:686-765`) and MUST advertise the **same** schema — one execution backend,
differing only in MCP App metadata.

- Success results MUST satisfy it, including `logs_count`
  (`crates/labby-codemode/src/trace.rs`).
- `result` is deliberately unconstrained (`{}`), so a truncation marker is conforming.
- The **error** trace (`crates/labby/src/mcp/call_tool_codemode.rs`, the `isError` structured
  payload) sets `logs_count: 0`. It carries `isError: true`, so it is exempt from conformance
  under C3.2 regardless — the field is there for internal consistency with trace consumers
  (the inline inspector reads `structuredContent` on both paths), not to satisfy the schema.
  Implemented as SPEC FR-7.
- `result_shaping` is present when the result-shape policy is non-`Off`, **or** when the soft
  large-result warning fired under any policy — including the default `Off`
  (`crates/labby-codemode/src/execute.rs`, and `shape_final_result` in `shape.rs`, which
  computes the warning before matching on policy). Consumers MUST NOT treat its presence as
  evidence that shaping is enabled, nor its absence as "not truncated" — see C7.

---

## C5. Upstream (proxied) tools

- An upstream tool's `outputSchema` is relayed as declared (`handlers_tools.rs:277-290`).
  Labby MUST NOT synthesize, widen, or narrow it.
- A **successful** relayed result's `structuredContent` MUST reach the downstream client
  byte-identically.
- **Error results are explicitly excluded** from that guarantee:
  `enrich_completed_tool_error_result` deliberately rewraps them (C3.2). A test asserting
  byte-identity MUST scope itself to the success path.
- On a builtin/upstream name collision the builtin answers, and the advertised schema MUST be
  the builtin's.

### C5.1 Metadata sanitization (keyword-scoped)

"Relay as declared" above describes *fidelity of shape*. It is NOT licence to forward
unsanitized upstream text into a client's context: upstream-authored `description` fields at
any nesting depth are a prompt-injection surface.

Documentation-bearing upstream metadata — `description`, `title`, `$comment`, and the
annotations title, at any schema nesting depth — MUST be sanitized once at the relay cache
chokepoint (`upstream/pool/helpers.rs::cached_upstream_tool` →
`gateway/projection.rs::sanitize_upstream_tool_metadata`), so the Raw `tools/list` relay and
the Code Mode catalog path are covered by one implementation.

Schema-semantic keyword values — `enum`, `const`, `default`, `examples`, `pattern`, `format`,
`$ref`, and property names — MUST relay byte-identically. Rewriting them would make Labby
advertise a schema that its own byte-identical relayed results (C5.2) then violate, so strict
clients would reject *conforming* upstream results.

Implemented as SPEC FR-9a.

### C5.2 Redaction non-goal (explicit)

Labby's gateway performs **no output-side redaction** of tool result payloads. Sanitization
applies to tool *metadata* (descriptions/schemas), never to result data. A byte-identical
relay test asserts the *absence* of transformation — it must not be mistaken for evidence that
payloads are safe. If an upstream echoes a secret in a result body, Labby proxies it
faithfully, by design.

---

## C6. Code Mode in-sandbox `callTool`

Derived by `unwrap_code_mode_upstream_result`
(`crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs:606-630`). Byte-identical since
`977cb2166` (2026-05-31) across three refactors.

**Precedence (normative).** First match wins.

| # | Condition | Result |
|---|---|---|
| 0 | `isError == Some(true)` | Not unwrapped. Caller (`:494`) converts to `CodeModeCallError`; surfaces in-sandbox as a thrown error. |
| 1 | `structured_content` is `Some(v)` | `v` as-is — including falsy JSON (`false`, `0`, `null`, `""`). |
| 2 | `content` non-empty and every block is text | Joined with `"\n"`, then parsed as JSON; on failure, the joined string. |
| 3 | `content` empty | `Value::Null`. |
| 4 | otherwise (mixed / binary) | The entire `CallToolResult` as JSON. |

**Invariants**

- Rule 1 MUST precede any inspection of `content`. When both are present the structured value
  wins and content blocks are discarded. mcp-ui links are unaffected — read from `_meta` via
  `extract_ui_link` (`:583-590`), not `content`.
- A present-but-falsy structured value MUST NOT be treated as absent (`if let Some(..)` tests
  presence, not truthiness).
- The unwrap MUST NOT stringify a structured value.
- Rule 2 joins **all** text blocks before a **single** parse attempt.
- Rule 4 exposes the upstream's `_meta` to sandbox code. No Labby-internal field was found to
  leak (the only Labby `_meta` write, `LABBY_ERROR_META_KEY`, fires solely on `isError` results,
  which never reach the unwrap), but the exposure is upstream-controlled and open-ended.

**Prior art.** Matches Cloudflare's `unwrapMcpResult`
([packages/codemode/src/mcp.ts](https://github.com/cloudflare/agents/blob/1bca2a62435dee1a75914c8840d028b832913d0f/packages/codemode/src/mcp.ts))
except: no legacy `toolResult` field (rmcp has none), and empty content yields `Null` rather
than the raw result.

---

## C7. Truncation and shaping

Truncation applies **only** at the outer execution boundary. In-sandbox intermediate `callTool`
results are never truncated.

**Two distinct markers exist:**

| Path | Shape | Default |
|---|---|---|
| `labby-codemode/src/truncate.rs:178-192` | object: `{truncated: true, original_size, original_tokens, preview, artifacts, next_action}` | **yes** — always-on budget net |
| `labby-codemode/src/shape.rs:99-135` | plain string `"[code mode result truncated]…"` | no — non-`Off` result-shape policy only |

- Only these reductions are permitted; shaping MUST NOT re-serialize a surviving structured
  value.
- A truncated run's trace MUST retain structured `calls[]`.
- The trace schema currently declares `result_shaping` as `{"type": "object"}` with no
  properties, so the `truncated` discriminator is **not** discoverable from `outputSchema`.
  Consumers must not rely on schema introspection to detect truncation.

---

## C8. Prohibitions

1. MUST NOT stringify-and-reparse structured data across a layer boundary.
2. MUST NOT embed schema JSON in a tool `description`.
3. MUST NOT `#[derive(Serialize)]` on `ToolError`.
4. MUST NOT add an error `kind` without variant + `IntoResponse` arm + `docs/dev/ERRORS.md`.
5. MUST NOT let descriptor builders — or the gating booleans that feed them — disagree.
6. MUST NOT admit unsanitized upstream schema text into an LLM-facing surface (C5.1).
7. MUST NOT construct a Labby-owned descriptor outside the shared registry builder.
   **Enforced, not aspirational:** all five Labby-owned descriptors are built in
   `crates/labby/src/mcp/permanent_tools.rs`, and `/clippy.toml` bans `Tool::new` elsewhere
   via `disallowed_methods = "deny"` — reintroducing a second construction site is a compile
   error, not a review catch.

---

## C9. Conformance checklist

- [ ] Advertised schemas match audited real shapes, and `structuredContent` is always present on success.
- [ ] All descriptor and gating logic derives from one implementation.
- [ ] Error results carry `isError: true`, are not schema-checked, and keep their recovery contract.
- [ ] Unwrap precedence C6 holds across the edge-case matrix.
- [ ] Success-path relayed `structuredContent` is byte-identical; error rewrapping is asserted as intended.
- [ ] Both truncation markers behave per C7.
- [ ] `cargo nextest run --workspace --all-features` and `cargo clippy --workspace --all-features --all-targets` clean.
