# Labby development

Labby is a Rust MCP gateway and operator control plane. CLI, MCP, HTTP API, and the web UI adapt shared operations. Read applicable nested AGENTS.md files; [docs/README.md](docs/README.md) indexes current contracts.

## Build and test

Commands are root-relative unless stated otherwise. The pinned `msrv` (1.97.1) is synchronized across Cargo, rust-toolchain.toml, CI, and releases. Gateway Admin requires Node 22.x and pnpm 9.15.9; .mise.toml does not select Node. Verify the activated versions.

```bash
just check                         # workspace, all features
just test                          # workspace nextest, all features
just lint                          # drift, toolchain, module reachability, Clippy, fmt
just docs-check                    # generated freshness, links, documentation contracts
just rustdoc-check                 # strict Rustdoc plus doctests
```

Focused example: `cargo nextest run -p labby-gateway --all-features`. Nextest does not run doctests. Feature changes need `bash scripts/check-feature-slices.sh`; all-feature builds can hide broken slices. The product manifest owns feature membership; proxy-testkit is test-only.

Run these package checks from the directory in the first column:

| Directory | Checks |
| --- | --- |
| apps/gateway-admin | `pnpm install --frozen-lockfile; pnpm lint; pnpm test; pnpm test:browser; pnpm build` |
| apps/browser-extension | `npm ci; npm test; npm run typecheck` |
| apps/labby-desktop | `pnpm verify`; `cargo test --manifest-path src-tauri/Cargo.toml --locked` |
| packages/labby-mcp | `npm run check; npm test` |

From the root, use `just verify-check`, `just verify-test`, and `just verify-lint` for tools/verification. Unraid packaging uses `bash scripts/ci/unraid-runtime-tests.sh` with the platform prerequisites in CI.

Desktop and tools/verification are separate Cargo workspaces; root tests do not cover them. Keep verification fixtures serial as encoded in just verify-test. `just test-integration` runs opt-in ignored tests; `just live-e2e` is the owned-process live harness. See [Testing](docs/dev/TESTING.md) for prerequisites.

Build traps: `just build` uses release-fast. `just build-release`/`just install` install a binary; host-sync restarts the service. Do not use as compile checks. `just chat-local` disables browser auth and binds all interfaces; it is not an isolated default preview. Preview with loopback and a separate absolute LABBY_HOME.

## Find the owning implementation

- `crates/labby/src/dispatch/` owns product operations; cli/, mcp/, and api/ are adapters. entrypoint.rs and cli/serve.rs compose the runtime. registry.rs and catalog.rs own product discovery; access/ owns durable authority and migrations.
- `labby-gateway` owns upstream pools, transports, catalogs, OAuth lifecycle, routing, and the gateway Code Mode host. Product dispatch/upstream.rs is a shim.
- `labby-codemode` owns the host-neutral Javy/QuickJS subprocess runner, protocol, budgets, and snippets. Do not duplicate it in the gateway or substitute Wasmtime.
- `labby-primitives` owns leaf action/plugin/security vocabulary; `labby-runtime` owns shared errors, authority, Artifacts, Agents/Tasks, and lifecycle contracts. `labby-auth` owns reusable inbound/upstream authentication.
- `labby-apis` owns pure core, doctor, setup, and artifact_control SDK contracts, without ambient config loading or product transports. Browser runtime belongs in labby-browser; hardened OpenAPI execution in labby-openapi; static assets in labby-web.
- `labby-model` is development-only. Windows FFI stays behind labby-winjob's safe API; xtask is repository automation.

Use MCP upstreams for external capabilities; add built-ins only for Labby-owned state/lifecycle. Follow [Service Onboarding](docs/dev/SERVICE_ONBOARDING.md). Use foo.rs plus foo/, not mod.rs; native async traits, not #[async_trait]. `just module-reachability` catches orphan Rust files that rustc never sees.

## Contracts that must survive changes

Keep `lab://`, `ui://lab/`, MCP server key `lab`, and scopes `lab:read`, `lab`, `lab:admin`. Product MCP service tools accept action + params. CLI commands derive from Clap, not a copied MCP command inventory. The direct stdio proxy exposes its child and is not a registered proxy service.

Construct Labby-owned Tool descriptors through permanent_tools.rs. Wire listing and peer_contract.rs descriptor hashing must agree, or tools/list_changed lies about visibility. Preserve upstream annotations and native ui:// identities without Labby normalization.

ActionSpec is shared policy: requires_admin and destructive are independent; a restartable mutation or stdio configuration is not automatically destructive. Enforce exposure on direct invocation/read/get as well as discovery, including subject-scoped and relay paths. Preserve caller, Team, route, loadout, and OAuth-subject isolation.

Use bounded listing helpers, not Peer::list_all_*. Keep per-upstream concurrency, pagination/byte/deadline limits, coalesced refreshes, and partial-failure diagnostics. One slow upstream must not stall unrelated ones. Cancellation must stop waiting/dispatch and propagate appropriately; caller cancellation is not an upstream circuit-breaker failure. OAuth mutation invalidates subject, relay, and task peers; late discoveries must fail lifecycle-epoch publication guards.

Preserve typed error kinds, causes, side_effects, and recovery through dispatch; map envelopes only at adapters. Follow [Errors](docs/dev/ERRORS.md) and [Observability](docs/dev/OBSERVABILITY.md), including cli/mcp/api surface names and credential redaction. Keep SSRF/redirect, spawn-guard, filesystem/handle, and response-budget protections in their owning runtime.

Configuration loading owns env/.env/TOML precedence. An explicitly configured remote target must never fall back silently to local state. Access-store changes need migration approval, integrity checks, and rollback qualification; validate current schemas on one snapshot to avoid bootstrap races.

## UI, packaging, and documentation

Gateway Admin is a Next.js static export, not a production Node server. Build apps/gateway-admin/out before rebuilding Rust: labby-web accepts a missing export and embeds no UI. Preserve generated docs/install assets, build IDs, and bundle-budget checks. Reuse the [Aurora contract](docs/DESIGN.md), @aurora primitives, and existing typed API/auth helpers.

The desktop hosts that same UI with confined navigation and bounded browser sign-in handoff, not a second product renderer or durable token store. The extension uses native WebMCP over /browser/socket; preserve explicit site permissions and fingerprint-approved pairing, not DOM-automation fallbacks.

plugins/labby owns skills and MCP metadata; skills/ contains discovery aliases. Setup/repair and host lifecycle belong to the binary, not automatic plugin hooks. After root README edits, run `node packages/labby-mcp/scripts/sync-readme.js`. Release workflows own verified artifacts, version synchronization, and draft promotion.

Regenerate docs/generated with `just docs-generate` after metadata/CLI/schema changes; never hand-edit catalogs. Do not revive retired ACP/Marketplace/Fleet/Deploy products. Artifact discovery and Linux File Stash are separate contracts. docs/sessions/ and docs/superpowers/ are protected: no edits, moves, or link audits without explicit scope and the protected-docs-approved label. docs/references/ is an untracked cache.
