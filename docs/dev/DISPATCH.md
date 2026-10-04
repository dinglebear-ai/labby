---
title: "Dispatch"
created: "2026-07-30"
updated: "2026-07-30"
---

# Dispatch

This document is the canonical dispatch-layer contract for Labby.

It defines:

- the layer model between product surfaces and shared runtimes
- which layer owns operation metadata and execution
- the shared operation schema used across CLI, MCP, and API
- allowed dependency direction
- what each surface adapter owns
- how typed CLI, MCP, and API relate to the same shared backend

## Goal

Every service operation must have one shared execution path regardless of which product surface invokes it.

The contract is:

- humans use a typed CLI
- machines use `action + params` through MCP and API
- all three surfaces call the same surface-neutral dispatch layer

This prevents:

- `CLI -> MCP` coupling
- `API -> MCP` coupling
- repeated client-resolution logic per surface
- repeated operation validation and execution logic per surface

## Layer Model

The product adapters call shared dispatch or the owning extracted runtime:

- `labby-primitives`, `labby-runtime`, `labby-gateway`, `labby-codemode`, and `labby-auth`
- `labby-apis` where pure SDK/HTTP contracts are needed
- `crates/labby/src/dispatch`
- `crates/labby/src/cli`
- `crates/labby/src/mcp`
- `crates/labby/src/api`

### `labby-apis`

`labby-apis` owns:

- pure setup, doctor, and provider-neutral Artifact control contracts
- shared HTTP primitives, request/response types, and SDK error taxonomy

It does not own product-surface dispatch or the upstream MCP gateway. The latter
lives in `labby-gateway`; services need not introduce an SDK client when their
operation is owned by a local or extracted runtime.

### `crates/labby/src/dispatch`

`dispatch` is the shared product dispatch layer.

It owns:

- operation catalog per service
- operation schema per service
- param metadata and validation
- destructive-op metadata
- client and instance resolution
- invoking the owning runtime or SDK client
- surface-neutral results
- surface-neutral dispatch errors

It does not own:

- `clap` parsing
- MCP tool registration
- MCP envelopes
- HTTP status codes
- axum response types
- table rendering

Code Mode follows the same ownership rule across extracted crates:
`labby-gateway/src/gateway/code_mode/` owns catalog/search and upstream host
integration; `labby-codemode` owns the sandbox, parent broker, and runner
protocol. `dispatch/gateway/` supplies product wiring. MCP and CLI adapt inputs
and outputs; neither owns the shared execution engine.

### Surface Adapters

The three product surfaces are adapters over `dispatch`.

#### CLI

CLI owns:

- typed command and flag parsing
- human-facing command UX
- output formatting
- confirmation prompts

CLI does not own shared operation semantics.

CLI syntax, help, and completion derive from the Clap command graph. Shared
dispatch still owns parameter validation and action metadata; CLI confirmation
uses the shared destructive classification. Generated CLI help is therefore a
separate projection from the action catalog, not a catalog-generated CLI.

The CLI does not need to expose machine-oriented `action + params` syntax to humans in order to consume the shared schema.

#### MCP

MCP owns:

- tool registration
- one-tool-per-service exposure
- MCP envelopes
- protocol-level `help` and `schema` exposure
- elicitation behavior

MCP does not own shared operation semantics.

MCP must project the shared operation schema rather than acting as the source of truth for it.

#### API

API owns:

- axum routing
- request extraction
- status-code mapping
- HTTP response shaping

API does not own shared operation semantics.

API must use the shared operation schema for validation. When API documentation is exposed, it must derive from that same shared schema.

## Allowed Dependency Direction

Allowed:

- `cli -> dispatch/owning runtime -> SDK where needed`
- `mcp -> dispatch/owning runtime -> SDK where needed`
- `api -> dispatch/owning runtime -> SDK where needed`

Forbidden:

- `cli -> mcp`
- `api -> mcp`
- `cli -> api`
- `mcp -> api`

The MCP and API layers are sibling adapters, not shared backends for each other.

### Sanctioned cross-service edges

