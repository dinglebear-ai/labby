# Architecture

Lab is a Rust workspace with a split between reusable upstream-facing SDK clients and product-facing dispatch and surface layers. The architecture prioritizes clean crate boundaries: the SDK layer (`labby-apis`) has no dependency on product machinery, and the product binary (`labby`) composes those clients with surface-specific adapters.

## Core Principles

- **One workspace, multiple crates** - Separation between SDK, runtime, and product layers
- **One binary, four surfaces** - Single `labby` binary exposing CLI, MCP, HTTP API, and web UI
- **Feature-gated slices** - Build variants for different deployment scenarios
- **Shared dispatch layer** - Service-neutral operation handlers invoked by all surfaces
- **Extraction target** - Crates designed for reuse as standalone packages

## Crate Responsibilities

### `labby-primitives` (Dependency-Free Leaf)

**Purpose**: Shared vocabulary types that must not pull in dependencies.

**Contents**:
- `ActionSpec`/`ParamSpec` - Action metadata for service catalogs
- `PluginMeta`/`EnvVar`/`Category` - Plugin metadata
- `UiSchema` - Bootstrap wizard field schemas
- Static SSRF preflight checks

**Why separate**: Both `labby-apis` (SDK) and `labby-gateway` (extraction crate) need these types. Placing them in `labby-apis` would force gateway crates to pull in the full SDK. Placing them in `labby-runtime` would pull runtime dependencies into SDK service modules.

**Source**: [`crates/labby-primitives/`](../crates/labby-primitives/)

### `labby-apis` (Pure SDK Layer)

**Purpose**: Upstream-facing service clients and request/response models.

**Contents**:
- Typed service clients (HTTP-based)
- Request and response models
- Auth handling (HTTP client middleware)
- Shared error taxonomy
- Health-check contracts

**Re-exports**: Action/plugin metadata from `labby-primitives`

**Constraints**: No CLI parsing, no MCP transport, no HTTP routing, no `.env` loading.

**Source**: [`crates/labby-apis/`](../crates/labby-apis/)

### `labby-auth` (Auth Middleware)

**Purpose**: OAuth 2.0 and JWT authentication for HTTP surfaces.

**Contents**:
- OAuth 2.0 authorization server (Google OIDC provider)
- JWT signing and validation (RS256)
- SQLite-backed token and session storage
- axum middleware and route handlers
- Upstream OAuth manager, cache, and runtime helpers

**Why separate from `labby-apis`**: Depends on `axum`, which is forbidden in the pure SDK crate.

**Source**: [`crates/labby-auth/`](../crates/labby-auth/)

### `labby-runtime` (Surface-Neutral Contracts)

**Purpose**: Shared contracts and helpers used across product and extracted runtime crates.

**Contents**:
- `ToolError` - Standardized tool error types
- Gateway config DTOs
- Redaction and path-safety helpers
- Backoff/jitter helpers
- Feature-gated pure DTO dependencies

**What's NOT here**: Dispatch-helper payloads and stdio spawn-guard/SSRF checks live in `labby-gateway` instead to avoid pulling `labby-primitives` into `labby-auth` and `labby-codemode`.

**Source**: [`crates/labby-runtime/`](../crates/labby-runtime/)

### `labby-codemode` (Code Mode Execution)

**Purpose**: Client-neutral Code Mode execution kernel.

**Contents**:
- Javy/QuickJS runner protocol
- Warm runner pool
- Result shaping
- Snippet engine
- TypeScript descriptor generation
- `CodeModeHost` trait for tool injection

**Source**: [`crates/labby-codemode/`](../crates/labby-codemode/)

### `labby-gateway` (MCP Gateway Runtime)

**Purpose**: Reusable gateway runtime for upstream MCP proxy operations.

**Contents**:
- Upstream MCP proxy pools
- Discovery and import orchestration
- Virtual servers
- Protected routes
- Gateway OAuth lifecycle
- Manager state
- Code Mode host adapter
- Gateway-specific `action`/`params` dispatch helpers
- stdio spawn-guard and SSRF security checks

**Constraints**: Does not own product config rendering or `.env` writes; those are injected by the host through `GatewayConfigStore`.

**Source**: [`crates/labby-gateway/`](../crates/labby-gateway/)

### `labby-web` (Static Asset Serving)

