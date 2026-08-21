# Labby Product Documentation

This directory is the canonical documentation entrypoint for the current Labby product.

The live Rust/TypeScript implementation and the generated catalogs under [generated/](./generated/README.md) are the ground truth for what is compiled, registered, and exposed. Product prose should explain that implementation rather than preserve old product shapes.

Historical research, session logs, and implementation plans are intentionally outside the canonical product-doc surface. In particular, `docs/references/`, `docs/sessions/`, and `docs/plans/` are not used to determine current product behavior.

## Start Here

- [Architecture](./ARCH.md) — workspace boundaries, runtime flow, and product surfaces.
- [Technology](./TECH.md) — toolchain, dependencies, build posture, and release model.
- [Conventions](./CONVENTIONS.md) — engineering rules that current code is expected to follow.
- [Service model](./dev/SERVICES.md) — the current registered service inventory and onboarding rules.
- [CLI](./surfaces/CLI.md), [MCP](./surfaces/MCP.md), and [Transport](./surfaces/TRANSPORT.md) — public surface behavior.
- [Configuration](./runtime/CONFIG.md) and [Environment](./runtime/ENV.md) — runtime configuration and environment variables.
- [Operations](./OPERATIONS.md) — build, doctor, deployment, CI, release, and operator workflows.

## Current Product Services

The generated [service catalog](./generated/service-catalog.md) is authoritative. The current product documentation is split by service:

| Service | Product doc | Notes |
| --- | --- | --- |
| `gateway` | [services/GATEWAY.md](./services/GATEWAY.md) | Upstream catalog, protected routes, virtual servers, OAuth, Code Mode host |
| upstream proxy runtime | [services/UPSTREAM.md](./services/UPSTREAM.md) | HTTP/Unix/stdio upstream MCP connections, discovery, filtering, health, OAuth, skills |
| `setup` | [services/SETUP.md](./services/SETUP.md) | Bootstrap, settings, repair, plugin lifecycle, proxy setup, host provisioning |
| `server_logs` | [services/SERVER_LOGS.md](./services/SERVER_LOGS.md) | Labby's own server-process log query and journal tail |
| `fs` | [services/FILESYSTEM.md](./services/FILESYSTEM.md) | Optional jailed read-only workspace browsing and preview |
| `snippets` | [services/SNIPPETS.md](./services/SNIPPETS.md) | Reusable Code Mode workflow storage, validation, execution, testing, promotion |
| `lab_admin` | [services/LAB_ADMIN.md](./services/LAB_ADMIN.md) | Runtime-conditional onboarding audit surface |
| direct stdio proxy | [guides/STDIO_MCP_PROXY.md](./guides/STDIO_MCP_PROXY.md) | One selected stdio MCP server exposed over Streamable HTTP |

Do not hand-maintain a duplicate action inventory in prose. Use the generated [action catalog](./generated/action-catalog.md) for exact action names, parameters, scopes, destructive classification, and surfaces.

## Public Surfaces

- [CLI](./surfaces/CLI.md) — command grammar, output modes, confirmation behavior, and operator commands.
- [MCP](./surfaces/MCP.md) — tool/resource/prompt behavior, Code Mode, MCP Apps, and capability exposure.
- [MCP conformance](./surfaces/MCP_CONFORMANCE.md) — current protocol-version and conformance contract.
- [RMCP](./surfaces/RMCP.md) — how Labby integrates the Rust MCP SDK.
- [Transport](./surfaces/TRANSPORT.md) — stdio, Streamable HTTP, Unix socket, middleware, CORS, DNS-rebinding protection, and subscriptions.
- [TUI](./surfaces/TUI.md) — explicit status of the deferred TUI surface.

## Runtime And Operations

- [Configuration](./runtime/CONFIG.md)
- [Environment](./runtime/ENV.md)
- [OAuth](./runtime/OAUTH.md)
- [OAuth callback relay](./runtime/CALLBACK_RELAY.md)
- [Reverse proxy](./runtime/REVERSE_PROXY.md)
- [Host gateway runtime](./runtime/HOST_GATEWAY.md)
- [Incus](./runtime/INCUS.md)
- [Unraid plugin](./runtime/UNRAID.md)
- [GitHub Actions runner](./runtime/ACTIONS_RUNNER.md)
- [CI/CD](./runtime/CICD.md)
- [Operations](./OPERATIONS.md)
- [Rust build setup](./RUST.md)

## Developer Contracts

- [Dispatch](./dev/DISPATCH.md) — surface-neutral operation ownership and dependency direction.
- [Service model](./dev/SERVICES.md) — service inventory and registration rules.
- [Service onboarding](./dev/SERVICE_ONBOARDING.md) — end-to-end checklist for a new first-class capability.
- [Code Mode](./dev/CODE_MODE.md) — Code Mode runtime and host integration.
- [Errors](./dev/ERRORS.md) — stable error taxonomy and surface mapping.
- [Observability](./dev/OBSERVABILITY.md) — required fields, correlation, redaction, and verification.
- [Testing](./dev/TESTING.md) — local and CI verification expectations.
- [Serialization](./design/SERIALIZATION.md) — output and wire-shape ownership.

Normative cross-surface contracts live under [contracts/](./contracts/):

- [Agent error contract](./contracts/agent-error-contract.md)
- [Code Mode tool errors](./contracts/code-mode-tool-errors.md)
- [MCP tool output](./contracts/mcp-tool-output.md)
- [Gateway schema resources](./contracts/gateway-schema-resources.md)
- [Skills extension](./contracts/skills-extension.md)
- [Stdio MCP proxy](./contracts/stdio-mcp-proxy.md)

## Product Design

- [Design index](./design/README.md)
- [Web design-system contract](./design/design-system-contract.md)
- [Component development](./design/component-development.md)
- [CLI design system](./design/CLI_DESIGN_SYSTEM.md)
- [Claude Code Aurora theme](./design/CLAUDE_CODE_AURORA_THEME.md)
- [Google credential broker](./design/GOOGLE_CREDENTIAL_BROKER.md)
- [Remote gateway target](./design/REMOTE_GATEWAY_TARGET.md)
- [Tool annotations](./design/tool-annotations/README.md)
- [Brand assets](./assets/brand/README.md)

## Plugins And Snippets

- [Plugins](./PLUGINS.md) — checked-in Labby plugin, distribution boundary, and setup lifecycle.
- [Snippet authoring](./snippets/README.md) — executable Code Mode snippet format and workflow.

## Generated Product References

Run:

```bash
just docs-generate
just docs-check
scripts/check-product-docs.py
```

Generated artifacts include:

- [service catalog](./generated/service-catalog.md)
- [action catalog](./generated/action-catalog.md)
- [environment reference](./generated/env-reference.md)
- [proxy configuration reference](./generated/proxy-config-reference.md)
- [API routes](./generated/api-routes.md)
- [MCP help](./generated/mcp-help.md)
- [CLI help](./generated/cli-help.md)
- [feature matrix](./generated/feature-matrix.md)
- `openapi.json`

Never edit generated artifacts by hand.

## Source-Of-Truth Rules

When documentation and implementation disagree:

1. verify the current implementation and generated catalogs;
2. fix the canonical product doc that owns the concern;
3. regenerate code-owned docs when code metadata changed;
4. update cross-links only where the behavior crosses product boundaries.

Avoid creating duplicate top-level summaries for a topic that already has a canonical service, runtime, surface, design, or developer doc.
