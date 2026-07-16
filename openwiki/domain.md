# Domain Concepts

This document explains the core domain concepts and business logic in Lab: how the MCP gateway works, how Code Mode executes code, how services are dispatched, and how configuration and authentication flow.

## MCP Gateway Model

Lab's primary domain is the MCP (Model Context Protocol) gateway: a proxy that connects to upstream MCP servers, inspects their capabilities, applies exposure filters, and publishes protected routes.

### Upstream Connection Model

**Transport types**:
- **stdio** - Spawn upstream as child process, communicate over stdin/stdout
- **HTTP (streamable)** - Connect to upstream HTTP MCP server via reqwest

**Connection lifecycle** ([`labby-gateway/src/upstream/`](../crates/labby-gateway/src/upstream/)):
1. **Discovery** - Import upstream by URL or command, inspect capabilities
2. **Pool management** - Maintain connection pool with circuit breaker
3. **Virtual server** - Expose filtered tools/resources/prompts as virtual MCP server
4. **Protected routes** - OAuth-protected HTTP routes for upstream access
5. **Resource proxying** - Intercept and proxy resource reads with auth checks

### Capability Inspection

When connecting to an upstream MCP server, Lab inspects:

- **Tools** - Callable operations with input/output schemas
- **Resources** - Readable data endpoints with URI patterns
- **Prompts** - Template-based message generators
- **Completions** - Auto-complete suggestions for tools/resources

The inspection uses MCP's `list_tools`, `list_resources`, `list_prompts`, and `complete` methods. Results are cached in the gateway catalog.

### Exposure Filtering

Lab applies exposure filters to upstream capabilities:

**Filter types**:
- **Allow/deny lists** - Explicit include/exclude patterns for tool/resource IDs
- **SSRF protection** - URL-based resources checked against static allowlist
- **Path safety checks** - Filesystem tools validated against safe paths

**Implementation** ([`crates/labby-gateway/src/upstream/`](../crates/labby-gateway/src/upstream/)):
- Static SSRF checks in `labby-primitives`
- Path safety helpers in `labby-runtime`
- stdio spawn guard prevents arbitrary process execution

### Gateway OAuth

Lab can manage OAuth for upstream MCP servers that require authentication:

**OAuth flow** ([`docs/runtime/OAUTH.md`](../docs/runtime/OAUTH.md)):
1. **Upstream OAuth manager** - Maintains token cache and refresh logic
2. **Gateway-managed routes** - Protected HTTP routes (`/v1/gateway/upstreams/:id/*`)
3. **Callback forwarding** - OAuth callbacks forwarded through Lab to upstream
4. **JWKS endpoint** - Public keys for JWT verification at `/.well-known/jwks.json`

**Auth modes**:
- **Static bearer** - Pre-shared bearer token (compatibility mode)
- **Google OAuth** - Internal Google OIDC provider
- **Lab-issued JWT** - RS256-signed tokens (default for web UI)

See [docs/runtime/OAUTH.md](../docs/runtime/OAUTH.md) for OAuth architecture.

### Code Mode Integration

The gateway can collapse upstream tool catalogs into Code Mode `search` and `execute` operations:

**Purpose**: Reduce API surface for LLMs by exposing two high-level tools instead of hundreds of individual tools.

**Implementation** ([`crates/labby-gateway/src/gateway/code_mode/`](../crates/labby-gateway/src/gateway/code_mode/)):
- Catalog cache tracks upstream tools
- Code Mode host adapter injects tool calling capabilities
- Upstream tools wrapped as executable JavaScript functions

## Code Mode Execution

Code Mode is Lab's JavaScript execution environment for agent tooling. It uses Javy/QuickJS to run user code in a sandboxed WASM runtime.

### Runner Protocol

**Execution flow** ([`crates/labby-codemode/`](../crates/labby-codemode/)):
1. **Prologue injection** - Add helper functions and imports to user code
2. **Normalization** - Standardize function exports and handle statements
3. **Runner pool** - Warm Javy instance pool for low-latency execution
4. **Result shaping** - Format stdout/stderr/return values as structured output
5. **Descriptor generation** - TypeScript types for tool completion

### Security Model

**Sandbox boundaries**:
- **WASM runtime** - Code runs in Javy/QuickJS sandbox
- **Timeout enforcement** - Configurable execution timeout
- **No filesystem access** - Code cannot read/write filesystem directly
- **Tool injection** - Controlled via `CodeModeHost` trait

