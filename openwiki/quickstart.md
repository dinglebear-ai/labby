# OpenWiki Quickstart

**Lab** is a pluggable homelab CLI and MCP server implemented as a Rust workspace. The `labby` binary exposes operator capabilities through four surfaces: a CLI, an MCP server, an HTTP API, and the Labby web UI.

## What is Lab?

Lab is a local-first control plane for agent tooling and homelab operations. One binary (`labby`) provides:

- **MCP gateway** - Connect HTTP and stdio upstream MCP servers, inspect their tools/resources/prompts, apply exposure filters, and publish protected MCP routes
- **Registry and marketplace** - Browse Claude/Codex plugin marketplaces and the official MCP Registry
- **Generated discovery** - Auto-publish service catalogs, API routes, OpenAPI specs, and feature matrices

Lab underwent a major architectural refactoring in early 2026 ("Slim labby gateway host" pass) that extracted several major subsystems (marketplace, deploy, ACP chat, nodes, stash, logs) into separate runtime crates. The main `labby` binary now focuses on gateway operations, filesystem access, and bootstrap services.

## Quick Start

### Install a Release

Linux/macOS:
```bash
curl -fsSL https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh | sh
labby setup
labby serve --host 127.0.0.1 --port 8765
```

Windows PowerShell:
```powershell
irm https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.ps1 | iex
labby setup
labby serve --host 127.0.0.1 --port 8765
```

### Build from Source

Prerequisites:
- Rust 1.92 or newer (verified with 1.94.1)
- `just` for repo commands
- `cargo-nextest` for tests
- `pnpm 9.15.9` for the Labby web UI

```bash
git clone https://github.com/jmagar/lab.git
cd lab
just install
just web-build
labby serve --host 127.0.0.1 --port 8765
```

## Repository Structure

The repository is a Cargo workspace split into reusable crates plus one product binary:

- **`crates/labby/`** - Main product binary with CLI, MCP, HTTP API, and product dispatch
- **`crates/labby-apis/`** - Pure SDK layer with typed service clients and request/response models
- **`crates/labby-auth/`** - OAuth 2.0 and JWT auth middleware (axum-based)
- **`crates/labby-codemode/`** - Code Mode execution kernel (Javy/QuickJS runner)
- **`crates/labby-gateway/`** - MCP gateway runtime, proxy pools, discovery, and virtual servers
- **`crates/labby-runtime/`** - Surface-neutral contracts (ToolError, gateway DTOs, redaction)
- **`crates/labby-primitives/`** - Dependency-free shared types (ActionSpec, PluginMeta, UiSchema)
- **`crates/labby-web/`** - Embedded/static asset serving for Labby web UI
- **`crates/labby-winjob/`** - Windows Job Object helper for process-tree reaping
- **`crates/labby-openapi/`** - OpenAPI spec generation
- **`crates/xtask/`** - Development utilities

See [Architecture](architecture.md) for details on the crate split and dependency relationships.

## Product Surfaces

The `labby` binary exposes the same operator capabilities through four surfaces:

1. **CLI** - Human-readable command-line interface powered by clap
2. **MCP server** - Model Context Protocol server (stdio and HTTP transport) using rmcp 2.1
3. **HTTP API** - RESTful JSON API powered by axum
4. **Labby web UI** - Browser-based interface (served via labby-web)

All surfaces share the same underlying service dispatch layer. See [Domain Concepts](domain.md) for details on the MCP model, Code Mode execution, and service dispatch patterns.

## Core Services

The current `labby` product surface focuses on these services (post-refactoring):

- **Gateway** - MCP upstream proxy, routing, OAuth, and virtual servers
- **FS** - Filesystem operations (always-on bootstrap service)
- **Doctor** - Service health checks and diagnostics
- **Setup** - Incus container bootstrap for gateway hosting
- **Snippets** - Code snippet catalog and execution

Several major subsystems were extracted to separate crates in the "Slim labby gateway host" refactoring:
- Marketplace, Deploy, ACP chat, Nodes, Stash, and Logs now live in `labby-runtime`/`labby-apis`

## Configuration

Lab uses a layered configuration system:

1. **`config.toml`** - Main configuration file (see [docs/runtime/CONFIG.md](../docs/runtime/CONFIG.md))
2. **`.env` file** - Environment variables loaded via dotenvy (see [`.env.example`](../.env.example))
3. **SQLite storage** - Auth tokens, sessions, and OAuth state

Key configuration areas:
- Gateway upstreams and OAuth providers
- Transport settings (stdio vs HTTP, ports, hosts)
- Logging and observability
- Feature flags and build variants

See [Domain Concepts](domain.md) for the configuration model and auth architecture.

## Development

For contributors working on Lab code:

- Start with [Development Guide](development.md) for build/test commands and code organization
- Review [docs/dev/OBSERVABILITY.md](../docs/dev/OBSERVABILITY.md) for logging and tracing requirements
- Read [docs/dev/DISPATCH.md](../docs/dev/DISPATCH.md) for service layer rules
- Consult [docs/dev/SERVICE_ONBOARDING.md](../docs/dev/SERVICE_ONBOARDING.md) when adding new services

The repo uses:
- Rust 2024 edition with workspace resolver version 3
- `tokio` for async runtime
- `axum` for HTTP server
- `rmcp 2.1` for MCP transport
- `clap` for CLI parsing

## Documentation Structure

The existing docs in [docs/](../docs/README.md) are the source of truth. This OpenWiki provides a navigable overview for humans and agents:

- **[Architecture](architecture.md)** - Crate responsibilities, workspace structure, dependency graph
- **[Domain Concepts](domain.md)** - MCP gateway model, Code Mode, service dispatch, configuration
- **[Development Guide](development.md)** - Build/test commands, code organization, contribution rules

## Key Source References

- **Binary entry**: [`crates/labby/src/entrypoint.rs`](../crates/labby/src/entrypoint.rs)
- **CLI commands**: [`crates/labby/src/cli.rs`](../crates/labby/src/cli.rs)
- **Service dispatch**: [`crates/labby/src/dispatch/`](../crates/labby/src/dispatch/)
- **Gateway runtime**: [`crates/labby-gateway/src/`](../crates/labby-gateway/src/)
- **MCP server**: [`crates/labby/src/mcp/`](../crates/labby/src/mcp/)
- **HTTP API**: [`crates/labby/src/api/`](../crates/labby/src/api/)
- **Config loading**: [`crates/labby/src/config.rs`](../crates/labby/src/config.rs)

## Recent Changes

The "Slim labby gateway host" refactoring (commit f42004b0, July 2026) removed these features from the main `labby` crate:
- `marketplace`, `deploy`, `acp_registry`, `acp`, `nodes`, `stash`, `logs` features
- These now exist only in `labby-runtime`/`labby-apis` as extracted crate slices
- Current standalone product slices: `gateway`, `fs`
- Always-on bootstrap services: `doctor`, `setup`, `snippets`

When working with the codebase, assume all features are enabled by default. The repo is developed and verified as an all-features binary.

## Next Steps

- **New contributors**: Read [Architecture](architecture.md), then [Domain Concepts](domain.md)
- **Feature work**: Start with [Development Guide](development.md), then consult service-specific docs
- **Deployment**: See [docs/runtime/CONFIG.md](../docs/runtime/CONFIG.md) and [docs/runtime/OPERATIONS.md](../docs/runtime/OPERATIONS.md)
- **MCP integration**: See [docs/surfaces/MCP.md](../docs/surfaces/MCP.md) and [docs/services/GATEWAY.md](../docs/services/GATEWAY.md)