**Purpose**: Embedded and filesystem static asset serving for Labby web UI.

**Contents**:
- Embedded asset handlers
- Filesystem asset serving
- Symlink escape defense

**Source**: [`crates/labby-web/`](../crates/labby-web/)

### `labby-winjob` (Windows Process Helper)

**Purpose**: Windows Job Object helper for process-tree reaping.

**Contents**:
- Platform FFI for Job Objects
- Process-tree management on Windows

**Why separate**: Contains `unsafe` code, allowing the rest of the workspace to use `unsafe_code = "forbid"`.

**Source**: [`crates/labby-winjob/`](../crates/labby-winjob/)

### `labby-openapi` (OpenAPI Generation)

**Purpose**: OpenAPI specification generation from HTTP routes.

**Source**: [`crates/labby-openapi/`](../crates/labby-openapi/)

### `labby` (Product Binary)

**Purpose**: Main product binary composing all surfaces and services.

**Contents**:
- [`entrypoint.rs`](../crates/labby/src/entrypoint.rs) - Binary entry point, tracing initialization
- [`cli.rs`](../crates/labby/src/cli.rs) - CLI command definitions (clap-based)
- [`api/`](../crates/labby/src/api/) - HTTP API routes and handlers (axum-based)
- [`mcp/`](../crates/labby/src/mcp/) - MCP server implementation (rmcp 2.1)
- [`dispatch/`](../crates/labby/src/dispatch/) - Service-neutral operation handlers
- [`config.rs`](../crates/labby/src/config.rs) - Configuration loading and rendering
- Surface-specific modules (CLI shims, MCP handlers, HTTP middleware)

**Build assumptions**: Developed and verified as an all-features binary. Feature-slice builds (gateway, fs) are supported for CI validation but not the primary development target.

**Source**: [`crates/labby/`](../crates/labby/)

### `xtask` (Dev Utilities)

**Purpose**: Development and maintenance tasks.

**Contents**:
- Code Mode runner benchmarks
- Crate extraction utilities
- Documentation generation helpers

**Source**: [`crates/xtask/`](../crates/xtask/)

## Product Surfaces

The `labby` binary exposes the same capabilities through four surfaces:

### 1. CLI (Command-Line Interface)

- **Framework**: `clap` (derive API)
- **Entry**: [`crates/labby/src/cli.rs`](../crates/labby/src/cli.rs)
- **Pattern**: Thin shims that parse args, call service clients or local subsystems, and format output
- **Output format**: Human-readable tables or JSON (via `--json` flag)

**Key commands**:
- `labby serve` - Start MCP server
- `labby mcp` - Start MCP server over stdio
- `labby doctor` - Service health checks
- `labby setup` - Incus bootstrap
- Gateway management commands (see [docs/surfaces/CLI.md](../docs/surfaces/CLI.md))

### 2. MCP Server (Model Context Protocol)

- **Framework**: `rmcp` 2.1
- **Transports**: stdio and streamable HTTP
- **Entry**: [`crates/labby/src/mcp/`](../crates/labby/src/mcp/)
- **Pattern**: One tool per service with `action` + `params` shape
- **Capabilities**: Tools, resources, prompts, completions, logging

**Key features**:
- Upstream MCP proxy and pooling
- Tool/resource/prompt inspection
- Exposure filters and protected routes
- Code Mode execution integration
- Gateway-managed OAuth

See [docs/surfaces/MCP.md](../docs/surfaces/MCP.md) for MCP surface details.

### 3. HTTP API (RESTful JSON)

- **Framework**: `axum` 0.8
- **Entry**: [`crates/labby/src/api/`](../crates/labby/src/api/)
- **Middleware stack**: Auth, logging, CORS, compression
- **Routes**: `/v1/*` for API, `/` for web UI, `/health` for health checks

**Key endpoints**:
- `/v1/gateway/*` - Gateway management
- `/v1/doctor` - Service health
- `/v1/setup` - Configuration management
- `/v1/snippets` - Code snippets
- `/v1/openapi.json` - OpenAPI spec
- OAuth routes at `/oauth/*`

### 4. Labby Web UI

- **Framework**: React (served via `labby-web`)
- **Serving**: Embedded assets or filesystem path
- **Entry**: [`crates/labby/src/api/web.rs`](../crates/labby/src/api/web.rs)
- **Security**: Symlink escape defense

