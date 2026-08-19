# gateway/ Instructions

This directory is the gateway orchestration layer inside `labby-gateway`. It owns gateway configuration/state, imports and discovery, lifecycle management, virtual servers, OAuth resource state, enrichment, usage accounting, and the thin adapter that binds host-neutral `labby-codemode` to an `UpstreamPool`.

## Current Layout

- `config.rs`, `config_store.rs`, `config_mutation.rs`: persisted gateway configuration and mutation safety.
- `manager/`: lifecycle, imports, persistence, OAuth resources, pool lifecycle, protected routes, views, virtual servers, usage, and Code Mode runtime resolution.
- `discovery/`: imports from supported client ecosystems.
- `enrichment/`: optional metadata collection and summarization.
- `code_mode/`: gateway-side host adapter, catalog/search projection, embeddings, error mapping, and catalog cache.
- `oauth.rs` and `oauth_lifecycle/`: gateway OAuth configuration and probes.
- `virtual_servers.rs` and `protected_routes.rs`: derived gateway views and routing state.

The JavaScript runner, warm-runner pool, protocol, result shaping, artifacts, snippets, and sandbox state live in the separate `labby-codemode` crate. There is no Wasmtime runner in this directory. Do not recreate one here.

## Boundary Rules

- Keep gateway/runtime code surface-neutral. Do not depend on Clap, product MCP handlers, or the web app.
- The Code Mode adapter may translate gateway/upstream vocabulary into `labby-codemode` host contracts; runner mechanics belong in `labby-codemode`.
- Configuration mutations must be transactional and leave persisted configuration valid on failure. Runtime mutation locks are never repository artifacts.
- Preserve OAuth subject/owner isolation when resolving runtime state, tools, and retained artifacts.
- Discovery/import logic must not silently overwrite operator-owned configuration.
- Virtual-server and view projections are derived state; do not create a second source of truth.

## Upstream Rules

Read `../upstream/CLAUDE.md` before changing pool or transports. Preserve bounded discovery, response-size limits, timeout classification, repeated-cursor protection, spawn guards, SSRF protections, and structured upstream error mapping.

## Code Mode Rules

Read `crates/labby-codemode/CLAUDE.md` before changing the adapter. The live runtime is Javy/QuickJS via the runner protocol managed by `labby-codemode`; this directory only supplies discovery and tool-call capabilities through the gateway host implementation.

## Verification

```bash
cargo test -p labby-gateway
cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings
```
