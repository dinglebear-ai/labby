# MODELS — Rust types on the path

Every model a change under issue #210 touches or relies on, with its real definition site.
Types marked **new** do not exist yet; everything else is current-tree fact.

---

## 1. SDK models (`rmcp` 3.1.0 — do not redefine)

Vendored at `~/.cargo/registry/src/*/rmcp-3.1.0/`. Pinned exactly (`rmcp = "=3.1.0"`), and no
bump is needed: every API this issue requires already exists.

### 1.1 `Tool` — `src/model/tool.rs:28-30`

```rust
pub struct Tool {
    pub name: Cow<'static, str>,
    pub input_schema: Arc<JsonObject>,
    /// An optional JSON Schema object defining the structure of the tool's output
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Arc<JsonObject>>,
    // … description, annotations, meta
}
```

- Wire name is `outputSchema` (struct-level `#[serde(rename_all = "camelCase")]`).
- `Option` + `skip_serializing_if` means a tool without a schema emits no key at all — so
  attaching schemas is additive and invisible to clients that do not look for it.
- `Arc<JsonObject>` is why schemas are built once into a `LazyLock` and shared by
  `Arc::clone`, never rebuilt per tool.

**Builders**

| Method | Site | Use |
|---|---|---|
| `with_raw_output_schema(Arc<JsonObject>) -> Self` | `tool.rs:210-213` | **What Labby uses.** Hand-built schema. |
| `with_output_schema::<T: JsonSchema>() -> Self` | `tool.rs:242-245` | schemars-derived; requires the `server` feature. |

Labby hand-builds every `Tool` and does **not** use the `#[tool]` macro, so the issue's
phrasing "give `#[tool]` fns concrete return types so rmcp derives outputSchema" does not
apply as written. The equivalent here is attaching a hand-built schema — which the codebase
already does once, at `handlers_tools.rs:204`.

### 1.2 `CallToolResult` — `src/model.rs:3785-4034`

```rust
pub struct CallToolResult {
    pub result_type: Option<ResultType>,
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,   // wire: structuredContent
    pub is_error: Option<bool>,
    pub meta: Option<MetaObject>,
}
```

Constructors relevant here:

- `CallToolResult::success(content)` — Labby's current path, followed by manual
  `result.structured_content = Some(envelope)`.
- `CallToolResult::structured(value)` — sets `structured_content` **and** a text block
  containing `value.to_string()`, satisfying the 2025-06-18 backward-compat SHOULD in one
  call. A candidate simplification, but see the warning below.
- `CallToolResult::structured_error(value)` — same, with `is_error: true`.

> **Do not blind-swap `success(...) + manual assignment` for `structured(...)`.**
> The existing code controls the exact text serialization (`envelope.to_string()`,
> `serde_json::to_string(&response)`). `structured()` uses `value.to_string()`. For the
> dispatch envelope these coincide, but the Code Mode path serializes a *different* value
> into text than it puts in `structured_content` (the execution response vs. the enriched
> trace) — collapsing them there would change observable output. Treat any such swap as a
> behavior change requiring its own test, not a cleanup.

### 1.3 Schema helpers — `src/handler/server/common.rs`

```rust
pub fn schema_for_type<T: JsonSchema + Any>() -> Arc<JsonObject>;
pub fn schema_for_input<T: JsonSchema + Any>() -> Result<Arc<JsonObject>, String>;
pub fn schema_for_output<T: JsonSchema + Any>() -> Arc<JsonObject>;
```

Thread-local `TypeId`-keyed caches; repeat calls return the same `Arc`. `schema_for_output`
performs no root-type validation (output schemas are not restricted to `type: "object"` per
SEP-2106). Only relevant if a future change derives schemas from Rust types; the envelope
schema is hand-built because the envelope is assembled as a `serde_json::json!` literal, not
a struct.

### 1.4 `Json<T>` wrapper — `src/handler/server/wrapper/json.rs`

`Json<T>` makes a `#[tool]` fn derive `outputSchema` and return structured content
automatically. **Not applicable to Labby** (no `#[tool]` macro usage). Documented so future
readers do not conclude it was overlooked.

---

## 2. Labby envelope models

### 2.1 The dispatch envelope — `crates/labby/src/mcp/envelope.rs:42-49`

```rust
#[must_use]
pub fn build_success(service: &str, action: &str, data: &Value) -> Value {
    json!({
        "ok": true,
        "service": service,
        "action": action,
        "data": data,
    })
}
```

There is **no Rust struct** for the envelope — it is a `serde_json::Value` literal. That is
why `schemas/dispatch-envelope.schema.json` is hand-written rather than schemars-derived. It
sets `additionalProperties: true` (SPEC §5.2): the literal has exactly four keys today, but
closing the object would export that brittleness to clients the day a fifth is added. The
"exactly four keys" conformance test enforces it internally instead.

Introducing a `DispatchEnvelope` struct is **out of scope**: it would ripple through
`format_dispatch_result`, the HTTP surface, and `ToolError`'s hand-written serialization for
no gain this issue needs.

### 2.2 Attachment point — `crates/labby/src/mcp/result_format.rs:111-114`

```rust
let envelope = build_success(service, action, &v);
let mut result = CallToolResult::success(vec![ContentBlock::text(envelope.to_string())]);
result.structured_content = Some(envelope);
```

Built once, serialized twice — text block and structured content are the same value. This is
the pattern CONTRACT §C2.1 makes normative, and the reason no stringify-reparse exists on this
path.

The error branch does the same at `result_format.rs:66-70`.

