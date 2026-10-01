---
title: "Architecture"
created: "2026-07-30"
updated: "2026-09-30"
---

# Architecture

`labby` is a Rust MCP gateway and operator control plane split between reusable runtime crates, shared product dispatch, and thin surface adapters. In addition to gateway, Code Mode, auth, and the direct stdio proxy, the current product owns access/Projects, Artifacts, Agents/Tasks, browser and development-container lifecycles, and operator services. The [generated service catalog](./generated/service-catalog.md) and [service index](./services/README.md) are authoritative for exact registration and platform/feature exposure.

## Core Shape

- One workspace
- Reusable `labby-*` crates plus one product binary crate
- One `labby` binary
- A small set of feature-gated product slices
- One MCP tool per service

## Crate Split

### `crates/labby-primitives`

`labby-primitives` is a workspace-dependency leaf crate: `ActionSpec`/`ParamSpec`
(action metadata), `PluginMeta`/`EnvVar`/`Category` (plugin metadata),
`UiSchema` (Bootstrap wizard field schemas), and the static SSRF preflight
checks. These types are shared by both `labby-apis` (which re-exports them
from `core/`) and the gateway-extraction crates (`labby-gateway` depends on it
directly), so they live below both rather than in either — avoiding a choice
between forcing the gateway crates to pull in the full SDK, or forcing SDK
service modules to pull in gateway/runtime machinery just to declare
`pub const META: PluginMeta`.

### `crates/labby-apis`

`labby-apis` is the pure SDK layer for shared core primitives and the current setup/doctor contracts, not a general one-module-per-external-service SDK. It owns:

- typed service clients
- request and response models
- auth handling
- shared HTTP behavior
- shared error taxonomy
- health-check contracts

Action and plugin metadata (`ActionSpec`, `PluginMeta`, etc.) are re-exported
from `labby-primitives` rather than owned here — see above.

It does not own CLI parsing, MCP transport, HTTP routing, `.env` file loading,
or shell-facing UX.

### `crates/labby-auth`

`labby-auth` is the auth middleware crate. It owns:

- inbound OAuth authorization with the selected Google or Authelia provider
- JWT signing and validation (Ed25519 / EdDSA; Google ID-token verification remains RS256)
- SQLite-backed token and session storage
- axum middleware and route handlers
- upstream OAuth manager/cache/runtime helpers

It is separated from `labby-apis` because it depends on `axum`, which is
forbidden in the pure SDK crate. It does not own CLI parsing or MCP transport.

### `crates/labby-runtime`

`labby-runtime` owns surface-neutral contracts and helpers used across product
and extracted runtime crates:

- `ToolError`
- gateway config DTOs
- redaction and path-safety helpers
- backoff/jitter helpers
- feature-gated pure DTO dependencies

The runtime also owns shared Artifact/Skills, authority, Agent/task, development-container, and usage contracts. Gateway-specific dispatch helpers and transport/spawn guards remain in `labby-gateway`; generic SSRF vocabulary lives in `labby-primitives`. Follow the actual Cargo dependency graph rather than assuming auth or runtime crates do not consume primitives.

### `crates/labby-codemode`

`labby-codemode` is the client-neutral Code Mode execution kernel. It owns the
Javy/QuickJS runner protocol, warm runner pool, result shaping, snippet engine,
and TypeScript descriptor generation. Hosts inject tools through `CodeModeHost`.

### `crates/labby-gateway`

`labby-gateway` is the reusable gateway runtime. It owns upstream MCP proxy
pools, discovery/import orchestration, virtual servers, protected routes,
gateway OAuth lifecycle, manager state, the Code Mode host adapter, its own
`action`/`params` dispatch helpers, and the stdio spawn-guard/SSRF security
checks. Its public direct-stdio connector gives the product proxy the same
environment scrubbing, stderr draining, lifecycle negotiation, Unix process
group, and Windows Job Object ownership without routing through the aggregate
gateway catalog. It does not own product config rendering or `.env` writes;
those are injected by the host through `GatewayConfigStore`.

### `crates/labby-browser`

`labby-browser` owns the surface-neutral browser bridge runtime and persistence. Product registration and HTTP/MCP adapters remain in `labby`.

### `crates/labby-openapi`

`labby-openapi` owns OpenAPI parsing/projection and hardened outbound execution for the Code Mode local OpenAPI provider. Its HTTP transport stays outside the host-neutral JavaScript kernel.

### Development-only crates

