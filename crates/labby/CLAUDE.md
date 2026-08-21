# labby — Product Binary Crate

This crate owns the Labby product binary/library and the product-specific adapters around reusable workspace crates.

## Current Feature Contract

Use `Cargo.toml` as the source of truth:

- `default = ["gateway-host"]`
- `gateway-host = ["gateway"]`
- `all = ["lab-admin", "api-docs", "gateway-host", "fs", "systemd", "skills"]`
- `proxy-testkit` is test support only.

Retired ACP/Marketplace/Stash/Fleet/Deploy surfaces are deleted rather than feature-gated.

## Ownership

- `src/dispatch/` — shared product semantics and local product services
- `src/cli/` — clap adapters
- `src/mcp/` — MCP protocol adapters/resources/apps
- `src/api/` — axum HTTP adapters and middleware
- `src/output/` + `src/cli/style.rs` — Aurora human CLI renderer and clap styling
- `src/config.rs` — product configuration loading
- `src/registry.rs` — product service registration

Reusable gateway, Code Mode, auth, web-asset, and shared runtime behavior belongs in the extracted `labby-*` crates rather than being duplicated here.

## Surface Rule

CLI, MCP, and API are peers over shared dispatch. Keep handlers thin and do not reimplement operation semantics at the surface.

See nested instructions in `src/CLAUDE.md`, `src/cli/CLAUDE.md`, `src/mcp/CLAUDE.md`, `src/api/CLAUDE.md`, and `src/dispatch/CLAUDE.md`.

## ToolError

`ToolError` is the shared product error type. Its serialization is intentional and recovery-aware. Do not replace its manual serialization with a derived shape or stringify it before the surface boundary. See `docs/dev/ERRORS.md` and `docs/contracts/agent-error-contract.md`.

## Output

Human CLI output is implemented by the Aurora renderer under `src/output/`; JSON output is compact, unstyled machine data. Do not add command-local color palettes or ad hoc table systems.

## Verification

For product-crate changes run focused tests plus the relevant workspace gates. Changes to service metadata or CLI/MCP/API discovery also require `just docs-generate` and `just docs-check`.
