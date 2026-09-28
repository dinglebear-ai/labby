# Labby

Labby is a Rust MCP gateway and operator control plane. CLI, MCP, HTTP API, and UI adapt shared operations.

## Read for the task

Read applicable nested AGENTS.md files and the matching references below for the changed subsystem, not the entire tree. [Product docs](docs/README.md) indexes contracts.

| Change | Start here |
| --- | --- |
| Architecture or service ownership | [Architecture](docs/ARCH.md), [Service onboarding](docs/dev/SERVICE_ONBOARDING.md) |
| Upstream transport, catalog, or routing | [Upstream instructions](crates/labby-gateway/src/upstream/AGENTS.md), [Upstream contract](docs/services/UPSTREAM.md) |
| Code Mode or snippets | [Runner instructions](crates/labby-codemode/AGENTS.md), [Code Mode](docs/dev/CODE_MODE.md), [Snippets](docs/services/SNIPPETS.md) |
| MCP tools, schemas, or notifications | [MCP instructions](crates/labby/src/mcp/AGENTS.md), [Conformance](docs/surfaces/MCP_CONFORMANCE.md) |
| Authentication, access, or configuration | [Auth instructions](crates/labby-auth/AGENTS.md), [Access](docs/services/ACCESS.md), [Configuration](docs/runtime/CONFIG.md) |
| Errors, retries, or diagnostics | [Error mapping](docs/dev/ERRORS.md), [Observability](docs/dev/OBSERVABILITY.md) |
| Operator UI, desktop, or extension | [Gateway Admin](apps/gateway-admin/AGENTS.md), [Desktop](apps/labby-desktop/AGENTS.md), [Browser bridge](docs/services/BROWSER.md) |
| Skills, Artifacts, or packaging | [Plugin instructions](plugins/labby/AGENTS.md), [Artifacts/Skills](docs/services/SKILLS.md), [Plugin contract](docs/PLUGINS.md) |
| Tests, documentation, or CI | [Testing](docs/dev/TESTING.md), [Documentation maintenance](docs/dev/DOCUMENTATION.md), [CI/CD](docs/runtime/CICD.md) |

## Implementation boundaries

Put shared product operations in `crates/labby/src/dispatch/` or the owning extracted runtime, not duplicated CLI/MCP/API handlers. Adapters translate input, caller context, and output; validation, authorization, destructive classification, and retry policy stay below them.

`labby-gateway` owns upstream pools, discovery, routing, OAuth lifecycle, and the gateway Code Mode host. Product `dispatch/upstream.rs` is only a compatibility shim. `labby-codemode` owns the host-neutral execution kernel and runner protocol; gateway catalog wiring does not belong there.

`labby-apis` remains a pure SDK boundary without ambient configuration or product transports. `labby-model` is never a product dependency; Windows FFI stays behind `labby-winjob`'s safe API.

Prefer configured MCP upstreams for external capabilities. Add built-ins only for Labby-owned state/lifecycle through Service Onboarding; Architecture owns the complete crate map.

## Contracts to preserve

