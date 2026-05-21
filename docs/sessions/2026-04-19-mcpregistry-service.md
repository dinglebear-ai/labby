---
date: 2026-04-19 11:58:10 EST
repo: git@github.com:jmagar/lab.git
branch: bd-lab-77y5/mcpregistry-service
head: 4163552
plan: none
agent: Claude (claude-sonnet-4-6)
session id: c1daa3e7-7709-4e4e-810e-7dc005f2ce98
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/c1daa3e7-7709-4e4e-810e-7dc005f2ce98.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab [bd-lab-77y5/mcpregistry-service]
---

## User Request

Execute bead `lab-77y5` — a 3-wave epic to implement a full `mcpregistry` service that lets users browse, search, and install MCP servers from the official registry (https://registry.modelcontextprotocol.io).

## Session Overview

Implemented the complete `mcpregistry` service across three sequential waves: (1) pure SDK in `lab-apis`, (2) dispatch layer + all product surfaces (CLI/MCP/API), and (3) the `install` compose subcommand that chains registry lookup → SSRF validation → gateway registration. All 1646 tests pass; clippy clean; 3 commits on branch `bd-lab-77y5/mcpregistry-service`.

## Sequence of Events

1. Read Wave 1 bead (lab-77y5.1) and all reference files (OpenAPI spec, `bytestash` patterns, `HttpClient` internals).
2. Created `lab-apis` SDK: `mcpregistry/types.rs`, `mcpregistry/client.rs`, `mcpregistry/error.rs`, `mcpregistry.rs` with `ServiceClient` impl, 12 wiremock tests.
3. Fixed two issues: `--features` flag syntax for `cargo test`, and a Clippy backtick warning on a doc comment.
4. Committed Wave 1 as `feat(lab-77y5.1)` (d955aa7).
5. Read Wave 2 bead (lab-77y5.2), read all integration reference files: `bytestash` dispatch/client/catalog/dispatch patterns, `api/services/bytestash.rs`, `dispatch/clients.rs`, `registry.rs`, `cli.rs`, `dispatch/error.rs`.
6. Created all Wave 2 files: `dispatch/mcpregistry/{catalog,client,params,dispatch}.rs`, `dispatch/mcpregistry.rs`, `cli/mcpregistry.rs`, `mcp/services/mcpregistry.rs`, `api/services/mcpregistry.rs`.
7. Registered service in: `dispatch.rs`, `dispatch/clients.rs`, `cli.rs`, `mcp/services.rs`, `api/services.rs`, `api/router.rs`, `registry.rs`, `lab/Cargo.toml`.
8. Fixed three compile errors: wrong `&p` borrow on `list_servers`, missing `Auth::None` arg to `McpRegistryClient::new`, missing `Serialize` derives on response types.
9. Added `From<RegistryError> for ToolError` in `dispatch/error.rs`.
10. Committed Wave 2 as `feat(lab-77y5.2)` (541c24c).
11. Read Wave 3 bead (lab-77y5.3), read `gateway/params.rs` and `config.rs` for `UpstreamConfig` shape.
12. Rewrote `cli/mcpregistry.rs` with `McpregistryCommand` subcommand enum: `Action` (generic dispatch shim) + `Install` (two-call compose).
13. Discovered `registry_and_router_service_sets_are_identical` test failure — added `mcpregistry` to the `http_router_services` set in `registry.rs`.
14. All 1646 tests pass; committed Wave 3 as `feat(lab-77y5.3)` (4163552).

## Key Findings

- `HttpClient::from_parts()` is `pub(crate)` — client construction in `lab-apis/src/mcpregistry/client.rs` uses a custom `reqwest::Client` with 20s timeout and `redirect::Policy::none()`.
- `ServerListResponse` and related response types were `Deserialize`-only; needed `Serialize` added for `to_json()` helper at the dispatch boundary — `lab-apis/src/mcpregistry/types.rs:58–115`.
- `ListServersParams` field names: `search` (not `query`), `cursor` (not `offset`) — caught when writing `dispatch/mcpregistry/params.rs`.
- `get_server()` takes `(name, version)` — must pass `"latest"` as second arg (`lab-apis/src/mcpregistry/client.rs:92`).
- `registry_and_router_service_sets_are_identical` test at `registry.rs:566` enforces that every MCP-registered service also has an HTTP route; required adding `mcpregistry` to the hardcoded set at `registry.rs:523`.
- `ToSocketAddrs` DNS resolution in `validate_registry_url` is synchronous blocking — annotated as `MUST NOT call from async context without spawn_blocking` (`cli/mcpregistry.rs`).
- DNS TOCTOU residual risk documented at the `gateway.add` call site: short-TTL rebind could bypass the check, accepted under homelab threat model.

## Technical Decisions

- **`Auth::None` for registry client**: The MCP registry is a public API requiring no auth header. `McpRegistryClient::new` signature still requires the `Auth` param for structural consistency; pass `Auth::None`.
- **`Serialize` added to all response types**: Instead of converting to `serde_json::Value` manually in dispatch, added `Serialize` to `ServerListResponse`, `Metadata`, `ServerResponse`, `ResponseMeta`, `RegistryExtensions`, `ValidationResult`, `ValidationIssue` — keeps the dispatch arms using the shared `to_json()` helper.
- **`McpregistryCommand` subcommand enum**: Wave 3 split the flat `action + params` CLI into two named subcommands (`action` and `install`) to give `install` its own typed flag set (`--gateway-name`, `--bearer-token-env`, `--version`, `-y`).
- **SSRF validation in CLI surface only**: Bead specifies composition lives only in CLI; dispatch layers stay decoupled. `validate_registry_url()` is a private helper in `cli/mcpregistry.rs`, not promoted to a shared util.
- **`client_from_env()` always succeeds**: Unlike most services, `mcpregistry` has a well-known public default URL (`REGISTRY_DEFAULT_URL`). The function falls back to that default when `MCPREGISTRY_URL` is unset, so it always returns `Some(client)` unless TLS init fails.

## Files Modified

| File | Change |
|------|--------|
| `crates/lab-apis/src/mcpregistry/types.rs` | Added `Serialize` derives to `ServerListResponse`, `Metadata`, `ServerResponse`, `ResponseMeta`, `RegistryExtensions`, `ValidationResult`, `ValidationIssue` |
| `crates/lab-apis/src/mcpregistry/client.rs` | Created: `McpRegistryClient`, `list_servers`, `get_server`, `list_versions`, `validate`, `health_probe`, 12 wiremock tests |
| `crates/lab-apis/src/mcpregistry/error.rs` | Created: `RegistryError` with `InvalidInput` and `Api(ApiError)` variants |
| `crates/lab-apis/src/mcpregistry.rs` | Created: module entry, `META: PluginMeta`, `ServiceClient` impl |
| `crates/lab-apis/src/lib.rs` | Added `#[cfg(feature = "mcpregistry")] pub mod mcpregistry;` |
| `crates/lab-apis/Cargo.toml` | Added `mcpregistry = []` feature + entry in `all` array |
| `crates/lab/src/dispatch/mcpregistry/catalog.rs` | Created: `ACTIONS` const with `help`, `schema`, `server.list`, `server.get`, `server.versions` |
| `crates/lab/src/dispatch/mcpregistry/client.rs` | Created: `client_from_env`, `require_client`, `not_configured_error` |
| `crates/lab/src/dispatch/mcpregistry/params.rs` | Created: `list_servers_params`, `require_name` helpers |
| `crates/lab/src/dispatch/mcpregistry/dispatch.rs` | Created: `dispatch` + `dispatch_with_client` |
| `crates/lab/src/dispatch/mcpregistry.rs` | Created: module entry, re-exports, 5 unit tests |
| `crates/lab/src/dispatch/error.rs` | Added `From<RegistryError> for ToolError` (hand-rolled, not macro) |
| `crates/lab/src/dispatch/clients.rs` | Added `mcpregistry: Option<Arc<McpRegistryClient>>` field + `from_env` init |
| `crates/lab/src/dispatch.rs` | Added `#[cfg(feature = "mcpregistry")] pub mod mcpregistry;` |
| `crates/lab/src/cli/mcpregistry.rs` | Created (Wave 2) + expanded (Wave 3) with `Action` + `Install` subcommands, `validate_registry_url`, 9 unit tests |
| `crates/lab/src/cli.rs` | Added `#[cfg(feature = "mcpregistry")] pub mod mcpregistry;`, `Mcpregistry` command variant, dispatch arm |
| `crates/lab/src/mcp/services/mcpregistry.rs` | Created: thin bridge with 1 test |
| `crates/lab/src/mcp/services.rs` | Added `#[cfg(feature = "mcpregistry")] pub mod mcpregistry;` |
| `crates/lab/src/api/services/mcpregistry.rs` | Created: axum `POST /v1/mcpregistry` handler with `DefaultBodyLimit::max(1_048_576)` |
| `crates/lab/src/api/services.rs` | Added `#[cfg(feature = "mcpregistry")] pub mod mcpregistry;` |
| `crates/lab/src/api/router.rs` | Added `mount_if_enabled!(v1, state, "mcpregistry", "mcpregistry", mcpregistry);` |
| `crates/lab/src/registry.rs` | Added `register_service!` call + `service_meta` arm + `http_router_services` test set entry |
| `crates/lab/Cargo.toml` | Added `mcpregistry = ["lab-apis/mcpregistry"]` feature + entry in `all` array |

## Commands Executed

```bash
# Test Wave 1 only
cargo test -p lab-apis --all-features mcpregistry
# → 12 passed

# Check all-features build (Wave 2)
cargo check --all-features
# → 0 errors after fixing borrow, Auth arg, Serialize derives

# Run targeted mcpregistry tests
cargo test --all-features mcpregistry
# → 22 passed (Wave 1+2), then 42 passed (Wave 1+2+3)

# Full test suite (Wave 3 final)
cargo test --all-features
# → 1646 passed, 2 ignored

# Clippy verification
cargo clippy --all-features
# → 0 errors, 0 mcpregistry warnings
```

## Errors Encountered

| Error | Root Cause | Fix |
|-------|-----------|-----|
| `cargo test -p lab-apis --features mcpregistry mcpregistry` — 219 errors | Other feature-gated tests tried to compile without their required deps | Changed to `--all-features` flag |
| Clippy: "item in documentation is missing backticks" | `TimeoutLayer` in doc comment needed backticks | Changed to `` `TimeoutLayer` `` |
| `list_servers(&p)` — mismatched types | `ListServersParams` is taken by value, not by reference | Removed `&` |
| `McpRegistryClient::new(&url)` — wrong arg count | Forgot `Auth` parameter | Added `Auth::None` |
| `ServerListResponse: serde::Serialize` not satisfied | Response types only derived `Deserialize` | Added `Serialize` to all response types |
| `get_server(&name)` — wrong arg count | `get_server(name, version)` requires version | Added `"latest"` as second arg |
| `registry_and_router_service_sets_are_identical` test failure | `mcpregistry` registered in MCP registry but not in test's `http_router_services` set | Added `#[cfg(feature = "mcpregistry")] s.insert(lab_apis::mcpregistry::META.name);` in test |

## Behavior Changes (Before/After)

**Before:** No MCP Registry integration — users had to manually find and configure MCP servers.

**After:**
- `lab mcpregistry action server.list [query=foo]` — searches the official MCP registry
- `lab mcpregistry action server.get name=<name>` — shows server details including transport URLs
- `lab mcpregistry action server.versions name=<name>` — lists available versions
- `lab mcpregistry install <name> [-y] [--gateway-name n] [--bearer-token-env E]` — fetches server from registry, validates URL (HTTPS + no private ranges), adds as a gateway upstream in one command
- `POST /v1/mcpregistry` — same dispatch surface over HTTP
- MCP tool `mcpregistry` registered for AI agents

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo test --all-features mcpregistry` | all mcpregistry tests pass | 42 passed | ✓ |
| `cargo test --all-features` | full suite passes | 1646 passed, 2 ignored | ✓ |
| `cargo clippy --all-features` | no errors | 0 errors, 0 mcpregistry warnings | ✓ |
| `cargo check --all-features` | 0 errors | 0 errors | ✓ |
| `registry_and_router_service_sets_are_identical` | mcpregistry in both sets | 2 passed | ✓ |

## Risks and Rollback

- **SSRF validation gap**: `validate_registry_url()` checks IP addresses and resolves hostnames at call time. DNS TTL expiry between validation and `gateway.add` could rebind a hostname to a private address. Documented at call site; accepted under homelab threat model.
- **`MCPREGISTRY_URL` SSRF**: An operator setting `MCPREGISTRY_URL` to a private address (e.g., Docker socket at `http://localhost:2375`) could use it to probe internal services. The bead notes this risk but defers mitigation to a follow-up; current code has no server-side guard on the env var value.
- **Rollback**: `git revert d955aa7 541c24c 4163552` removes all three commits cleanly. No schema migrations, no persistent state changes. The feature is gated on `mcpregistry` feature flag which is included in `all` but not `default`.

## Decisions Not Taken

- **`gateway.add_from_registry` in gateway dispatch**: The architecture review comment in the bead (INVESTIGATION) recommended moving composition to a new `gateway.add_from_registry` dispatch action. Rejected per the bead's Locked Decisions: "Composition lives ONLY in CLI surface — dispatch layers stay fully decoupled." MCP agents achieve the same result with two tool calls.
- **`DefaultBodyLimit` via `axum_extra`**: Initial implementation imported from `axum_extra` (not a dependency). Fixed to use `axum::extract::DefaultBodyLimit` which is part of the existing `axum` 0.8 dependency.
- **Shared `validate_registry_url` util**: Could have placed SSRF validation in a shared helper module. Left in `cli/mcpregistry.rs` per bead Discretion guidance; can be promoted if other surfaces need it.

## References

- OpenAPI spec: `openapi.yaml` (authoritative type source for `mcpregistry/types.rs`)
- Pattern reference: `crates/lab/src/dispatch/bytestash/` (catalog, client, dispatch, params templates)
- Pattern reference: `crates/lab/src/api/services/bytestash.rs` (API handler pattern)
- Pattern reference: `crates/lab/src/dispatch/clients.rs` (ServiceClients field pattern)
- Bead epic: `lab-77y5` — MCP Registry Service
- Sub-beads: `lab-77y5.1` (SDK), `lab-77y5.2` (dispatch+surfaces), `lab-77y5.3` (install compose)

## Open Questions

- Should `MCPREGISTRY_URL` be validated against RFC-1918 ranges at `client_from_env()` time? The bead INVESTIGATION flagged this as `SECURITY HIGH` but the Locked Decisions scoped SSRF validation only to the CLI install path. Needs a follow-up bead.
- The `proxy_resources: false` default in `install` means resources from installed servers are not proxied. Should this be a CLI flag? Currently Discretion left it as hardcoded default.
- `server.get` always fetches `version = "latest"`. The `ActionSpec` for `server.get` doesn't expose a `version` param to callers. Should be added if users need pinned-version installs.

## Next Steps

**Started but not completed:**
- None — all three waves are committed and tests pass.

**Follow-on tasks:**
- Create PR from `bd-lab-77y5/mcpregistry-service` into `main`.
- Follow-up security bead: validate `MCPREGISTRY_URL` at startup against RFC-1918 blocklist (flagged as `SECURITY HIGH` in bead comments).
- Wave 3 integration tests (noted in bead Testing section): `cli_install_with_remote_calls_gateway_add` and `cli_install_no_remotes_prints_error_and_exits` require wiremock wiring that was deferred.
- TUI `metadata.rs` — add `mcpregistry` to the plugin manager (currently only `radarr` is wired).