By default a service's dispatch module does not import or call another service's
dispatch module. A small set of cross-service edges is explicitly sanctioned for
composite operations that are intrinsically multi-service. Each sanctioned edge
is also encoded in the `ALLOWED_EDGES` matrix in
`crates/labby/tests/architecture_orchestrator.rs`, which fails the build if an
unlisted `dispatch::<a> → dispatch::<b>` import appears.

The executable allowlist is authoritative and includes, for example:

- **`setup → doctor`** — the Bootstrap orchestrator exception.
  `setup.draft.commit` invokes `doctor::dispatch("audit.full", _)` to gate the
  merge of `.env.draft` into `.env` on a clean health audit. The direction is
  strictly one-way: setup may depend on doctor; doctor must never depend on
  setup.

- `gateway → snippets`, `doctor → gateway`, and `setup → gateway` for runtime
  integration; `skills → skill_library` for library delegation; and
  `tasks → agents` for pinned Agent execution. The matrix also includes Artifact
  composition edges; do not treat this summary as its complete inventory.

Surface adapters (CLI/MCP/HTTP) must not chain dispatch calls across services
themselves — composite orchestration belongs in the shared dispatch layer so all
three surfaces share identical semantics. To add a new cross-service edge, add
the `(consumer, target)` pair to `ALLOWED_EDGES` with a one-line rationale and
explain it in the same PR.

## Operation Contract

Each service has one canonical operation catalog in `dispatch`.

That catalog owns:

- operation name
- description
- param schema metadata
- destructive flag and independent admin requirement
- result description

Operation names must remain stable and machine-oriented. Dotted names such as `movie.get` or `sites.list` are appropriate for shared internal identity even when the CLI exposes a different typed syntax.

## Shared Operation Schema

The operation catalog must be represented as a shared surface-neutral schema.

That schema must define:

- operation name
- description
- params
- required versus optional params
- param types
- destructive flag
- result description

The implementation uses `ActionSpec` and `ParamSpec` from `labby-primitives`
(with compatibility re-exports). The ownership rule is:

- the schema belongs to `dispatch`
- surfaces project it
- surfaces do not redefine it independently

The shared schema is the semantic contract that keeps:

- typed CLI operation mapping and shared validation
- MCP `help` and `schema`
- API validation and documentation

aligned over time.

## Metadata Ownership

Semantic metadata belongs to `dispatch`, not to any single transport.

That includes:

- operation names
- descriptions
- params metadata
- destructive flags
- return descriptions

In practice, this must be modeled as the shared operation schema rather than as transport-local copies.

Transport layers may project that metadata into:

- action explanations used alongside Clap-owned CLI help
- MCP `help`
- MCP `schema`
- API documentation

They must not redefine it independently.

## Error Contract

`dispatch` returns `Result<Value, ToolError>` directly.

**Design decision (2026-04-09):** A separate `DispatchError` type was considered and rejected. Both `dispatch/` and the surface adapters live in the same `labby` crate — there is no structural enforcement benefit to a parallel error vocabulary. A `DispatchError → ToolError` mapping layer adds a catch-all arm trap (any unmatched variant silently becomes `internal_error`) with no architectural gain. `ToolError` already has the correct vocabulary: `UnknownAction`, `MissingParam`, `InvalidParam`, `UnknownInstance`, `Sdk`. Using it directly keeps the error path exhaustively checked by the compiler at every call site.

Those errors may represent:

- shared SDK failures (`ToolError::Sdk { sdk_kind, message }` — passthrough from `ApiError::kind()`)
- missing or invalid params (`ToolError::MissingParam`, `ToolError::InvalidParam`)
- unknown operations (`ToolError::UnknownAction`)
- unknown instances (`ToolError::UnknownInstance`)
- missing destructive confirmation (`ToolError::ConfirmationRequired`) on
  surfaces that implement confirmation; HTTP action dispatch does not add an
  interactive confirmation gate

Surface adapters receive `ToolError` directly and handle it for their transport:

