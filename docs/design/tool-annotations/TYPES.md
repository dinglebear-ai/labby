# Types — Tool Annotations

Concrete Rust being added. Domain rationale in [MODELS.md](MODELS.md); call-site
wiring in [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md).

House rules that constrain everything here: no `mod.rs`, no `#[async_trait]`
(`disallowed_macros = "deny"`), `unsafe_code = "forbid"`, edition 2024, MSRV
1.97.1.

## T1. New module

**Two** new modules, declared in `crates/labby/src/mcp.rs` alongside the existing
`pub(crate) mod` entries:

```rust
pub(crate) mod annotations;   // hint policy
pub(crate) mod descriptors;   // shared Tool construction (both mirror sites)
```

Visibility is `pub(crate)` — this is internal policy, not public API. No new
dependency: `rmcp` is already a direct dependency of `labby`.

> **Revised after review.** Two changes from the first draft, both verified:
> the shared descriptor builders live in a **new `mcp/descriptors.rs`**, not
> `permanent_tools.rs` (four of five relocated descriptors are *conditionally*
> advertised, contradicting that module's permanence invariant, and importing
> `RegisteredService` there closes a `registry ↔ permanent_tools` cycle); and the
> `AnnotationPolicy` struct is replaced by a free function plus a `match`.
> `ToolAnnotations` is `#[non_exhaustive]` **and** its builders are not `const fn`
> (`tool.rs:94-152`), so a plain-data intermediate is forced — but a struct with
> derives and a `new` ctor is one layer more than that requires.

## T2. Wire translation (revised — no wrapper struct)

```rust
//! Tool-level MCP annotation policy for Labby-owned tools.
//!
//! MCP annotations are per-tool; Labby records `destructive` per *action*
//! (`ActionSpec::destructive`) and fronts a whole service with one tool. A
//! tool-level hint is therefore the least-safe union of its actions.
//!
//! These hints are advisory to clients **and** consumed by Labby's own
//! fail-closed derivation at the next hop in labby-to-labby chains
//! (`cached_upstream_tool`). They are never an authorization gate on this hop.

use rmcp::model::ToolAnnotations;

use crate::registry::RegisteredService;

/// All four hints set explicitly; `title` is intentionally left unset.
///
/// `ToolAnnotations` is `#[non_exhaustive]` and its builders are not `const fn`,
/// so this cannot be a `const` table of `ToolAnnotations` — the booleans are the
/// stored form and this is the one translation point.
pub(crate) fn to_annotations(
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
) -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(destructive)
        .idempotent(idempotent)
        .open_world(open_world)
}

/// Least-safe shape for any tool without an explicit policy: over-warn, never
/// under-warn.
pub(crate) fn least_safe() -> ToolAnnotations {
    to_annotations(false, true, false, true)
}
```

## T3. Service hint table and derivation

```rust
/// Reviewed, human-audited hints: (read_only, idempotent, open_world).
///
/// `destructive` is absent on purpose — it is derived from the service's action
/// catalog so it can never drift from `ActionSpec::destructive`. `read_only: true`
/// asserts *every* action is non-mutating, a stronger claim than "no destructive
/// actions" — see SPEC.md § 5.
///
/// Returns `None` for an unlisted service so the caller can fall back to the
/// least-safe shape: a forgotten row over-warns rather than under-warns.
fn service_hints(name: &str) -> Option<(bool, bool, bool)> {
    Some(match name {
        // read-only
        "fs" | "lab_admin" => (true, true, false),
        // mutating, non-destructive: system.checks writes a probe file,
        // proxy.check probes caller-supplied URLs.
        "doctor" => (false, true, true),
        // documented override of R2 — requires_admin + key-only redaction.
        // See SPEC.md § 6. Not derived; deliberately forced destructive below.
        "server_logs" => (false, true, false),
        // mixed: at least one destructive action
        "snippets" | "setup" | "gateway" => (false, false, true),
        _ => return None,
    })
}

/// Services whose `destructiveHint` is deliberately NOT derived. See SPEC.md § 6.
const FORCED_DESTRUCTIVE: &[&str] = &["server_logs"];

/// Annotations for a builtin service tool.
///
/// Pure function of its argument — do NOT memoize in a process-global
/// `LazyLock`/`OnceLock` keyed by service name. `build_default_registry`,
/// `build_docs_registry`, and test registries produce different service sets, so
/// a global cache would leak one registry's answer into another's. The scan is
/// ~250 ns over `&'static` data against a ~1-30 ms caller; it is not worth caching.
pub(crate) fn service_annotations(svc: &RegisteredService) -> ToolAnnotations {
    let destructive = FORCED_DESTRUCTIVE.contains(&svc.name)
        || svc.actions.iter().any(|action| action.destructive);
    match service_hints(svc.name) {
        Some((read_only, idempotent, open_world)) => {
            to_annotations(read_only, destructive, idempotent, open_world)
        }
        None => least_safe(),
    }
}
```

Do **not** move these hints onto `RegisteredService`. That struct is built at
seven call sites via the documented service-onboarding path; a forgotten field
would silently take a default instead of failing closed, destroying the
over-warn-not-under-warn property this design depends on.

`RegisteredService.actions` is `&'static [ActionSpec]`
(`crates/labby/src/registry.rs:67`), so the derivation is a synchronous slice
scan over data already in hand at the call site — no async, no I/O, no lookup.

## T4. Meta-tool policies

Meta tools have no `RegisteredService`, so nothing is derivable — these are fixed.
Gate them with `#[cfg(feature = "gateway")]` to match their only consumers, or the
`fs`-only slice build reports dead code.

