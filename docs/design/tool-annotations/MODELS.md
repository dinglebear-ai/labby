# Models — Tool Annotations

The domain model: what data exists, where it lives, and how an action-level flag
becomes a tool-level hint. Concrete Rust in [TYPES.md](TYPES.md); wire shape in
[SCHEMAS.md](SCHEMAS.md).

## Model map

```
ActionSpec.destructive          (per action,    crates/labby-primitives/src/action.rs:9)
        │
        │  any(…)                        ← derivation, this epic
        ▼
AnnotationPolicy                (per tool,     crates/labby/src/mcp/annotations.rs, new)
        │
        │  .to_annotations()
        ▼
rmcp::model::ToolAnnotations    (wire,         rmcp-3.1.0/src/model/tool.rs:50)
        │
        │  Tool::with_annotations
        ▼
rmcp::model::Tool.annotations   ──► tools/list ──► downstream client
                                          │
                                          │ (labby → labby only)
                                          ▼
                                 cached_upstream_tool  (helpers.rs:423)
                                          │
                                          ▼
                                 UpstreamTool.destructive  (upstream/types.rs:34)
                                          │
                                          ▼
                                 next-hop MRTR gate
```

## M1. `ActionSpec` — the per-action source of truth

`crates/labby-primitives/src/action.rs:9`

```rust
pub struct ActionSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub destructive: bool,      // ← the only safety field
    pub requires_admin: bool,
    pub params: &'static [ParamSpec],
    pub returns: &'static str,
}
```

There is **no** `read_only`, `idempotent`, or `open_world` field. That absence is
the reason `readOnly`/`idempotent`/`openWorld` must come from a reviewed constant
table rather than derivation, and the reason extending this struct is an explicit
non-goal (SPEC N1).

`destructive` already drives MRTR elicitation and CLI confirmation, so deriving
`destructiveHint` from it guarantees the advertised hint and the enforced gate
cannot disagree.

## M2. `RegisteredService` — the per-service aggregate

`crates/labby/src/registry.rs:51`

```rust
pub struct RegisteredService {
    pub name: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub kind: RegisteredServiceKind,
    pub status: &'static str,              // "available" | "stub"
    pub actions: &'static [ActionSpec],    // ← derivation input
    pub dispatch: DispatchFn,
}
```

Iterated via `ToolRegistry::services()` (`registry.rs:174`) — the same loop that
builds the tool list at `handlers_tools.rs:130`. Because `actions` is a
`&'static` slice on the value already in hand at construction time, the
derivation needs no new lookup, no async, and no I/O.

A `status == "stub"` service has an empty `actions` slice, so it derives
`destructive: false`. Stubs return `unknown_action` on call, so this is
harmless — but the constant table still governs its `readOnly` value, and an
unlisted service falls back to the least-safe shape (SPEC R4).

## M3. `AnnotationPolicy` — the new tool-level model

The unit this epic introduces: four booleans describing one tool.

| Field | Source for service tools | Source for meta tools |
|---|---|---|
| `read_only` | constant table (audited) | constant |
| `destructive` | **derived** from `actions` | constant |
| `idempotent` | constant table | constant |
| `open_world` | constant table | constant |

Split rationale: `destructive` has a machine-checkable source, so derive it and
it can never drift. The other three have no source in the data model, so they are
declared once, reviewed by a human, and protected by invariant tests (SPEC R7).

**Invariant.** `read_only == true` ⇒ the service has zero destructive actions.
One-directional: `doctor` has zero destructive actions yet is *not* read-only
(it writes a probe file). A test enforces the implication, so adding a
destructive action to `fs` or `lab_admin` fails CI rather than shipping a false
read-only claim. (`server_logs` is no longer in the read-only bucket — see
[SPEC § 6](SPEC.md#6-decision-table-normative).)

The invariant is necessary but **not sufficient**: `ActionSpec` cannot express
"mutating but non-destructive", so a mutating action added to a read-only service
would still pass. That is why the read-only services also carry a pinned
action-name allowlist, so any addition fails CI and forces a human re-audit.

## M4. `ToolAnnotations` — the wire model

`rmcp-3.1.0/src/model/tool.rs:50`, `#[non_exhaustive]`, `camelCase`, every field
`skip_serializing_if = "Option::is_none"`.

| Field | Type | MCP default when absent |
|---|---|---|
| `title` | `Option<String>` | — (we leave unset) |
| `read_only_hint` | `Option<bool>` | `false` |
| `destructive_hint` | `Option<bool>` | `true` (meaningful only when `readOnlyHint == false`) |
| `idempotent_hint` | `Option<bool>` | `false` (meaningful only when `readOnlyHint == false`) |
| `open_world_hint` | `Option<bool>` | `true` |

Because defaults are non-trivial and asymmetric, we set all four explicitly
(SPEC R1) rather than relying on omission. `#[non_exhaustive]` also forbids
struct literals from outside rmcp — construction must go through the builders
([TYPES.md § rmcp API](TYPES.md#t4-rmcp-310-api-surface-used)).

## M5. `UpstreamTool` — the proxied model

`crates/labby-gateway/src/upstream/types.rs:34`

```rust
pub struct UpstreamTool {
    pub tool: Tool,                 // ← the FULL upstream Tool, annotations intact
    pub input_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub upstream_name: Arc<str>,
    pub destructive: bool,          // ← derived, fail-closed, gateway-side only
}
```

Two distinct facts coexist deliberately:

- `tool.annotations` — the upstream's own claim, forwarded verbatim (C4).
- `destructive` — Labby's fail-closed *interpretation*, used only for its own
  gating. Never serialized back onto the tool.

This separation is why passthrough and safety do not conflict: Labby relays the
upstream's claim untouched while independently deciding how to gate it.

## M6. `ToolSafetyHints` — the rmcp-free mirror

`crates/labby-runtime/src/agent_error.rs:145`, populated by
`safety_hints_from_annotations` (`crates/labby-gateway/src/upstream/tool_error.rs:36`).

All four hints are **already** modeled end-to-end on the *consume* side and
surface in tool-error evidence. This epic only fills the *produce* side. Nothing
in the error contract changes; Labby's own tools simply start contributing real
values where they previously contributed `None`.

## M7. What is deliberately not modeled

| Not modeled | Why |
|---|---|
| Per-action hints | SPEC N1 — would require changing `ActionSpec` and the dispatch metadata contract. |
| Hints in the Code Mode catalog | `labby-codemode::ToolDescriptor` (`crates/labby-codemode/src/types.rs:98`) has no hint fields. Stretch bead `lab-g1av5.4`. |
| Annotation-sensitive `.dts` cache | `tool_shape_digest` (`gateway/code_mode/search.rs:25`) hashes description + schemas only. Same stretch bead. |
| Labby overriding upstream hints | Contract C4 forbids it. |
