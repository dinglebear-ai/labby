# Labby

Labby is a Rust MCP gateway and operator control plane. CLI, MCP, HTTP API, and the web UI expose shared operations rather than separate implementations.

## Implementation boundaries

Product semantics belong in `crates/labby/src/dispatch/` or the owning surface-neutral crate. CLI/MCP/API handlers adapt inputs, caller context, and outputs; they must not duplicate validation, authorization, retry policy, destructive classification, or business rules.

Use `labby-primitives` for leaf action/plugin/security vocabulary; `labby-gateway` for upstream transports, discovery, routing, and gateway lifecycle; `labby-codemode` for the host-neutral JavaScript runner; `labby-auth` for reusable authentication; and `labby-runtime` for shared runtime, Artifact, authority, and task contracts. Browser and OpenAPI runtime behavior belong in their extracted crates, not surface handlers. `labby-apis` remains a pure contract/SDK boundary without ambient configuration loading or product transports.

The old product `dispatch/upstream.rs` is a compatibility shim, not the upstream implementation owner. `labby-model` is development-only and must not become a product dependency. Windows FFI and verified handle operations stay behind the safe API of `labby-winjob`.

External capabilities normally belong in configured upstream MCP servers. Add a built-in service only when Labby owns its state or lifecycle; follow [Service Onboarding](docs/dev/SERVICE_ONBOARDING.md).

## Protocol and runtime contracts

- Keep intentional compatibility identifiers: `lab://`, `ui://lab/`, the MCP server key `lab`, and scopes `lab:read`, `lab`, and `lab:admin`.
- Registered product services use shared action metadata and MCP `action` + `params` requests. `requires_admin` and `destructive` are independent axes. Restartable mutations or stdio configuration are not automatically destructive.
- Preserve caller/subject isolation, route exposure, bounded discovery, response budgets, cancellation, OAuth lifecycle fencing, spawn guards, and SSRF protections. Do not use rmcp's unbounded `Peer::list_all_*` helpers.
- Preserve structured error kinds, causes, effects, and recovery metadata through dispatch. Apply envelope mapping at the surface boundary. Use the [error contract](docs/contracts/agent-error-contract.md), [Code Mode errors](docs/contracts/code-mode-tool-errors.md), and [observability contract](docs/dev/OBSERVABILITY.md), including canonical surface names `cli`, `mcp`, and `api`.
- An explicit remote gateway target must never silently fall back to local state. Follow [configuration](docs/runtime/CONFIG.md), [OAuth](docs/runtime/OAUTH.md), and [remote authority](docs/design/REMOTE_GATEWAY_TARGET.md).

Standalone ACP chat, Marketplace/Registry browsers, Fleet, Deploy, and the old Agent Artifact Manager are retired. Provider-backed Artifact discovery does not restore them. Current principal-scoped File Stash is Linux-only and does not revive the retired Stash product. The direct stdio proxy is a CLI runtime exposing its child's MCP surface, not another registered product service.

## Build and verification

Use sibling `foo.rs` plus `foo/` modules, not `mod.rs`. Use native async traits; workspace Clippy policy forbids `#[async_trait]`.

The pinned `msrv` (1.97.1) is shared by Cargo, `rust-toolchain.toml`, CI, and container contracts. Product feature slices must compile independently where the feature contract declares them standalone; `proxy-testkit` is test support, not a product slice.

Run the relevant focused regression tests and owning package's gates. Root recipes:

```bash
just check
just test
just lint
just docs-check
just rustdoc-check
```

Metadata, CLI, or discovery changes also require `just docs-generate`. Cross-surface changes need coverage for every affected adapter. Linux/Windows-specific behavior needs its platform lane.

`tools/verification` is a separate Cargo workspace and lockfile; the desktop shell has a separate Rust manifest. Root workspace tests do not cover either automatically. See [Development](docs/dev/DEVELOPMENT.md) and [Testing](docs/dev/TESTING.md) for commands.

## UI and distribution

`apps/gateway-admin` is a statically exported operator UI consumed by the Rust host. Use the [Aurora design contract](docs/DESIGN.md) and existing `@aurora` registry components. Validate relevant UI tests and `just web-build`; do not introduce a standalone Node-server requirement.

`apps/labby-desktop` is a thin Tauri shell around the same control plane, not a second renderer, credential store, or product surface.

`plugins/labby` ships metadata, MCP configuration, and skills, not the binary or host bootstrap. Setup and repair belong to the binary; do not restore automatic Claude install/repair hooks. The npm launcher README is generated from the root README, not independently authored.

## Documentation boundaries

[docs/README.md](docs/README.md) indexes product documentation. Exact service/action/CLI inventories live in `docs/generated/`; regenerate them rather than copying lists into instructions or hand-editing output.

`docs/sessions/` and `docs/superpowers/` are protected historical/work-product trees. Normal audits must not edit, relocate, retire, or link-audit them. Explicit changes require the maintainer-applied `protected-docs-approved` PR label. `docs/references/` is an untracked reference cache.

[Architecture](docs/ARCH.md), [CI/CD](docs/runtime/CICD.md), and [Documentation Maintenance](docs/dev/DOCUMENTATION.md) own their detailed contracts.