### 2.3 `ToolError` — `crates/labby/src/dispatch/error.rs`

Canonical error type across MCP, HTTP, and CLI. **Invariants that constrain this issue:**

- Serialization is **hand-written**, never `#[derive(Serialize)]` — the `Sdk { sdk_kind }`
  variant promotes `sdk_kind` to the top-level `kind`.
- `Display` emits JSON, not prose.
- `IntoResponse` is shared by MCP and HTTP, so status mapping changes hit both.
- Adding a kind requires variant + `IntoResponse` arm + `docs/dev/ERRORS.md` together.

This issue introduces **no** new kinds. If one becomes necessary, that is a spec change.

---

## 3. Code Mode models

### 3.1 `ToolDescriptor` — `crates/labby-codemode/src/types.rs:97-114`

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolDescriptor {
    pub kind: CodeModeCatalogKind,
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,   // ← already exists
    pub signature: String,
    pub dts: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<CodeModeSnippetInputEntry>,
}
```

`output_schema` already exists and is already populated — scope item 3 of the issue is largely
verification, not construction.

**Constructors**

- `ToolDescriptor::tool(namespace, tool, description, schema, output_schema)` —
  `types.rs:136-163`. Takes the host's **already-sanitized** schemas and calls
  `generate_tool_types` to derive the TS signature and `.d.ts`.
- `ToolDescriptor::snippet(info)` — `types.rs:165-192`. Hard-codes `output_schema: None`
  (`:186`). **Correct and deliberate**: a snippet returns an arbitrary JS value, so
  `Promise<any>` is truthful. SPEC FR-8 requires a comment saying so, not a fix.

### 3.2 `CodeModeCatalogKind` — `types.rs:116-121`

```rust
#[serde(rename_all = "snake_case")]
pub enum CodeModeCatalogKind { Tool, Snippet }
```

### 3.3 Population from upstream — `crates/labby-gateway/src/gateway/code_mode/search.rs:135-141`

```rust
ToolDescriptor::tool(
    &upstream,
    &name,
    &sanitize_tool_text(&description, 2048),
    sanitize_schema(tool.input_schema),
    sanitize_schema(tool.output_schema),   // ← upstream output schema captured
)
```

Sanitization is not optional decoration: descriptions and schemas flow verbatim into an LLM
context, making them a prompt-injection surface. Any new path adding schema content to the
catalog must route through `sanitize_schema`.

### 3.4 `ToolCallOutcome` and the unwrap — `code_mode_host.rs:450-548`, `:606-630`

```rust
Ok(ToolCallOutcome {
    value: unwrap_code_mode_upstream_result(result),
    ui,
})
```

```rust
fn unwrap_code_mode_upstream_result(result: CallToolResult) -> Value {
    if let Some(value) = result.structured_content {
        return value;                    // rule 1 — precedes all content inspection
    }
    let all_text = !result.content.is_empty()
        && result.content.iter().all(|c| c.as_text().is_some());
    if all_text {
        let text = result.content.iter()
            .filter_map(|c| c.as_text())
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text));
    }
    if result.content.is_empty() { Value::Null } else { serde_json::json!(result) }
}
```

`if let Some(value)` is why falsy JSON (`false`, `0`, `null`, `""`) is preserved rather than
skipped — the JS truthiness bug class is structurally impossible here. Rule 0 (`is_error`) is
enforced by the caller at `code_mode_host.rs:494`, before unwrap is reached.

---

## 4. New models introduced by this work

Exactly one, and it is a function rather than a type:

```rust
/// Success-envelope output schema shared by every builtin service tool.
/// Mirrors `build_success` (mcp/envelope.rs:42) — see
/// docs/plans/210-mcp-output-schema/schemas/dispatch-envelope.schema.json.
pub(crate) fn dispatch_envelope_output_schema() -> Arc<serde_json::Map<String, Value>>;
```

Built with `LazyLock` and returned by `Arc::clone`, exactly mirroring
`code_mode_trace_output_schema()` (`handlers_tools.rs:686-765`). Full implementation in
`IMPLEMENTATION_PLAN.md` §3.1.

**Where it lives (corrected).** An earlier draft proposed a new `mcp/tool_descriptors.rs`
module. Research found the seam already exists: `PermanentToolRegistry`
(`crates/labby/src/mcp/permanent_tools.rs:56-77`) is already a registry-owned descriptor
constructor calling `Tool::new(...).with_raw_output_schema(...)`, already consumed by both call
sites via `self.registry.permanent_tools()`. The schema function and the new
`builtin_service_tool` / synthetic-tool constructors belong there. A parallel module would split
"where do Labby-owned descriptors come from?" across two files — see SPEC §5.6.

### 4.1 One existing type worth knowing: `ActionSpec.returns`

```rust
/// Type-name hint for the return shape … Not a runtime contract — purely informational.
pub returns: &'static str,   // crates/labby-primitives/src/action.rs:42-44
```

Live values are `"DoctorReport"`, `"Catalog"`, `"stream<Finding>"` — labels that resolve to no
definition anywhere in the repo. This is why SPEC NG-1 records "call the `schema` action" as a
**limitation rather than a mitigation**: that action returns this field, so it cannot describe
`data`. If per-action output schemas are ever built, `ActionSpec.returns` is the seam to grow —
do not introduce a third return-shape vocabulary alongside it and the tool-level schema.

No new structs, no new enums, no new error kinds. The work is descriptor plumbing plus tests —
the correct outcome given how much of the issue was already implemented.
