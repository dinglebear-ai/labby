---
title: "Testing"
created: "2026-07-30"
updated: "2026-07-30"
---

# Testing

Last updated: 2026-04-10

This document is the canonical testing contract for `labby`.

It defines:

- which layer owns which tests
- that implementation work must follow TDD
- the minimum required test coverage for new services
- what must be verified locally before claiming work complete
- what belongs in CI-safe tests versus live verification

## Goal

Testing must prove:

- shared behavior is correct
- surface adapters are aligned
- public contracts stay stable
- service integrations are observable and diagnosable
- destructive operations are not casually exercised

## TDD Rule

Implementation work must follow test-driven development.

Rules:

- write or update the failing test first for the behavior being added, fixed, or refactored
- make the smallest implementation change that makes the test pass
- refactor only after the behavior is covered
- if true test-first sequencing is not possible, document the reason explicitly in the task or review note

This applies to:

- new service endpoints
- shared contract changes
- bug fixes
- refactors that change observable behavior

Pure docs changes are excluded.

## Test Layers

There are four testing layers.

### 1. SDK Tests

Owned by `labby-apis`.

Purpose:

- request construction
- response decoding
- transport error mapping
- service client behavior

Rules:

- use mock HTTP where practical
- do not depend on real external services in CI-safe tests
- test shared HTTP behavior in `labby-apis`

### 2. Shared Dispatch-Layer Tests

Owned by `crates/labby/src/dispatch`.

Purpose:

- operation matching
- param validation
- shared schema behavior
- client / instance resolution
- shared `ToolError` behavior

Rules:

- these tests must not require CLI, MCP, or HTTP protocol setup
- they must be the primary place to test shared product-surface orchestration

### 3. Surface Adapter Tests

Owned by `crates/labby/src/cli`, `crates/labby/src/mcp`, and `crates/labby/src/api`.

Purpose:

- CLI parsing and output behavior
- MCP envelope shape, help, schema, and elicitation behavior
- HTTP request extraction, status mapping, and response shape

Rules:

- do not re-test shared operation semantics here unless the transport changes them
- keep these tests focused on adapter behavior

### 4. Live Verification

Owned by the implementation task for the service or feature.

Purpose:

- confirm real integration behavior against a running service
- verify observability and operator-facing behavior
- catch mismatches between upstream docs and the real system

Rules:

- live verification is opt-in and environment-dependent
- destructive actions must not be exercised unless explicitly intended and safe

## CI-Safe Versus Live

### CI-Safe Tests Must Cover

- SDK behavior with mocks
- shared dispatch validation and error behavior
- MCP and HTTP envelope shape
- CLI parsing and machine-readable output behavior
- contract-level serialization and error shape tests

### Live Verification Must Cover When Available

- at least one successful read-only path
- at least one failing path with the expected stable `kind`
- observability evidence for the path
- generated service/action/feature documentation alignment with the actual implementation

## Required Minimums For New Services

A new service is not fully online until all of the following exist:

1. SDK tests for core client behavior
2. shared dispatch-layer tests for operation matching and validation
3. MCP registry and shared-dispatch tests for envelope and schema behavior
4. API adapter tests for status and JSON shape
5. CLI tests for parsing and machine-readable output where the service exposes CLI behavior
6. non-destructive live verification for CLI and MCP when a real instance is available
7. observability verification according to [OBSERVABILITY.md](./OBSERVABILITY.md)

## Required Minimums For Non-Service Refactors

If a change affects shared contracts such as:

- observability
- errors
- serialization
- dispatch
- CLI behavior
- MCP behavior
- API behavior

then the change must add or update tests at the layer where the contract actually lives.

Those tests must be introduced before or alongside the implementation change, not added as cleanup afterward.

## Destructive-Operation Rule

Destructive operations must be tested differently from read-only operations.

Rules:

- destructive behavior may be covered by unit tests or mocked integration tests
- destructive live verification is not required by default
- if destructive live verification is performed, it must be intentional, documented, and safe
- non-destructive paths must still be live-tested when the user asked for real verification

## Contract Tests

The following contracts must have focused tests:

- error kind stability
- MCP success and error envelope shape
- HTTP JSON error shape and status mapping
- shared operation schema projection
- observability field presence

These tests must be narrow and stable. They exist to prevent silent contract drift.

## Verification Before Completion

Before claiming work complete, run the smallest set of commands that proves the touched contract still holds.

Minimum expectation:

- the change followed the TDD rule above unless explicitly documented otherwise
- targeted tests for the touched files or contract
- crate-level tests for the touched crate when the change is non-trivial
- broader verification when the change affects shared infrastructure
- `just docs-check` when changing registry entries, action catalogs,
  `PluginMeta`, API route metadata, Cargo features, onboarding audit checks, or
  generated docs artifacts

Preferred runner:

- use `cargo nextest run` for crate-level verification
- use `cargo test` only when nextest is unavailable or you need a narrow one-off command that nextest does not cover cleanly
- for this repo, `cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features` is the standard full-crate verification command

If tests were not run, say so explicitly.

## Command Guidance

Common commands:

```bash
just check
just docs-check
just test
just lint
cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features
cargo test -p labby-apis
cargo test --manifest-path crates/labby/Cargo.toml
```

Use narrower commands first when iterating, then broaden before completion.

## Generated Product Docs

Code-owned catalogs under `docs/generated/` are part of verification, not a substitute for tests. Run `just docs-generate` after metadata/surface changes and `just docs-check` before handoff. Generated service, action, route, environment, CLI, MCP, and feature projections must remain aligned with code.

## Ownership Summary

- `labby-apis` owns SDK tests
- `crates/labby/src/dispatch` owns shared dispatch tests
- `cli`, `mcp`, and `api` own adapter tests
- implementation tasks own live verification

## Related Docs

- [OBSERVABILITY.md](./OBSERVABILITY.md)
- [ERRORS.md](./ERRORS.md)
- [SERIALIZATION.md](../design/SERIALIZATION.md)
- [DISPATCH.md](./DISPATCH.md)
- [SERVICE_ONBOARDING.md](./SERVICE_ONBOARDING.md)
- [OPERATIONS.md](../OPERATIONS.md)