**Tool injection** ([`crates/labby-codemode/src/protocol.rs`](../crates/labby-codemode/src/protocol.rs)):
- Host implements `CodeModeHost` to provide tools
- Tools exposed as global functions in JS runtime
- Tool calls marshaled back to host for execution

### Snippet Engine

Code Mode includes a snippet catalog for reusable code fragments:

**Implementation** ([`crates/labby/src/dispatch/snippets/`](../crates/labby/src/dispatch/snippets/)):
- Snippet metadata and storage
- Search and retrieval by tag/name
- Execution via Code Mode runner

## Service Dispatch Pattern

Lab uses a surface-neutral dispatch layer: CLI, MCP, and HTTP all call the same service handlers.

### Dispatch Contract

**Service module layout** ([`docs/dev/DISPATCH.md`](../docs/dev/DISPATCH.md)):
```
dispatch/
  <service>/
    catalog.rs     # ActionSpec, PluginMeta
    params.rs      # Input types
    dispatch.rs   # Operation handlers
    client.rs     # Upstream client (if applicable)
```

**Operation handlers**:
- **Synchronous** - Simple I/O operations (e.g., FS reads)
- **Async** - HTTP client calls or long-running operations
- **Batch** - Multiple operations in one request

**Error handling**:
- Handlers return `Result<Output, ToolError>`
- `ToolError` serialized consistently across surfaces
- See [docs/dev/ERRORS.md](../docs/dev/ERRORS.md) for error taxonomy

### MCP Tool Mapping

Each runtime service exposes exactly **one MCP tool**. The tool accepts a dotted
`action` plus a free-form `params` object and dispatches through the shared
catalog:

**Tool shape** ([`crates/labby/src/mcp/`](../crates/labby/src/mcp/)):
```json
{
  "name": "<service>",
  "inputSchema": {
    "type": "object",
    "properties": {
      "action": { "type": "string", "example": "resource.verb" },
      "params": { "$ref": "#/definitions/Params" }
    }
  }
}
```

**Rationale**:
- Reduces tool catalog bloat (dozens of tools instead of hundreds)
- Consistent pattern across services
- Easier to maintain and extend

### CLI Command Mapping

CLI commands are thin shims over dispatch handlers:

**Pattern** ([`crates/labby/src/cli/`](../crates/labby/src/cli/)):
```rust
pub struct XxxArgs {
    // clap-derived fields
}

impl XxxArgs {
    pub async fn run(self, client: &HttpClient) -> Result<()> {
        let output = dispatch::xxx::run(&self.params).await?;
        // format and print output
        Ok(())
    }
}
```

**Output formats**:
- **Human** - Tables, progress bars, styled text (via `indicatif`, `console`)
- **JSON** - Structured output (via `--json` flag)

### HTTP Route Mapping

HTTP routes map to dispatch handlers via axum handlers:

**Pattern** ([`crates/labby/src/api/`](../crates/labby/src/api/)):
```rust
async fn xxx_handler(
    State(client): State<Arc<HttpClient>>,
    Json(params): Json<XxxParams>,
) -> Result<Json<XxxOutput>, ApiError> {
    let output = dispatch::xxx::run(&client, &params).await?;
    Ok(Json(output))
}
```

**Middleware stack**:
- Auth (JWT or bearer token)
- Logging and tracing
- CORS (if enabled)
- Compression (if enabled)

## Configuration Model

Lab uses a layered configuration system with precedence rules.

### Configuration Layers (in order of precedence)

1. **CLI flags** - Command-line arguments (highest priority)
2. **Environment variables** - `LAB_*` env vars
3. **`.env` file** - Loaded via dotenvy
4. **`config.toml`** - Main configuration file
5. **Defaults** - Hardcoded fallbacks (lowest priority)

### Config Structure

**Main config** ([`crates/labby/src/config.rs`](../crates/labby/src/config.rs)):
```toml
[log]
filter = "labby=info,rmcp=warn"

[server]
host = "127.0.0.1"
port = 8765

[gateway]
# Gateway upstreams and settings

[oauth]
# OAuth provider settings
```