- Keep compatibility identifiers `lab://`, `ui://lab/`, MCP server key `lab`, and scopes `lab:read`, `lab`, `lab:admin`. Router tools accept `action` plus `params`; atomic projections derive their schemas from shared action metadata. CLI commands derive from Clap, not the MCP catalog.
- `ActionSpec.requires_admin` and `destructive` are independent axes. Restartable mutation and stdio configuration are not automatically destructive; use each surface's shared policy.
- Build Labby-owned descriptors through `permanent_tools.rs`. Wire listing and `peer_contract.rs` hashing must agree for correct `tools/list_changed`. Upstream annotations and native `ui://` identities pass through without Labby normalization.
- Enforce exposure on direct invocation/read/get, not only listing. Preserve caller, Team, route, loadout, OAuth-subject, and retained-result isolation.
- Use bounded upstream listing, not `Peer::list_all_*`. Preserve pagination, byte/deadline limits, per-upstream concurrency, refresh coalescing, and partial-failure diagnostics. A slow upstream must not stall unrelated ones. Caller cancellation is not an upstream circuit-breaker failure.
- Fence late connection/catalog publication with the OAuth lifecycle epoch and publication guard. Subject, relay, and task-peer invalidation must not permit stale results to republish.
- Preserve typed error kinds, causes, `side_effects`, and recovery until surface mapping. Code Mode uses the [tool-error contract](docs/contracts/code-mode-tool-errors.md); traces use `cli`, `mcp`, and `api` surface names. Keep SSRF, redirect, spawn-guard, filesystem, and response-budget enforcement in the owning runtime.
- Configuration loading owns env/.env/TOML precedence. An explicit remote target must not silently fall back to local state: see [Remote gateway authority](docs/design/REMOTE_GATEWAY_TARGET.md). Access-store changes require migration/integrity checks; current-schema validation must use one snapshot to avoid bootstrap races.

## Build and verification

From the repository root, use [Justfile](Justfile) recipes, the [Rust pin](rust-toolchain.toml), and [product feature definitions](crates/labby/Cargo.toml). Package manifests specify Node/pnpm requirements.

```bash
just check                         # all-feature workspace compilation
just test                          # all-feature workspace nextest
just lint                          # drift, toolchain, reachability, Clippy, fmt
just docs-check                    # generated freshness and documentation gates
just rustdoc-check                 # strict Rustdoc and workspace doctests
```

Focused gateway tests: `cargo nextest run -p labby-gateway --all-features`. Nextest does not run doctests. Feature changes also need `bash scripts/check-feature-slices.sh`; all-feature success can conceal a broken standalone slice. `proxy-testkit` is test support, not a product slice. `just module-reachability` catches orphan Rust files that rustc never visits.

`just build` builds web assets first, then the all-feature `release-fast` binary. Raw Cargo builds do not refresh the static export. `just build-release` and `just install` install a binary; `just host-sync` restarts a service. These are not interchangeable compile checks. `just chat-local` disables browser auth and binds all interfaces, so it is not the default isolated preview.

Run package checks in the named directory:

| Package | Commands in that directory |
| --- | --- |
| `apps/gateway-admin` | `pnpm install --frozen-lockfile`; `pnpm lint`; `pnpm test`; `pnpm test:browser`; `pnpm build` |
| `apps/browser-extension` | `npm ci`; `npm test`; `npm run typecheck` |
| `apps/labby-desktop` | `pnpm verify`; `cargo test --manifest-path src-tauri/Cargo.toml --locked` |
| `packages/labby-mcp` | `npm run check`; `npm test` |

Desktop and `tools/verification` have separate Cargo workspaces. For verification, use root `just verify-check`, `just verify-test`, and `just verify-lint`; keep fixtures serial as the recipe requires. See Testing for live/ignored-suite prerequisites.

## UI, packaging, and documentation

Gateway Admin is a static export, not a production Node server. Reuse the [Aurora contract](docs/DESIGN.md), `@aurora` components, and typed API/auth helpers. Preserve export build IDs, generated assets, and bundle budgets. Desktop hosts that same control plane, not a second renderer or durable token store. The browser bridge requires explicit site permissions and approved pairing.

Plugin assets own skills and MCP metadata; the binary owns setup/repair and host lifecycle. Do not restore automatic plugin install hooks. After root README edits, run `node packages/labby-mcp/scripts/sync-readme.js`.

Regenerate `docs/generated/` with `just docs-generate` after metadata/CLI/schema changes; do not hand-edit catalogs. Standalone ACP/Marketplace/Fleet/Deploy products remain retired; Artifact discovery and Linux File Stash are separate contracts.

`docs/sessions/` and `docs/superpowers/` are protected history: no edits, moves, or link audits without explicit scope and the `protected-docs-approved` label. `docs/references/` is an untracked cache, not product authority.