## Service Dispatch Layer

The dispatch layer ([`crates/labby/src/dispatch/`](../crates/labby/src/dispatch/)) provides surface-neutral service implementations:

- **Pattern**: One service module per domain (e.g., `gateway/`, `fs/`, `doctor/`)
- **Contract**: Each service exposes `catalog.rs` (metadata), `params.rs` (input types), and operation handlers
- **Invocation**: CLI shims, MCP tools, and HTTP routes all call into the same dispatch handlers
- **Ownership**: Dispatch owns business logic; surfaces own parsing/serialization/UX

**Service modules** (current product surface):
- [`gateway/`](../crates/labby/src/dispatch/gateway/) - Gateway operations
- [`fs/`](../crates/labby/src/dispatch/fs/) - Filesystem operations
- [`doctor/`](../crates/labby/src/dispatch/doctor/) - Health checks
- [`setup/`](../crates/labby/src/dispatch/setup/) - Bootstrap configuration
- [`snippets/`](../crates/labby/src/dispatch/snippets/) - Code snippet catalog

**Extracted services** (moved to `labby-runtime`/`labby-apis`):
- Marketplace, deploy, ACP, nodes, stash, logs

See [docs/dev/DISPATCH.md](../docs/dev/DISPATCH.md) for dispatch layer rules.

## Feature Slices and Build Variants

The repository supports feature-gated build slices:

### Current Product Slices (in `labby` crate)

- **`gateway`** - MCP gateway operations (default)
- **`fs`** - Filesystem operations (default)

### Always-On Bootstrap Services

- **`doctor`** - Service health checks (not gated)
- **`setup`** - Bootstrap configuration (not gated)
- **`snippets`** - Code snippet catalog (gateway-gated)

### Extracted Slices (CI-only compile checks)

- **`acp`**, **`nodes`**, **`stash`** - Feature-gated base capabilities verified in CI but not supported as standalone product slices

### Development and Release

- **Default build**: `--all-features` (verified in CI)
- **Slice builds**: Used for CI compile checks to catch accidental cross-slice coupling
- **Release**: Single all-features binary

See [CLAUDE.md](../CLAUDE.md) for build assumptions and slice usage rules.

## Dependency Graph (Simplified)

```
labby-primitives (leaf)
    ↑
    ├── labby-apis (SDK layer)
    │     ↑
    │     └── labby (product binary)
    │
    ├── labby-runtime (surface-neutral)
    │     ↑
    │     ├── labby-gateway
    │     │     ↑
    │     │     └── labby
    │     │
    │     ├── labby-auth
    │     │     ↑
    │     │     └── labby
    │     │
    │     └── labby-codemode
    │           ↑
    │           └── labby-gateway → labby
    │
    └── labby-gateway (direct dependency on primitives for SSRF checks)
          ↑
          └── labby
```

## Recent Architectural Changes

### "Slim labby gateway host" (July 2026)

**Commit**: f42004b041bbb19fa8983c88de69c89852ed5189

**What changed**:
- Removed `marketplace`, `deploy`, `acp_registry`, `acp`, `nodes`, `stash`, `logs` features from main `labby` crate
- These features now exist only in `labby-runtime`/`labby-apis` as extracted crate slices
- Current standalone product slices: `gateway`, `fs`
- Always-on bootstrap services: `doctor`, `setup`, `snippets`

**Why**:
- Simplify the main `labby` binary to focus on gateway operations
- Enable extraction of major subsystems as reusable runtime packages
- Reduce binary size and attack surface for gateway-only deployments

**Implications**:
- Docs and CI matrix updated to remove references to deleted slices
- Feature-gated API routes split (web.rs now has `#[cfg(feature = "web-ui")]` stub)

## Architectural Decisions

For detailed architecture decision records, see [docs/adr/](../docs/adr/).

Key patterns:
- **SDK purity** - `labby-apis` has no dependency on product machinery
- **Extraction target** - Crates designed for reuse as standalone packages
- **Dispatch ownership** - Service-neutral handlers shared across all surfaces
- **Feature gating** - Build slices for different deployment scenarios
- **Auth separation** - `labby-auth` isolated to avoid axum dependency in SDK

See [docs/ARCH.md](../docs/ARCH.md) for the original architecture documentation.