`labby-model` owns pure lifecycle models and is never a product dependency. `xtask` owns repository automation, not product behavior. The root workspace has 13 members; `tools/verification` is a separate workspace with its own lockfile. See [Development Workflow](./dev/DEVELOPMENT.md) for the distinct validation gates.

### `crates/labby-web`

`labby-web` owns embedded and filesystem static asset serving for Labby web UI
exports, including symlink escape defense.

### `crates/labby-winjob`

`labby-winjob` is the small Windows Job Object helper crate. It contains the
platform FFI needed for process-tree reaping on Windows so the main workspace
can keep `unsafe_code = "forbid"` elsewhere.

### `crates/labby`

`labby` is the product binary. It owns:

- CLI commands
- MCP server registration and dispatch
- HTTP API route mounting
- config loading
- output rendering
- install/uninstall flows
- doctor and operator workflows
- foreground direct stdio proxy orchestration, loopback HTTP, Tailscale Serve,
  and ephemeral OAuth lease supervision
- product-local dispatch and config-store adapters

It must stay thin at the surface boundary. Reusable gateway, Code Mode, auth,
web-serving, and runtime helpers stay in their extracted crates.

## Golden Rule

If behavior is shared across product surfaces, it belongs in one shared execution layer. Pure setup/doctor SDK contracts belong in `labby-apis`; reusable gateway, auth, browser, OpenAPI, runtime, and Code Mode behavior belongs in the owning extracted `labby-*` crates; product-surface dispatch belongs in `crates/labby/src/dispatch`. The CLI, MCP, HTTP, and web layers are adapters, not logic owners.

That rule is structural, not aspirational:

- `labby-apis` has no `clap`, `rmcp`, or `axum`
- `labby-auth` has no `clap` or `rmcp`
- `labby-runtime` has no product-surface transport dependencies
- `labby` depends on extracted crates rather than duplicating runtime logic

## Module Layout

The workspace uses modern Rust module layout:

- no `mod.rs`
- a module `foo` is declared in `foo.rs`
- its submodules live in `foo/`

`labby-apis` contains `core`, `doctor`, and `setup`. Do not add a new SDK module merely to connect another external capability: normally configure an upstream MCP server. For a genuine Labby-owned lifecycle, follow [Service Onboarding](./dev/SERVICE_ONBOARDING.md).

Per-service layout in `labby` typically includes:

- `src/dispatch/<service>.rs` plus `src/dispatch/<service>/`
- `src/cli/<service>.rs`
- `src/api/services/<service>.rs` when the service is exposed over HTTP

## Shared Contracts

The architecture is anchored around a few cross-cutting contracts:

- `ServiceClient`: common health-check interface
- `ServiceStatus`: normalized health result
- service-specific ID newtypes
- `Auth`: shared auth model
- `ApiError`: normalized transport-layer error taxonomy
- `HttpClient`: shared SDK request/auth/timeout/logging/error mapping; operation owners decide retry/backoff
- `ActionSpec` / `ParamSpec`: service action catalog schema
- `PluginMeta`: service metadata for generated docs, install/setup flows, and
  doctor checks

These contracts keep service modules consistent and make CLI, MCP, HTTP, web,
and operator tooling compose cleanly.

### `ServiceClient`

The pure SDK `ServiceClient` contract exposes a common health surface:

- `name()`
- `service_type()`
- `health()`

This provides reusable health vocabulary without forcing local product services or the upstream MCP pool through one SDK trait. Gateway capability health and product doctor operations keep their own runtime contracts.

### `ServiceStatus`

`ServiceStatus` is the normalized health result shape. Its important fields are:

- reachability
- auth state
- optional version
- latency
- optional detail message

Rules:

- unreachable implies auth is not OK
- health probes have a shorter timeout budget than ordinary requests
- transport failures become structured status data rather than panics

### ID Newtypes

Service identifiers must use service-local newtypes rather than raw integers everywhere. The goal is to prevent mixing:

- internal ids
- external provider ids
- ids from different services

## Runtime Surfaces

The same service logic is exposed through the product surfaces that the service
opts into:

- CLI: `labby <service-or-command> ...`
- MCP stdio: `labby mcp`
- MCP HTTP: `labby serve`
- HTTP API and Labby web UI: `labby serve`

`labby proxy` is deliberately different: it is a CLI-only foreground product
runtime for one explicitly selected child. Its HTTP endpoint exposes the
child's MCP surface directly and does not register a `proxy` MCP tool or
`/v1/proxy` action route. OAuth lease management goes through the existing
admin-authenticated `gateway` action surface on a live daemon.

