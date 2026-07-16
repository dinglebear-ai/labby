# Development Guide

This guide covers how to build, test, and contribute to Lab. It assumes you're working with the Rust workspace and have the necessary prerequisites installed.

## Prerequisites

### Required Tools

- **Rust** 1.92 or newer (verified with 1.94.1 in CI)
- **cargo** - Rust package manager (included with Rust)
- **just** - Task runner (installed via `cargo install just`)
- **pnpm** 9.15.9 - For Labby web UI (pinned in [`.mise.toml`](../.mise.toml))
- **cargo-nextest** - Test runner (installed via `cargo install cargo-nextest`)

### Optional Tools

- **openssl** - For manual bearer token generation
- **docker** - For containerized testing (Incus support)

### Platform Support

- **Linux** (x86_64, aarch64) - Primary development platform
- **macOS** (x86_64, aarch64) - Supported
- **Windows** - Supported (via `labby-winjob` for process reaping)

## Build Commands

### Build All Features (Default)

```bash
cargo build --all-features
```

This is the primary build target. The repo is developed and verified as an all-features binary.

### Build Specific Feature Slices

```bash
# Gateway-only build
cargo build --no-default-features --features gateway

# Filesystem-only build
cargo build --no-default-features --features fs
```

These are used for CI validation to catch accidental cross-slice coupling, not for development.

### Development Build

```bash
# Build with dev optimizations
cargo build --all-features

# Build with full optimizations
cargo build --all-features --release
```

### Web UI Build

```bash
# Build Labby web UI
pnpm install
pnpm --filter gateway-admin run build
```

Or use the just command:
```bash
just web-build
```

## Test Commands

### Run All Tests

```bash
cargo nextest run --all-features
```

The repo uses `cargo-nextest` for parallel test execution.

### Run Specific Tests

```bash
# Run tests in a specific crate
cargo nextest run --package labby --all-features

# Run a specific test
cargo nextest run --all-features test_name

# Run tests matching a pattern
cargo nextest run --all-features gateway
```

### Test Organization

**Integration tests** ([`crates/labby/tests/`](../crates/labby/tests/)):
- `architecture_boundaries.rs` - Crate dependency validation
- `architecture_orchestrator.rs` - Service orchestration
- `auth_admin_api.rs` - Auth and admin API tests
- `code_mode_full_stack.rs` - Code Mode end-to-end
- `code_mode_runner.rs` - Code Mode runner tests
- `gateway_stdio_spawn.rs` - stdio transport tests
- `observability.rs` - Logging and tracing verification
- `upstream_oauth.rs` - OAuth flow tests

**Unit tests**: Co-located with source code in `*/tests.rs` modules.

### CI Test Matrix

The CI runs tests across multiple feature slices:

- **all-features** - Full test suite
- **gateway** - Gateway-specific tests
- **fs** - Filesystem-specific tests
- **acp**, **nodes**, **stash** - Compile-check only (extracted slices)

See [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) for the full matrix.

## Code Organization

### Module Structure

**Main binary** ([`crates/labby/src/`](../crates/labby/src/)):
- `entrypoint.rs` - Binary entry, tracing init
- `cli.rs` - CLI command definitions
- `config.rs` - Configuration loading
- `cli/` - CLI command handlers (thin shims)
- `api/` - HTTP API routes and handlers
- `mcp/` - MCP server implementation
- `dispatch/` - Service-neutral operation handlers
- `observability/` - Logging and tracing setup

**Service modules** ([`crates/labby/src/dispatch/`](../crates/labby/src/dispatch/)):
- `gateway/` - Gateway operations
- `fs/` - Filesystem operations
- `doctor/` - Health checks
- `setup/` - Bootstrap configuration
- `snippets/` - Code snippet catalog

**Reusable crates**:
- [`crates/labby-apis/`](../crates/labby-apis/) - SDK layer
- [`crates/labby-auth/`](../crates/labby-auth/) - Auth middleware
- [`crates/labby-codemode/`](../crates/labby-codemode/) - Code Mode execution
- [`crates/labby-gateway/`](../crates/labby-gateway/) - Gateway runtime
- [`crates/labby-runtime/`](../crates/labby-runtime/) - Shared contracts
- [`crates/labby-primitives/`](../crates/labby-primitives/) - Shared types

### Code Conventions

**Module layout** ([`docs/dev/DISPATCH.md`](../docs/dev/DISPATCH.md)):
- Each service has `catalog.rs`, `params.rs`, `dispatch.rs`, `client.rs` (if applicable)
- CLI commands are thin shims over dispatch handlers
- MCP tools map to dispatch handlers via `action` + `params`
- HTTP routes map to dispatch handlers via axum handlers

**Error handling** ([`docs/dev/ERRORS.md`](../docs/dev/ERRORS.md)):
- Use `ToolError` for service errors
- Use `ApiError` for HTTP surface errors
- Use `anyhow::Result` for CLI errors
- Follow shared error taxonomy

