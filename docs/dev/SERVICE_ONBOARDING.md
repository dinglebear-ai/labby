---
title: "Service Onboarding"
created: "2026-08-18"
updated: "2026-08-18"
---

# Service Onboarding

This is the end-to-end checklist for adding a **built-in Labby product service**.

Most external capabilities should not become built-in services at all. HTTP, Unix-socket, and stdio MCP servers belong in the gateway upstream catalog and are exposed through the existing `gateway`/upstream runtime. Add a new built-in service only when Labby itself owns the capability and its lifecycle.

The generated [service catalog](../generated/service-catalog.md) and [action catalog](../generated/action-catalog.md) are authoritative for the current product surface.

## Before Adding A Built-In Service

Prefer an upstream MCP server when the capability is independently deployable. A built-in service is appropriate when it needs Labby-owned local state, host integration, setup/doctor behavior, or another product-level contract that cannot live behind a normal upstream MCP boundary.

Do not recreate the retired pattern of one built-in SDK module per homelab application.

## Required Steps

1. Define stable request/result vocabulary in the lowest reusable crate that actually owns it. Use `labby-primitives` only for dependency-leaf vocabulary shared across crate boundaries.
2. Put reusable surface-neutral runtime behavior in the appropriate extracted crate (for example `labby-gateway`, `labby-runtime`, or `labby-codemode`).
3. Add the product dispatcher under `crates/labby/src/dispatch/<service>/`. The dispatcher owns action routing, validation, destructive classification, and shared semantics.
4. Keep CLI, MCP, HTTP, and web adapters thin. They must call shared dispatch rather than independently implement the operation.
5. Register only the surfaces the service actually supports.
6. Add a Cargo feature only when the service is an intentional optional product slice. Do not add placeholder features for future work.
7. Add a canonical service page under `docs/services/` when the service is user/operator visible.
8. Add tests for catalog registration, action schemas, errors, destructive/admin gates, feature slicing, and every exposed surface.
9. Regenerate code-owned docs and run product-doc validation.

## Verification

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
just docs-generate
just docs-check
```

## Source Documents

- [Service model](./SERVICES.md)
- [Dispatch](./DISPATCH.md)
- [Errors](./ERRORS.md)
- [Observability](./OBSERVABILITY.md)
- [Testing](./TESTING.md)
- [Architecture](../ARCH.md)