**Environment mapping**:
- `LABBY_LOG` - overrides `[log].filter`
- `LABBY_MCP_HTTP_HOST` - overrides the HTTP bind host
- `LABBY_MCP_HTTP_PORT` - overrides the HTTP bind port
- See [`.env.example`](../.env.example) for full list

### Gateway Config Store

Gateway configuration is injected via `GatewayConfigStore` trait:

**Purpose**: Allow `labby-gateway` crate to be used without pulling in product config machinery.

**Implementation** ([`crates/labby-gateway/src/gateway/manager/`](../crates/labby-gateway/src/gateway/manager/)):
- Host implements `GatewayConfigStore` to render config
- Mutating gateway actions persist through the store and explicitly reload the
  affected runtime state
- Supports dynamic reconfiguration through `gateway.reload`

## Authentication and Authorization

Lab supports multiple authentication modes for different surfaces.

### HTTP API Auth

**Modes** ([`docs/runtime/OAUTH.md`](../docs/runtime/OAUTH.md)):

1. **Static bearer token** - Pre-shared token (compatibility)
2. **Google OAuth** - OIDC flow with Google provider
3. **Lab-issued JWT** - RS256-signed tokens (default)

**JWT flow**:
1. User initiates OAuth via Google
2. Lab validates ID token, creates session
3. Lab issues JWT signed with Lab's RSA key
4. Client includes JWT in `Authorization: Bearer <token>` header
5. axum middleware validates JWT signature and claims
6. Request proceeds to handler

**Token storage**:
- SQLite database ([`labby-auth`](../crates/labby-auth/))
- Tables: `sessions`, `tokens`, `auth_codes`

### MCP Transport Auth

**stdio transport**:
- No auth (local execution only)
- Caller controls stdio, no remote access

**HTTP transport**:
- Bearer token or JWT via `Authorization` header
- Same validation as HTTP API

### Upstream OAuth

When proxying upstream MCP servers that require OAuth:

**Gateway-managed OAuth**:
1. Lab maintains OAuth session for upstream
2. Lab's gateway acts as OAuth client
3. Callbacks forwarded through Lab
4. Access token cached and refreshed as needed
5. Upstream requests made with cached token

See [docs/runtime/OAUTH.md](../docs/runtime/OAUTH.md) for upstream OAuth details.

## Registry and Marketplace

Lab includes a registry and marketplace for discovering and installing MCP servers, plugins, and ACP providers.

### MCP Registry

**Source**: Official MCP Registry (mirrored)

**Metadata enhancement** ([`docs/services/MCPREGISTRY_METADATA.md`](../docs/services/MCPREGISTRY_METADATA.md)):
- Lab-owned metadata layered onto registry entries
- Installation guidance and compatibility notes
- Categorized browsing (devtools, productivity, homelab)

### Marketplace Integration

**Supported marketplaces**:
- Claude plugin marketplace
- Codex plugin marketplace
- MCP Registry
- ACP Agent Registry

**Operations** (now in `labby-runtime`/`labby-apis`):
- Browse and search marketplace entries
- Install plugins/servers to local stash
- Version management and updates
- Deployment to configured targets

See [PLUGINS.md](../docs/PLUGINS.md) for plugin architecture.

## Observability and Logging

Lab has a structured observability model for request tracing, error handling, and verification.

### Logging Model

**Framework**: `tracing` and `tracing-subscriber`

**Log levels** ([`docs/dev/OBSERVABILITY.md`](../docs/dev/OBSERVABILITY.md)):
- `ERROR` - Failures requiring operator intervention
- `WARN` - Unexpected but non-fatal conditions
- `INFO` - High-level operation milestones
- `DEBUG` - Detailed execution flow
- `TRACE` - Fine-grained diagnostics

**Targets** (crate/module granularity):
- `labby` - Main product binary
- `labby::*` - Product modules (cli, api, mcp, dispatch)
- `labby_auth` - Auth middleware
- `labby_gateway` - Gateway runtime
- `rmcp` - MCP SDK

**Output formats**:
- **Console** - Human-readable with colors (when TTY)
- **JSON** - Structured logs (when `LABBY_LOG_FORMAT=json`)

### Request Tracing

**Correlation**:
- Each request gets unique trace ID
- Trace ID propagated across async tasks
- Included in log records and error responses

**Boundaries** ([`docs/dev/OBSERVABILITY.md`](../docs/dev/OBSERVABILITY.md)):
- HTTP requests: trace ID from request start
- MCP tool calls: trace ID from tool invocation
- CLI commands: trace ID from command start

