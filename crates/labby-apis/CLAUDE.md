# labby-apis — Pure SDK Contracts

`labby-apis` is a small pure Rust SDK/data crate for shared HTTP primitives plus the current `doctor` and `setup` contracts. It is not the old one-module-per-homelab-service SDK.

## Current Modules

- `core`
- `doctor`
- `setup`

The `all` feature is an empty compatibility aggregate; `test-utils` is a reserved test marker. There are no optional product service modules.

## Hard Rules

- no `clap`, `rmcp`, `axum`, presentation libraries, or product routing
- no ambient config-file or environment reads
- callers pass URLs, auth, paths, and runtime configuration explicitly
- wire-facing data is serde-compatible and presentation-free
- secret-bearing `Debug` implementations must redact
- return typed errors instead of panicking

Shared action/plugin/SSRF vocabulary is defined in `labby-primitives` and re-exported from `core` where compatibility requires it.

## HTTP Client

`core::HttpClient` is the shared HTTP primitive. Preserve its TLS, timeout, URL-safety, and error-kind contracts. Do not hide retry/backoff policy inside this generic client unless the product contract explicitly changes.

See `src/core/CLAUDE.md` for the detailed cross-cutting invariants.