**Logging** ([`docs/dev/OBSERVABILITY.md`](../docs/dev/OBSERVABILITY.md)):
- Use `tracing` macros: `error!`, `warn!`, `info!`, `debug!`, `trace!`
- Include structured fields: `?error`, `?params`, `trace_id`
- Redact secrets and sensitive paths
- Set appropriate targets (`labby::module_name`)

**Serialization** ([`docs/design/SERIALIZATION.md`](../docs/design/SERIALIZATION.md)):
- Use `serde` for all JSON serialization
- Prefer idiomatic serde patterns (derive, `serde_as`)
- Validate at boundaries (CLI, MCP, HTTP)
- Output envelope shapes documented in service specs

## Adding a Service

Follow the service onboarding process in [docs/dev/SERVICE_ONBOARDING.md](../docs/dev/SERVICE_ONBOARDING.md):

1. **Create service module** in `dispatch/<service>/`
2. **Implement catalog** (`catalog.rs`):
   - Define `ActionSpec` with metadata
   - Define `PluginMeta` for registry entry
   - Return via `pub fn catalog() -> ActionSpec`
3. **Define params** (`params.rs`):
   - Struct for input parameters
   - Implement serde `Serialize`/`Deserialize`
4. **Implement handlers** (`dispatch.rs`):
   - Async function accepting `Params`
   - Return `Result<Output, ToolError>`
5. **Add CLI command** in `cli/<service>.rs`:
   - Thin shim parsing args
   - Call dispatch handler
   - Format output
6. **Add MCP tool** in `mcp/`:
   - Register tool with server
   - Map `action` + `params` to handler
7. **Add HTTP route** in `api/` (if applicable):
   - axum handler with auth middleware
   - JSON request/response
8. **Update generated docs**:
   - Run `labby docs generate`
   - Verify artifacts in `docs/generated/`

## Architecture Rules

### SDK Purity

**Rule**: `labby-apis` must not depend on product machinery.

**Enforcement**:
- No axum, clap, or tracing in `labby-apis`
- No `.env` loading in SDK
- No MCP transport in SDK
- HTTP clients only (reqwest)

**Violations**: Caught by `architecture_boundaries.rs` tests.

### Dispatch Ownership

**Rule**: Service logic lives in dispatch, not in surfaces.

**Pattern**:
- CLI shims → dispatch handlers → service clients
- MCP tools → dispatch handlers → service clients
- HTTP routes → dispatch handlers → service clients

**Benefits**:
- Single source of truth for business logic
- Consistent behavior across surfaces
- Easier to test and maintain

### Feature Gating

**Rule**: Use feature flags for build slices, not runtime config.

**Current slices**:
- `gateway` - MCP gateway operations
- `fs` - Filesystem operations
- `web-ui` - Embedded web assets

**Always-on**:
- `doctor` - Health checks
- `setup` - Bootstrap
- `snippets` - Code catalog

**Extracted**:
- `acp`, `nodes`, `stash` - In `labby-runtime`/`labby-apis`

## Documentation

### Generated Docs

Lab auto-generates several documentation artifacts:

**Catalogs** ([`docs/generated/`](../docs/generated/)):
- `service-catalog.md` - All service actions and params
- `action-catalog.md` - Action-level metadata
- `feature-matrix.md` - Feature gate coverage

**API docs**:
- `api-routes.md` - HTTP API routes
- `openapi.json` - OpenAPI specification
- `cli-help.md` - CLI command reference
- `mcp-help.md` - MCP server capabilities

**Environment**:
- `env-reference.md` - Environment variable reference

### Generating Docs

```bash
# Generate all docs
cargo run --package labby --all-features -- docs generate

# Generate specific docs
cargo run --package labby --all-features -- docs check
```

### Manual Docs

**Topic docs** ([`docs/`](../docs/README.md)):
- Architecture, operations, development guides
- Source of truth for system behavior
- Update when changing contracts or behavior

**Code docs**:
- Rustdoc comments on public APIs
- Module-level documentation
- Example usage

## Debugging

### Logging

**Enable debug logging**:
```bash
LABBY_LOG=labby=debug cargo run --package labby --all-features -- serve
```

**JSON logs**:
```bash
LABBY_LOG_FORMAT=json cargo run --package labby --all-features -- serve
```

**Trace logs**:
```bash
LABBY_LOG=labby=trace,rmcp=debug cargo run --package labby --all-features -- serve
```

### MCP Debugging

**stdio transport**:
```bash
labby mcp  # Starts stdio MCP server, logs to stderr
```

**HTTP transport**:
```bash
labby serve --host 127.0.0.1 --port 8765
# Connect via HTTP to http://127.0.0.1:8765/mcp
```

### Code Mode Debugging

**Enable Code Mode tracing**:
```bash
LABBY_LOG=labby_codemode=debug cargo run --package labby --all-features -- snippets test <snippet_id>
```

