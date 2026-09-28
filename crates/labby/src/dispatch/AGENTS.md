# dispatch/ Instructions

This directory is Labby's shared product dispatch layer. CLI, HTTP, and MCP surfaces adapt into these handlers so validation, authorization, destructive metadata, error kinds, and business behavior stay consistent across transports.

## Current Service Shape

Current operation owners include access/bootstrap and Projects; Agents, Tasks, and the shared Assistant provider; Artifacts, distribution, sources, and protected publication; browser and development-container lifecycles; gateway, setup, doctor, server logs, snippets, optional filesystem access, and Linux File Stash. Inspect this directory and the generated service/action catalogs rather than treating a handwritten module list as exhaustive.

Shared helpers include clients, errors, execution catalogs, OAuth subjects, path safety, redaction, schemas, and security. `upstream.rs` is a compatibility shim to the extracted `labby-gateway` implementation. `artifacts` owns the local library and bounded provider-backed projections; `bundles`, `jobs`, `sources`, and `uploads` support remote authority.

Do not reintroduce retired standalone Marketplace, Agent Artifact Manager, Fleet/device-runtime, ACP, Deploy-product, or Registry-browser products. Current File Stash and provider-backed Artifact discovery do not restore those retired product contracts.

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
