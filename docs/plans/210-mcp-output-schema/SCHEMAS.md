# SCHEMAS — artifacts and provenance

Two JSON Schema artifacts back issue #210. **Reduced from four** after research showed two were
duplicating existing sources (see [RESEARCH.md](RESEARCH.md) §11).

| File | Advertised as `outputSchema`? | Runtime source of truth |
|---|---|---|
| [`dispatch-envelope.schema.json`](../../contracts/schemas/dispatch-envelope.schema.json) — **promoted** to `docs/contracts/schemas/` | **Yes** — builtin service tools (Raw mode on `tools/list`; Code Mode catalog via in-process peers) | `dispatch_envelope_output_schema()`, a private free function in `crates/labby/src/mcp/permanent_tools.rs` (privacy is load-bearing — see the plan's Encapsulation note) |
| [`code-mode-trace.schema.json`](schemas/code-mode-trace.schema.json) | **Yes** — already live for `codemode`/`codemode_ui` | `code_mode_trace_output_schema()` (`crates/labby/src/mcp/handlers_tools.rs:686-765`) |

**Removed:**

- `dispatch-error-envelope.schema.json` — duplicated the already-published, already-drift-tested
  `docs/contracts/schemas/agent-error.schema.json`. Maintaining a second copy of the error
  contract is exactly the drift this issue exists to eliminate. Reference the published one.
- `catalog-tool-descriptor.schema.json` — hand-mirrored the `#[derive(Serialize)] struct
  ToolDescriptor` (`crates/labby-codemode/src/types.rs:97-114`) with no test enforcing
  agreement. The Rust struct plus [MODELS.md](MODELS.md) §3.1 is sufficient.

---

## Repo convention (this package must conform on implementation)

Labby already publishes contract schemas, and **tests enforce them**:

- Location: `docs/contracts/schemas/*.schema.json`
- `$id`: `https://dinglebear.ai/schemas/labby/<name>-v1.json`
- Paired contract doc: `docs/contracts/<name>.md` with YAML frontmatter (`title`, `created`,
  `updated`), plus `Status:` / `Surfaces:` / `Related:` lines
- Drift test: reads the `.json` **as plain JSON data — no schema-validation dependency** — and
  asserts `required` fields and enum/const membership

Reference implementations: `crates/labby-runtime/tests/agent_error_schema.rs:86` and
`crates/labby-codemode/tests/code_mode_error_schema.rs:70`.

This answers what an earlier draft left open (whether to add a `jsonschema` dev-dependency):
**no** — follow the existing plain-JSON drift-test pattern. On implementation,
`dispatch-envelope.schema.json` moves to `docs/contracts/schemas/` and CONTRACT.md becomes
`docs/contracts/mcp-tool-output.md`.

---

## 1. Dispatch success envelope

```json
{
  "type": "object",
  "properties": {
    "ok":      { "const": true },
    "service": { "type": "string" },
    "action":  { "type": "string" },
    "data":    { }
  },
  "required": ["ok", "service", "action", "data"],
  "additionalProperties": true
}
```

**Why `data` is empty.** `{}` accepts any JSON value. One MCP tool serves an entire service's
action table; a tool-level schema cannot describe per-action payloads without becoming a union
regenerated on every action change. Note the honest consequence: there is currently **no**
mechanism by which a consumer learns a specific action's result shape — see SPEC NG-1, which
records this as a limitation rather than the mitigation an earlier draft claimed.

**Why `ok` is `const: true`.** Makes success/error discrimination explicit and guarantees an
error envelope can never validate against the success schema.

**Why `additionalProperties: true`.** `build_success` (`envelope.rs:42-49`) constructs exactly
four keys, and closing the object would make a stray fifth one a schema violation — but
*client-side*, on all seven builtins simultaneously, and coupled to a contract-hash move. That is
the textbook `PROPERTY_ADDED_TO_OPEN_CONTENT_MODEL` break, and this envelope family demonstrably
grows (the error envelope already gained a versioned recovery contract). The detectability that
`false` would buy is obtained internally instead, by the conformance test's "exactly four keys"
assertion. Decided in SPEC §5.2; CONTRACT §C3.5's version-bump rule still applies.

**Scope caveat.** Builtins are suppressed from `tools/list` whenever Code Mode is enabled, so
this schema reaches clients only in Raw mode (`server_logs` excepted). SPEC §2.1.

## 2. Code Mode trace

Already advertised. Two properties matter here:

- **`result` is unconstrained (`{}`)** — a truncation marker is a conforming value. Already
  correct; SPEC AC-8 pins it rather than changing it.
- **`required` includes `logs_count`**, which the error path omits. Because that trace carries
  `isError: true` it is exempt from conformance, so this is an internal inconsistency for trace
  consumers, not a protocol violation (SPEC FR-7).

A third property is worth knowing but is **not** fixed by this issue: `result_shaping` is
declared as `{"type": "object"}` with no properties, and is only *present* when the result-shape
policy is non-`Off` (the default is `Off`). So the `truncated` discriminator is neither
discoverable from the schema nor reliably present — consumers cannot detect truncation by schema
introspection. See CONTRACT §C7.

---

## 3. Validation

```bash
for f in docs/plans/210-mcp-output-schema/schemas/*.json; do jq -e type "$f" >/dev/null || echo "BAD $f"; done
```

Runtime conformance is enforced by Rust tests, not by loading these files — except for the
drift test described above, which reads them as data.