### Error Handling

**Error taxonomy** ([`docs/dev/ERRORS.md`](../docs/dev/ERRORS.md)):
- `ToolError` - Structured service errors
- `ApiError` - HTTP surface errors with status codes
- `CliError` - CLI surface errors with user-friendly messages

**Status code mapping**:
- 400 - Bad request (invalid params)
- 401 - Unauthorized (missing/invalid token)
- 403 - Forbidden (insufficient permissions)
- 404 - Not found (missing resource)
- 409 - Conflict (state mismatch)
- 429 - Rate limited (circuit breaker)
- 500 - Internal error (unexpected failure)

**Redaction**:
- Secrets redacted from logs and error messages
- Path safety checks applied to file paths
- URL query params redacted (except allowlist)

## Feature Flags and Build Variants

Lab supports feature-gated builds for different deployment scenarios.

### Feature Slices

**Current slices** (post-refactoring):
- `gateway` - MCP gateway operations
- `fs` - Filesystem operations

**Always-on services** (not gated):
- `doctor` - Health checks
- `setup` - Bootstrap
- `snippets` - Code catalog

**Extracted slices** (CI-only):
- `acp`, `nodes`, `stash` - Verified in CI but not standalone

### Build Matrix

**All-features build** (default):
```bash
cargo build --all-features
cargo nextest run --all-features
```

**Slice builds** (CI validation):
```bash
cargo build --no-default-features --features gateway
cargo build --no-default-features --features fs
```

**Release build**:
- Single all-features binary
- Optimized for size and performance

See [CLAUDE.md](../CLAUDE.md) for build assumptions.

## Data Persistence

Lab uses SQLite for persistent storage and file-based caching for catalogs.

### SQLite Storage

**Database location**:
- Path: `~/.labby/store/labby.sqlite`
- Managed by `labby-auth` crate

**Tables**:
- `sessions` - OAuth sessions and state
- `tokens` - Access and refresh tokens
- `auth_codes` - OAuth authorization codes

### Catalog Caching

**Gateway catalog**:
- In-memory cache of upstream capabilities
- Persisted to `~/.labby/cache/`
- Refreshed on upstream reconnection

**Snippet catalog**:
- File-based storage in `~/.labby/stash/`
- Version control friendly

### Log Storage

**Rolling log files**:
- Path: `~/.local/share/labby/logs/`
- Rotation: Daily
- Retention: 7 days
- Managed by `tracing-appender`

## Extension Points

Lab provides several extension points for adding capabilities.

### Service Onboarding

**Process** ([`docs/dev/SERVICE_ONBOARDING.md`](../docs/dev/SERVICE_ONBOARDING.md)):
1. Create service module in `dispatch/`
2. Implement `catalog.rs`, `params.rs`, `dispatch.rs`
3. Add CLI command in `cli/`
4. Add MCP tool in `mcp/`
5. Add HTTP route in `api/`
6. Update generated docs

**Contract**:
- Service must implement `catalog()` returning `ActionSpec`
- Handlers must accept `Params` and return `Result<Output, ToolError>`
- Errors must follow shared taxonomy

### Upstream MCP Clients

**Custom upstreams**:
- Implement MCP client protocol
- Register with gateway pool
- Expose tools/resources/prompts

### Code Mode Tools

**Tool injection**:
- Implement `CodeModeHost` trait
- Register tools with runner
- Tools available as global JS functions

See [docs/dev/SERVICE_ONBOARDING.md](../docs/dev/SERVICE_ONBOARDING.md) for detailed guidance.

## Related Documentation

- **[Architecture](architecture.md)** - Crate structure and dependencies
- **[Development Guide](development.md)** - Build, test, and contribution rules
- **[docs/runtime/CONFIG.md](../docs/runtime/CONFIG.md)** - Full configuration reference
- **[docs/runtime/OAUTH.md](../docs/runtime/OAUTH.md)** - OAuth architecture details
- **[docs/surfaces/MCP.md](../docs/surfaces/MCP.md)** - MCP surface specification
- **[docs/dev/DISPATCH.md](../docs/dev/DISPATCH.md)** - Dispatch layer contract
- **[docs/dev/OBSERVABILITY.md](../docs/dev/OBSERVABILITY.md)** - Logging and tracing requirements
