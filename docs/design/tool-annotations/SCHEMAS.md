# Schemas — Tool Annotations

Wire-level shapes. Authoritative values in
[SPEC.md § Decision table](SPEC.md#6-decision-table-normative).

## S1. `ToolAnnotations` JSON Schema

The object as it appears under `annotations` in a `tools/list` entry. Serialized
by rmcp 3.1.0 with `camelCase` names and `skip_serializing_if = "Option::is_none"`.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://labby.dev/schemas/tool-annotations.json",
  "title": "ToolAnnotations",
  "type": "object",
  "additionalProperties": true,
  "properties": {
    "title": {
      "type": "string",
      "description": "Human-readable display title. Labby leaves this unset on its own tools."
    },
    "readOnlyHint": {
      "type": "boolean",
      "default": false,
      "description": "Tool does not modify its environment."
    },
    "destructiveHint": {
      "type": "boolean",
      "default": true,
      "description": "Tool may perform irreversible updates. Meaningful only when readOnlyHint is false."
    },
    "idempotentHint": {
      "type": "boolean",
      "default": false,
      "description": "Repeated calls with the same arguments have no additional effect. Meaningful only when readOnlyHint is false."
    },
    "openWorldHint": {
      "type": "boolean",
      "default": true,
      "description": "Tool interacts with entities outside its local environment."
    }
  }
}
```

`additionalProperties: true` is deliberate: upstream servers may send fields from
a newer spec revision, and Labby forwards them untouched (contract C4).

## S2. Labby-owned tool invariants

Additional constraints that hold for tools Labby constructs — **not** for
proxied upstream tools.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://labby.dev/schemas/labby-owned-tool-annotations.json",
  "title": "LabbyOwnedToolAnnotations",
  "$comment": "readOnlyHint:true implies destructiveHint:false for Labby-owned tools.",
  "type": "object",
  "allOf": [{ "$ref": "https://labby.dev/schemas/tool-annotations.json" }],
  "required": ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"],
  "not": { "required": ["title"] },
  "if":   { "properties": { "readOnlyHint": { "const": true } }, "required": ["readOnlyHint"] },
  "then": { "properties": { "destructiveHint": { "const": false } } }
}
```

Encoded invariants:

1. All four hints present (SPEC R1).
2. `title` absent (SPEC R1).
3. `readOnlyHint: true` ⇒ `destructiveHint: false` (SPEC R7, MODELS M3).

## S3. Serialized output per tool

Exactly what `tools/list` will contain after this change. Non-annotation fields
elided.

**Read-only tools** — `fs`, `lab_admin`, `gateway_status`
(`server_logs` is **not** in this group — see [SPEC § 6](SPEC.md#6-decision-table-normative)):

```json
{
  "name": "fs",
  "annotations": {
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": false
  }
}
```

**Mutating but non-destructive** — `doctor`:

```json
{
  "name": "doctor",
  "annotations": {
    "readOnlyHint": false,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": true
  }
}
```

`mcp_app` is identical except `"openWorldHint": false`.

**Destructive** — `setup`, `gateway`, `snippets`, `codemode`, `codemode_ui`, `add_server`:

```json
{
  "name": "setup",
  "annotations": {
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": true
  }
}
```

## S4. Passthrough examples

Upstream annotations are relayed byte-identical, including partial blocks,
unknown fields, and `title`.

Upstream sends a partial block with a future field:

```json
{
  "name": "search",
  "annotations": {
    "title": "Web Search",
    "readOnlyHint": true,
    "someFutureHint": "experimental"
  }
}
```

Labby's downstream `tools/list` emits **the same object** — no filling of the two
missing hints, no stripping of `someFutureHint`, `title` preserved.

Upstream sends no annotations at all:

```json
{ "name": "legacy_tool" }
```

Labby emits no `annotations` key either. It does **not** substitute its own
fail-closed judgment. Internally it still records `UpstreamTool.destructive =
true` for gating (MODELS M5), but that value never reaches the wire.

## S5. Gate-derivation truth table

How `cached_upstream_tool` (`crates/labby-gateway/src/upstream/pool/helpers.rs:423`)
maps an annotations block to `UpstreamTool.destructive`. Unchanged by this epic;
included because Labby's newly annotated tools become inputs to it at the next
hop.

| `annotations` | `readOnlyHint` | `destructiveHint` | ⇒ `destructive` |
|---|---|---|---|
| absent | — | — | `true` |
| present | any | `true` | `true` |
| present | any | `false` | `false` |
| present | `true` | absent | `false` |
| present | `false` / absent | absent | `true` |

Applied to Labby's own tools after this change: the four read-only tools and
`doctor` / `mcp_app` (explicit `destructiveHint: false`) derive `false`; the six
destructive tools derive `true`.

## S6. Annotation-policy config schema (internal)

Shape of the reviewed constant table in `crates/labby/src/mcp/annotations.rs`.
Rust consts, not a config file — schema given for review clarity.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://labby.dev/schemas/annotation-policy.json",
  "title": "ServiceHintRow",
  "type": "object",
  "required": ["service", "readOnly", "idempotent", "openWorld"],
  "additionalProperties": false,
  "properties": {
    "service":    { "type": "string", "description": "Registered service name; matches RegisteredService.name." },
    "readOnly":   { "type": "boolean", "description": "Audited claim. true requires zero destructive actions." },
    "idempotent": { "type": "boolean" },
    "openWorld":  { "type": "boolean" }
  },
  "comment": "destructiveHint is intentionally absent — derived from the action catalog at runtime."
}
```

`destructive` is absent by design: including it would create a second, drifting
source of truth beside `ActionSpec.destructive`.
