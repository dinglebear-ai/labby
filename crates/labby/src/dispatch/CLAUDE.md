# dispatch/ Instructions

This directory is Labby's shared product dispatch layer. CLI, HTTP, and MCP surfaces adapt into these handlers so validation, authorization, destructive metadata, error kinds, and business behavior stay consistent across transports.

## Current Service Shape

The current first-party dispatch modules are:

- `artifact_control.rs`, `artifacts.rs`, and `remote_control.rs`
- `doctor/`
- `fs/` plus `fs.rs` and `fs_atomic.rs`
- `gateway/`
- `lab_admin/`
- `server_logs/`
- `setup/`
- `skill_library.rs` plus `skill_library/`, and `skills.rs` plus `skills/`
- `snippets/`

Shared helpers live at this level: `clients.rs`, `error.rs`, `helpers.rs`, `oauth_subject.rs`, `path_safety.rs`, `redact.rs`, `security.rs`, and the upstream compatibility shim. `artifacts` owns Labby's durable local library plus bounded provider-backed discovery/lifecycle projections; `bundles`, `jobs`, `sources`, and `uploads` are supporting remote-authority services. The generated service/action catalogs are authoritative for exact live action names and feature exposure.

Do not reintroduce retired standalone Marketplace, Stash, Fleet/device-runtime, ACP, Deploy-product, or Registry-browser dispatch modules. Bounded provider-backed discovery actions under `artifacts` are part of the current control plane and are not standalone product modules.

## Service Ownership

For a normal service, keep service-specific parameter validation, action routing, and typed results in its dispatch module. Surface adapters should be thin and must not duplicate business logic. When a reusable runtime behavior belongs in an extracted crate, adapt it here rather than moving product transport dependencies into the shared crate.

The gateway is the main exception because substantial runtime behavior lives in `labby-gateway`. `dispatch/gateway/` is the product adapter around that reusable runtime.

## Contracts

- Action metadata is shared contract data. Keep destructive/admin classification aligned with actual behavior.
- `requires_admin` and `destructive` are separate policy axes. Never infer one from the other.
- Use stable structured error kinds from `docs/dev/ERRORS.md`; do not make each surface invent its own strings.
- Secrets and auth material must be redacted before logging or error construction.
- URL, path, and host mutations must pass the shared safety helpers instead of open-coding validation.
- Current serialization rules live in `docs/design/SERIALIZATION.md`.

## Adding Or Changing An Action

1. Update the service's action catalog/metadata and parameter types.
2. Implement or change the shared dispatch behavior.
3. Update every surface projection that is not generated automatically.
4. Update generated docs with `just docs-generate`.
5. Add focused dispatch tests plus transport tests where adaptation logic changed.
6. Run `just docs-check` so docs/catalog drift is caught before review.

## Verification

Start with focused crate/tests for the service, then run the repository checks appropriate to the change. For dispatch-wide changes, at minimum run Labby tests with all features and clippy for the affected targets.