```rust
/// `codemode` / `codemode_ui`: execute snippets that may call any upstream tool.
#[cfg(feature = "gateway")]
pub(crate) fn code_mode() -> ToolAnnotations { to_annotations(false, true, false, true) }
/// `add_server`: persists gateway config and can spawn a local subprocess.
/// (NB: `gateway.test`/`gateway.add` are themselves `destructive: false` — this
/// value stands on its own merits, not on theirs. See SPEC.md § 5.)
#[cfg(feature = "gateway")]
pub(crate) fn add_server() -> ToolAnnotations { to_annotations(false, true, false, true) }
/// `gateway_status`: renders live upstream status; no mutation path.
#[cfg(feature = "gateway")]
pub(crate) fn gateway_status() -> ToolAnnotations { to_annotations(true, false, true, false) }
/// `mcp_app`: reversible `status|enable|disable` toggle for the Code Mode app surface.
#[cfg(feature = "gateway")]
pub(crate) fn mcp_app() -> ToolAnnotations { to_annotations(false, false, true, false) }
```

## T5. rmcp 3.1.0 API surface used

`rmcp-3.1.0/src/model/tool.rs`. Both `Tool` and `ToolAnnotations` are
`#[non_exhaustive]`, so **struct literals are illegal from this crate** —
construction must go through the builders.

| Item | Location | Signature |
|---|---|---|
| `ToolAnnotations::new` | `:95` | `fn new() -> Self` — all fields `None` |
| `.read_only` | `:125` | `fn read_only(self, bool) -> Self` |
| `.destructive` | `:131` | `fn destructive(self, bool) -> Self` |
| `.idempotent` | `:137` | `fn idempotent(self, bool) -> Self` |
| `.open_world` | `:143` | `fn open_world(self, bool) -> Self` |
| `ToolAnnotations::from_raw` | `:100` | `fn from_raw(title, read_only, destructive, idempotent, open_world) -> Self` — used by existing tests |
| `Tool::with_annotations` | `:216` | `fn with_annotations(self, ToolAnnotations) -> Self` |
| `Tool.annotations` | `:33` | `Option<ToolAnnotations>` |

Builders consume and return `self`, so they chain directly onto `Tool::new(..)`.

## T6. Shared descriptor builders

SPEC R6 requires one construction path feeding both mirror sites. That home is a
**new `crates/labby/src/mcp/descriptors.rs`**.

Not `permanent_tools.rs`, despite it already owning `code_mode_descriptor`
(`permanent_tools.rs:56`): that module's stated invariant is *"tools whose
identity and dispatch exist independently of upstream health"*, and four of the
five descriptors moving here are **conditionally advertised** (`route_scope` /
`service_visible_on_mcp` / `*_app_visible` gates). Only `mcp_app` is genuinely
permanent. Putting `RegisteredService` construction there would also close a
`registry ↔ permanent_tools` cycle. Leave `permanent_tools.rs` owning identity
and `resolve()`, and have `code_mode_descriptor` delegate to `descriptors::code_mode(..)`
so its `PERMANENT_TOOLS` assertion stays where it belongs.

```rust
use std::sync::Arc;

use rmcp::model::Tool;

use crate::mcp::annotations;
use crate::mcp::catalog::SERVER_LOGS_TOOL_NAME;
use crate::mcp::handlers_tools::server_logs_tool_meta;
use crate::registry::RegisteredService;

/// The single construction path for a builtin service tool.
///
/// Both `handlers_tools::list_tools_impl` (the wire) and
/// `peer_contract::visible_tool_descriptors` (the contract hash) must call this.
/// Divergence between them silently breaks `tools/list_changed`.
pub(crate) fn builtin_service_descriptor(
    svc: &RegisteredService,
    schema: Arc<serde_json::Map<String, serde_json::Value>>,
    server_logs_app_visible: bool,
) -> Tool {
    let tool = Tool::new(svc.name, svc.description, schema)
        .with_annotations(annotations::service_annotations(svc));
    if svc.name == SERVER_LOGS_TOOL_NAME && server_logs_app_visible {
        tool.with_meta(server_logs_tool_meta(svc.name))
    } else {
        tool
    }
}
```

The `schema` parameter takes the `Arc` by value; both call sites already hold one
and pass `Arc::clone(&schema)`.

## T7. Test-only types

```rust
#[cfg(test)]
fn fixture_annotated_upstream_tool(
    upstream: &std::sync::Arc<str>,   // NB: &Arc<str>, matching fixture_upstream_tool
    name: &str,
    annotations: rmcp::model::ToolAnnotations,
) -> UpstreamTool;
```

Sits beside the existing `fixture_upstream_tool`
(`crates/labby/src/mcp/handlers_tools/tests.rs:378`) and proves passthrough.
Built with `ToolAnnotations::from_raw(..)` to exercise **partial** blocks and
`title` preservation, which the fluent builders make awkward to express — a
partial block is the strong test, because it fails if Labby ever "helpfully"
fills in defaults.

The first draft declared this three-param but wrote it two-param, and typed
`upstream` as `&str`; neither would compile. Use the signature above.

## T8. Types explicitly not changed

| Type | Why untouched |
|---|---|
| `ActionSpec` (`labby-primitives/src/action.rs:9`) | SPEC N1 — no per-action hints this epic. |
| `UpstreamTool` (`labby-gateway/src/upstream/types.rs:34`) | Already carries the full `Tool`; nothing to add. |
| `ToolSafetyHints` (`labby-runtime/src/agent_error.rs:145`) | Consume side already complete. |
| `ToolDescriptor` (`labby-codemode/src/types.rs:98`) | Stretch bead `lab-g1av5.4`; must stay rmcp-free. |
| `ToolError` / envelopes | No error-surface change. |