- CLI: serialize to JSON string or format for human display
- MCP: already the native envelope type
- API: the local `ApiError` wrapper implements `IntoResponse` and maps `kind()`
  to HTTP status while preserving the recovery envelope

`ToolError` must not be constructed or pattern-matched inside `labby-apis`. Its
definition lives in `labby-runtime/src/error.rs` and product dispatch re-exports it.

The canonical shared error vocabulary remains defined by [ERRORS.md](./ERRORS.md).

## Result Contract

The dispatch layer must return a surface-neutral result.

For initial migration, returning `serde_json::Value` is acceptable if it reduces churn and keeps the refactor incremental.

Longer term, the dispatch layer may grow a more typed result wrapper if needed, but the contract does not require that immediately.

The important rule is:

- surfaces must not re-execute operation logic to reshape results

The canonical serialization rules remain defined by [design/SERIALIZATION.md](../design/SERIALIZATION.md).

## Client Resolution

Client and instance resolution belong below or inside shared dispatch.

Rules:

- surfaces must not read env directly to construct service clients
- default-instance and named-instance behavior must be consistent across CLI, MCP, and API
- client construction must use shared helpers

This is a primary reason the dispatch layer exists.

## CLI Contract

Typed CLI is the human-facing contract.

Rules:

- new services must default to typed subcommands
- typed CLI commands may map to shared machine-oriented operation names internally
- CLI syntax must not force MCP-style `action + params` onto human users

The CLI remains free to choose ergonomic command names and flags as long as those map to the canonical service operations.

## MCP Contract

MCP defaults to the machine-facing router projection.

Rules:

- router mode exposes one tool per service with `action + params`
- atomic and combined projections derive flat per-action tools from the same
  `ActionSpec`; `permanent_tools.rs` owns descriptor construction
- `help` and `schema` are projections of the shared operation schema
- elicitation behavior is driven by the shared destructive metadata

MCP must not be the owner of shared operation execution.

## API Contract

API mirrors the machine-facing dispatch model.

Rules:

- generic service dispatch uses `action + params`; dedicated REST routes adapt
  their own path/query/body shape to the same shared operations
- API owns routing, extraction, and status mapping only
- API must use the same semantic operation catalog and execution path as MCP and CLI

API must not call MCP dispatchers directly.

## Observability Boundary

Dispatch observability must be centered around the shared operation execution boundary.

That means:

- adapters add surface context
- the dispatch layer knows the canonical operation and instance
- SDK request logs inherit that context downstream

The canonical observability rules remain defined by [OBSERVABILITY.md](./OBSERVABILITY.md).

## Testing Contract

The dispatch layer must be testable independently of the surfaces.

That allows:

- operation validation tests
- client-resolution tests
- dispatch error tests
- service execution tests

Surface layers must then need only:

- adapter tests
- envelope/status mapping tests
- a small number of integration verifications

## Migration Rule

Existing MCP service modules may be the source material for the shared dispatch layer because they already contain much of the operation matching and validation logic.

The target state is:

- move shared orchestration into `dispatch` or its owning extracted runtime
- let MCP wrap that shared operation
- let CLI wrap that shared operation
- let API wrap `dispatch`

The end state must not preserve `CLI -> MCP` or `API -> MCP` dependencies.

## Suggested Layout

One acceptable layout is:

```text
crates/labby/src/
  dispatch.rs
  dispatch/
    helpers.rs
    gateway.rs
    doctor/
    server_logs/
    setup/
    snippets/
```

The exact file breakdown may evolve, but every migrated service must start directory-first: thin `<service>.rs` entrypoint plus a `<service>/` directory with `catalog.rs`, `client.rs`, `params.rs`, and `dispatch.rs`, plus optional domain modules.

## Related Docs

- [ARCH.md](../ARCH.md)
- [SERVICE_ONBOARDING.md](./SERVICE_ONBOARDING.md)
- [OBSERVABILITY.md](./OBSERVABILITY.md)
- [ERRORS.md](./ERRORS.md)
- [design/SERIALIZATION.md](../design/SERIALIZATION.md)