## Performance

### Benchmarks

**Code Mode runner**:
```bash
cargo run --package xtask -- bench-codemode
```

### Optimization Tips

- **Runner pool** - Keep Javy instances warm for low-latency execution
- **Connection pooling** - Reuse HTTP clients and upstream connections
- **Catalog caching** - Cache upstream capability inspections
- **Lazy loading** - Delay expensive operations until needed

## Security Considerations

### SSRF Protection

**Static checks** ([`crates/labby-primitives/`](../crates/labby-primitives/)):
- URL allowlist for resource endpoints
- Path safety checks for filesystem operations

**Runtime checks**:
- stdio spawn guard prevents arbitrary process execution
- OAuth callback validation

### Secret Redaction

**Automatic redaction**:
- Known secret patterns redacted from logs
- Path safety applied to file paths
- URL query params redacted (except allowlist)

**Manual redaction**:
- Use `?error` (not `{:?}`) to redact error details
- Mark secrets with `#[serde(skip_serializing)]`

### Sandbox Boundaries

**Code Mode**:
- WASM runtime isolates user code
- No filesystem access
- Tool injection controlled via `CodeModeHost`

**Upstream MCP**:
- stdio transport: caller controls process
- HTTP transport: auth required, resource proxying

## Common Tasks

### Add a CLI Command

1. Create `cli/<command>.rs` with clap derive
2. Add to `cli.rs` `Subcommand` enum
3. Implement thin shim over dispatch handler
4. Add tests

### Add an MCP Tool

1. Add tool definition in `mcp/handlers_tools.rs`
2. Map to dispatch handler
3. Add tests in `mcp/handlers_tools/tests.rs`
4. Update generated docs

### Add an HTTP Route

1. Add route in `api/router.rs`
2. Implement handler in `api/<module>.rs`
3. Add auth middleware if needed
4. Add tests
5. Update OpenAPI docs

### Add Configuration

1. Add field to `config::LabConfig` in `config.rs`
2. Add to `config.toml` template
3. Add env var mapping
4. Update `.env.example`
5. Add tests

## CI/CD

### GitHub Actions

**Workflows** ([`.github/workflows/`](../.github/workflows/)):
- `ci.yml` - Build and test matrix
- `build-incus-image.yml` - Incus image builds

**Test matrix**:
- All-features build + test
- Feature slice builds (gateway, fs)
- Compile-check slices (acp, nodes, stash)
- Platform-specific tests (Windows job object)

### Release Process

1. Bump version in `Cargo.toml` (workspace.package.version)
2. Update `CHANGELOG.md`
3. Create git tag
4. GitHub Action builds release assets
5. Upload to GitHub Releases

## Getting Help

### Documentation

- **Start here**: [Quickstart](quickstart.md)
- **Architecture**: [Architecture](architecture.md)
- **Domain**: [Domain Concepts](domain.md)
- **Topic docs**: [docs/README.md](../docs/README.md)

### Nested Guides

Read the nested `CLAUDE.md` files when working in specific areas:
- [`crates/labby/src/CLAUDE.md`](../crates/labby/src/CLAUDE.md) - Main binary rules
- [`crates/labby/src/dispatch/CLAUDE.md`](../crates/labby/src/dispatch/CLAUDE.md) - Dispatch rules
- [`crates/labby-gateway/src/upstream/CLAUDE.md`](../crates/labby-gateway/src/upstream/) - Gateway rules
- [`crates/labby-apis/src/core/CLAUDE.md`](../crates/labby-apis/src/core/) - SDK rules

### Troubleshooting

**Build failures**:
- Check Rust version: `rustc --version`
- Verify features: `cargo build --all-features --dry-run`
- Clean build: `cargo clean`

**Test failures**:
- Run single test: `cargo nextest run --all-features test_name`
- Enable backtraces: `RUST_BACKTRACE=1`
- Check observability tests require strict log validation

**Runtime issues**:
- Enable debug logging: `LABBY_LOG=labby=debug`
- Check config: `labby doctor`
- Verify SQLite: `ls ~/.labby/store/labby.sqlite`

## Related Documentation

- **[Quickstart](quickstart.md)** - Project overview and quick start
- **[Architecture](architecture.md)** - Crate structure and dependencies
- **[Domain Concepts](domain.md)** - MCP gateway, Code Mode, dispatch patterns
- **[docs/CONVENTIONS.md](../docs/CONVENTIONS.md)** - Code conventions and patterns
- **[docs/dev/SERVICE_ONBOARDING.md](../docs/dev/SERVICE_ONBOARDING.md)** - Adding new services
- **[docs/dev/OBSERVABILITY.md](../docs/dev/OBSERVABILITY.md)** - Logging and tracing requirements
- **[docs/dev/ERRORS.md](../docs/dev/ERRORS.md)** - Error handling and taxonomy
