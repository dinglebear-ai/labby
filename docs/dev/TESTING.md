---
title: "Testing"
created: "2026-07-30"
updated: "2026-08-18"
---

# Testing

This document is the canonical testing contract for Labby.

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
- docs/coverage alignment with the actual implementation

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
- `just rustdoc-check` when changing public Rust APIs, Rustdoc, crate/module
  structure, examples, binaries, or anything referenced by intra-doc links
- `just rustdoc-audit` when reviewing public API documentation coverage; this
  reports historical missing-prose debt without blocking the strict correctness gate

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
just rustdoc-check
just rustdoc-audit
just test
just lint
cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features
cargo test -p labby-apis
cargo test --manifest-path crates/labby/Cargo.toml
```

Use narrower commands first when iterating, then broaden before completion.

## Coverage And Verification

Coverage is established by executable tests, generated contract checks, and explicit live verification where a change crosses a runtime boundary. Historical per-service coverage ledgers are not maintained as a parallel documentation surface because they drifted from the code they were meant to describe.

Rules:

- tests and generated catalogs must reflect the real implementation surface
- docs must not claim live-tested status unless that testing actually happened
- implementation counts and file references must stay aligned with code

## Live End-to-End Qualification

The catalog-driven product suite is separate from MCP protocol conformance. Run
the bounded hermetic PR tier with:

```bash
just live-e2e pr 1
```

The Bash supervisor and its process-group orchestration tests are Unix-only.
Native Windows process-tree containment is verified separately through the
required Windows Job Object tests. The Unix checks do not replace those tests
or make them advisory.

`nightly` adds the one-worker live browser lane, `collision` runs two isolated
copies of the stateful HTTP/CLI/API shard, and `repeat10` executes the hermetic
PR tier ten times with seeds 1 through 10.
`manual` is the credential-free operator tier. External-provider probes remain
informational and require their own explicitly supplied credentials.

Every run writes a schema-versioned coverage report that joins every registered
action and route to its classification, scenario, surfaces, minimum evidence,
and explicit exclusions. Each declared shard must produce a run-bound content
hash bound to the run seed and binary identity; aggregation recomputes every
hash from its bounded owned log, and missing shard output fails. Primary,
cleanup, and evidence-retention failures are reported independently.

Release qualification requires `LABBY_RELEASE_BINARY` to name an absolute,
executable packaged binary. Its version and SHA-256 identity are recorded; the
release tier does not substitute `cargo run` for the packaged product identity.
Reproduction commands contain placeholders rather than credentials. Run roots,
browser state, traces, logs, and reports are disposable, permission-restricted,
and bounded. Each live fixture scans its actual generated credentials and seeded
secret canaries before deleting its owned root. The outer CI supervisor also
injects a run-specific scan-only secret and recursively scans every retained
file for that value before publishing evidence; symlinks and oversized retained
files fail the evidence audit.

The live browser job consumes prebuilt Gateway Admin assets and a supervisor-
created `LABBY_LIVE_BROWSER_DESCRIPTOR`; Playwright owns Chromium only. The
outer supervisor owns Labby, loopback ports, browser-session storage state,
fixtures, evidence roots, and teardown.

## Ownership Summary

- `labby-apis` owns SDK tests
- `crates/labby/src/dispatch` owns shared dispatch tests
- `cli`, `mcp`, and `api` own adapter tests
- implementation tasks own live verification

## Related Docs

- [OBSERVABILITY.md](./OBSERVABILITY.md)
- [ERRORS.md](./ERRORS.md)
- [design/SERIALIZATION.md](../design/SERIALIZATION.md)
- [DISPATCH.md](./DISPATCH.md)
- [SERVICE_ONBOARDING.md](./SERVICE_ONBOARDING.md)
- [RUSTDOC.md](./RUSTDOC.md)
- [OPERATIONS.md](../OPERATIONS.md)