Registered surfaces consume shared action metadata and operation semantics. An adapter may use the owning reusable runtime directly where appropriate; it must not duplicate business rules or invent a second service catalog.

The canonical ownership and dependency rules between `labby-apis`, extracted runtime crates, the shared dispatch layer, and the product surfaces live in [DISPATCH.md](./dev/DISPATCH.md).

## Logging Shape

Observability is a mandatory shared contract, not a per-service convention.

The canonical source of truth is [OBSERVABILITY.md](./dev/OBSERVABILITY.md).

High-level ownership is:

- `labby` owns caller context and dispatch logging
- `labby-apis::core::HttpClient` owns SDK request logging and transport failure detail
- gateway and OpenAPI transports own equivalent logging at their specialized outbound boundaries

Required boundary rules:

- CLI, MCP, and HTTP must emit one dispatch event per user-visible action
- SDK `HttpClient` calls emit `request.start` plus `request.finish` or `request.error`; other outbound runtimes preserve the owning transport observability contract
- health probes must be distinguishable from normal actions
- destructive actions must log intent and outcome

Field-level requirements, redaction rules, and verification gates live in [OBSERVABILITY.md](./dev/OBSERVABILITY.md). Do not redefine them piecemeal in service modules.

## Data Flow

Normal request flow:

1. Load config in `labby`
2. Construct the correct SDK client or product-local subsystem
3. Dispatch through the shared `crates/labby/src/dispatch` layer
4. Invoke the owning runtime or SDK client, preserving its auth, timeout, response-budget, and error contracts; retry/backoff belongs to the operation owner
5. Return typed or surface-neutral data to the caller surface
6. Render via CLI, MCP envelope, API envelope, or web view

Direct proxy flow:

1. Resolve and spawn one child through the reusable direct-stdio connector.
2. Bind a Streamable HTTP router to loopback with exact Host/Origin policy.
3. Apply tailnet, bearer, OAuth, or explicit no-auth policy.
4. For OAuth, lease the exact public resource through the live daemon.
5. Publish and supervise one exact Tailscale Serve or public Funnel mapping when selected.
6. On Ctrl+C or component failure, clean owned HTTP, Tailscale mapping, lease, and process
   resources without touching aggregate gateway state.

See [guides/STDIO_MCP_PROXY.md](./guides/STDIO_MCP_PROXY.md) for the operator
contract and [contracts/stdio-mcp-proxy.md](./contracts/stdio-mcp-proxy.md) for
the stable wire and CLI vocabulary.

## Config Boundary

`labby-apis` never reads config files or ambient env on its own. Config loading lives in `labby`.

- secrets: `$LABBY_HOME/.env` (normally `~/.labby/.env`)
- preferences: exactly `$LABBY_HOME/config.toml` when the absolute override is
  set, otherwise `~/.labby/config.toml`

The binary resolves those inputs, then constructs clients explicitly.
See [Runtime Configuration](./runtime/CONFIG.md) for the authoritative
precedence and path contract.

## Service Model

Cargo features, compiled registration, runtime availability, and caller exposure are separate dimensions. Use the [feature matrix](./generated/feature-matrix.md), current registration code, and generated service catalog rather than treating a short handwritten inventory as exhaustive. The approved principal-scoped File Stash
contract is current default `gateway-host` functionality on Linux. It is
runtime-conditional, not a separate Cargo feature, and unsupported platforms
omit it from registration and routing.
Retired ACP, Registry-browser, Marketplace, Fleet/device runtime, Deploy-product,
and Agent Artifact Manager implementations are deleted rather than retained as
sleeping aliases. File Stash must not restore their component, revision,
workspace, provider, deploy-target, Marketplace-fork, or drift semantics.

For a first-class service or capability, add only the surfaces it actually
supports:

- stable vocabulary in the lowest reusable crate that needs it, not an automatic new `labby-apis` service module
- one shared dispatch entry in `crates/labby/src/dispatch`
- CLI, MCP, API, and web adapters only when the service exposes those surfaces
- one `PluginMeta` when it participates in generated env/service metadata
- one health-check implementation when it models a remotely configured service

Product-local surfaces are explicit. [`GATEWAY.md`](./services/GATEWAY.md)
documents the product-local management surface for runtime upstream
configuration. SDK-only or extracted service modules must not be documented as
current Labby CLI/MCP/API services unless they are registered by the current
`labby` crate feature table.
