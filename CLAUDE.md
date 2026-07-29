# Labby — Development Instructions

## What is this?

`labby` is a Rust MCP gateway and homelab control plane. One binary exposes CLI, MCP (stdio + streamable HTTP + Unix socket), HTTP API, and Labby web UI surfaces over the same product dispatch layer. The retired ACP, Registry-browser, Marketplace-product, Fleet, Deploy-product, Stash, and device-runtime implementations are deleted from current source, manifests, packaging, and CI. MCP dispatch uses a single tool per runtime service with an `action` + `params` shape instead of hundreds of per-method tools.

The dispatch modules that actually exist today are `crates/labby/src/dispatch/{doctor,fs,gateway,lab_admin,server_logs,setup,snippets}`. `doctor`, `server_logs`, `setup`, and `snippets` (gateway-gated) are always-on operator services, while `lab_admin` is runtime-conditional. The CLI surface is `serve`, `mcp`, `doctor`, `docs`, `health`, `logs`, `setup`, `incus`, `update`, `completions`, `gateway`, `snippets`, `oauth`, and `help` — there is no `marketplace`, `stash`, `nodes`, `deploy`, or `acp` command. `docs/generated/cli-help.md` is the authoritative snapshot; regenerate rather than hand-editing command lists.

Start with `docs/README.md` for the docs index. The topic docs in `docs/` are the source of truth; if this file disagrees with them, this file is stale.

Observability is governed by `docs/dev/OBSERVABILITY.md`. When adding or changing request paths, treat that file as the source of truth for logging boundaries, required fields, correlation, redaction, and verification.
Errors are governed by `docs/dev/ERRORS.md`. Serialization and output-boundary rules are governed by `docs/design/SERIALIZATION.md`.
Shared dispatch ownership and adapter direction are governed by `docs/dev/DISPATCH.md`.

## Repo Facts

| Fact | Value |
|------|-------|
| Remote | `git@github.com:dinglebear-ai/labby.git` (the older `jmagar/lab` and `jmagar/labby` names survive only via GitHub transfer redirects — prefer the canonical URL) |
| Default branch | `main` |
| Cargo workspace | 11 members, `resolver = "3"`, single `[workspace.package]` version |
| Edition / MSRV | edition 2024, `rust-version = "1.92"`, toolchain pinned to 1.94.1 in `rust-toolchain.toml` |
| MCP SDK | `rmcp = "=3.0.0-beta.2"` — exact pin in `[workspace.dependencies]`, and the only repo in the fleet on rmcp 3.x. Bumping it is a breaking change across `crates/labby/src/mcp/` and `crates/labby-gateway/`. |
| Lint enforcement | `[workspace.lints]` is real here: `unsafe_code = "forbid"`, `mod_module_files = "deny"`, `disallowed_macros = "deny"` (see `/clippy.toml` — bans `#[async_trait]`) |
| Config / secrets | `~/.labby/config.toml` and `~/.labby/.env`; `$LABBY_HOME` overrides the `~/.labby` root |
| Worktrees | This checkout carries a busy `.worktrees/` (currently 9 active worktrees). Run `git worktree list` before any workspace-level edit, and never assume the main checkout is the only consumer of `target/`. |

**Build assumption.** This repo is developed and verified as an **all-features** binary. Treat `cargo build --workspace --all-features`, `cargo nextest run --workspace --all-features`, and the equivalent `just` commands as the default truth. The only standalone product feature slices left on `labby` are `gateway` and `fs` (CI job `feature-slices`, matrix `[gateway, fs]`, built with `--no-default-features --features <slice>`); `gateway` is the flagship slice and additionally runs its own nextest suite. Use slices to catch accidental cross-slice coupling, but check warning/removal decisions against the all-features build before deleting shared helpers — per-slice dead-code warnings are an expected consequence of disabling features. Never reintroduce retired feature flags as compatibility aliases.

The full `labby` feature list is `all = ["lab-admin", "api-docs", "gateway-host", "fs", "systemd"]`, with `default = ["gateway-host"]` and `gateway-host = ["gateway"]`.

**Service onboarding rule.** When bringing a service online, follow the dispatch/module layout in `docs/dev/SERVICE_ONBOARDING.md`, update generated docs, then validate with the all-features test/build path. The older `labby scaffold service` / `labby audit onboarding` workflow is not part of the current CLI surface unless those commands are restored in code.

**Nested guides.** Subdirectories carry their own `CLAUDE.md` with rules that don't belong at the root. Read the nearest one when working in:
- `crates/labby-apis/src/core/` — trait contracts, error taxonomy, HttpClient invariants
- `crates/labby/src/dispatch/` — product dispatch layer, required service layout, canonical templates
- `crates/labby-gateway/src/upstream/` — upstream MCP proxy pool, circuit breaker, layer contract
- `crates/labby/src/mcp/` — dispatch, envelopes, elicitation, catalog
- `crates/labby/src/cli/` — thin-shim pattern, destructive flags, batch commands
- `crates/labby/src/api/` — axum HTTP surface, status code mapping, middleware stack

## Repository Structure

The workspace is split into reusable crates plus one product binary crate. A
dependency-free leaf crate, `labby-primitives`, holds the small vocabulary
types (`ActionSpec`/`ParamSpec`, `PluginMeta`/`EnvVar`/`Category`, `UiSchema`,
static SSRF checks) shared by both the SDK and the gateway-extraction crates.
Pure SDK/domain clients live in `labby-apis`, which re-exports those types from
`labby-primitives`. HTTP/OAuth auth middleware and upstream OAuth runtime live
in `labby-auth`. Shared transport-neutral contracts and helpers (`ToolError`,
gateway config DTOs, redaction, path-safety, backoff) live in `labby-runtime`.
Code Mode execution lives in `labby-codemode`. Gateway runtime/proxy
orchestration — including its own dispatch helpers and the stdio spawn-guard/
SSRF security checks — lives in `labby-gateway`. OpenAPI schema assembly lives
in `labby-openapi`. Embedded/static web serving lives in `labby-web`. Windows
process-tree reaping lives in `labby-winjob`. CLI, MCP, HTTP API adapters,
config loading, product dispatch, and the `labby` binary live in `labby`.
Repo automation tasks live in the non-published `xtask` crate.

The 11 workspace members are exactly: `labby-primitives`, `labby-apis`,
`labby-auth`, `labby-runtime`, `labby-codemode`, `labby-gateway`,
`labby-openapi`, `labby-web`, `labby-winjob`, `labby`, and `xtask`. Adding or
removing a crate means editing `[workspace] members` in the root `Cargo.toml`
and updating this list.

```
labby/
├── crates/
│   ├── labby-primitives/             # Leaf crate: ActionSpec/ParamSpec, PluginMeta/EnvVar/Category,
│   │   │                             # UiSchema, static SSRF checks. Zero internal deps.
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── action.rs
│   │       ├── plugin.rs
│   │       ├── plugin_ui.rs
│   │       └── ssrf.rs
│   │
│   ├── labby-apis/                   # PURE Rust SDK — reusable in any binary
│   │   ├── Cargo.toml                # deps: reqwest, serde, thiserror, tokio, labby-primitives
│   │   └── src/
│   │       ├── lib.rs                # re-exports, feature gates
│   │       ├── core/                 # HttpClient, Auth, errors, traits; action/plugin/plugin_ui/ssrf
│   │       │                         # are thin re-exports of labby-primitives
│   │       ├── doctor/                # doctor pure data/client helpers
│   │       └── setup/                 # setup pure data/client helpers
│   │
│   ├── labby-auth/                   # HTTP/OAuth auth middleware and storage
│   ├── labby-runtime/                # ToolError, config DTOs, path/redaction/backoff helpers
│   ├── labby-codemode/               # Code Mode runner kernel + snippet engine
│   ├── labby-gateway/                # Gateway manager, upstream pool, OAuth lifecycle,
│   │                                 # dispatch helpers, stdio spawn-guard/SSRF checks
│   ├── labby-openapi/                # OpenAPI 3.1 schema assembly for the HTTP surface
│   ├── labby-web/                    # Embedded/filesystem web asset serving
│   ├── labby-winjob/                 # Windows Job Object helper crate
│   ├── xtask/                        # Repo automation tasks (not published)
│   └── labby/                        # BINARY: cli + mcp + api + product dispatch
│       ├── Cargo.toml                # deps: labby-*, clap, rmcp, axum, anyhow
│       └── src/
│           ├── main.rs
│           ├── api.rs                # axum surface module declaration
│           ├── catalog.rs            # build_catalog() — single source for help/resource/CLI
│           ├── cli/                  # clap subcommands per service (thin shims)
│           ├── cli.rs
│           ├── mcp/
│           │   ├── registry.rs       # runtime tool registration
│           │   ├── server.rs         # ServerHandler impl (rmcp 3.x)
│           │   ├── handlers_tools/   # list_tools/call_tool handlers
│           │   ├── call_tool.rs      # product + upstream + Code Mode call routing
│           │   ├── catalog.rs        # upstream tool catalog, coalescing, churn tracking
│           │   ├── peers.rs          # per-peer contract negotiation
│           │   ├── elicitation.rs    # MRTR destructive-action elicitation
│           │   ├── resources.rs      # action catalog as MCP resources
│           │   ├── error.rs          # structured JSON errors
│           │   └── services/         # one dispatch module per service
│           ├── mcp.rs
│           ├── dispatch/             # shared product dispatch (doctor, fs, gateway,
│           │                         # lab_admin, server_logs, setup, snippets)
│           ├── api/                  # axum HTTP API
│           │   ├── state.rs          # AppState — Catalog + ToolRegistry (Arc-wrapped)
│           │   ├── error.rs          # ApiError + IntoResponse mapping
│           │   ├── router.rs         # build_router() + middleware stack
│           │   ├── health.rs         # /health + /ready endpoints
│           │   └── services/         # per-service route groups
│           ├── config.rs             # ~/.labby/.env + config.toml loading (CWD → ~/.labby/ → ~/.config/labby/)
│           └── output.rs             # table/json formatting
├── apps/gateway-admin/               # Next.js Labby web UI, statically exported
├── packages/labby-mcp/               # npm launcher wrapper (`npx -y labby-mcp mcp`)
├── plugins/                          # Claude/Codex plugin assets (labby, vibin, testing, …)
├── scripts/                          # install.sh/install.ps1, incus-bootstrap.sh, CI helpers
├── openwiki/                         # generated repository wiki
├── Cargo.toml                        # workspace: 11 members, resolver 3, shared lints
├── rust-toolchain.toml               # pinned 1.94.1 (MSRV is 1.92)
├── Justfile
├── clippy.toml                       # disallowed_macros config (bans #[async_trait])
├── deny.toml
├── docs/README.md
└── CLAUDE.md
```

## Key Patterns

### Per-Service Module Structure (in `labby-apis`)

Every service is a module under `crates/labby-apis/src/`:

```
foo.rs              # module declaration: pub mod client; pub mod types; pub mod error; pub const META: ...
foo/
├── client.rs       # FooClient with async methods — ALL business logic
├── types.rs        # Request/response types (serde)
└── error.rs        # Service-specific errors (thiserror)
```

Modern Rust module style: **no `mod.rs` files anywhere**. A module `foo` is declared in `foo.rs` (sibling to the `foo/` directory), not in `foo/mod.rs`.

Note: `commands.rs` and `tools.rs` do **not** live here. CLI subcommands and MCP dispatch live in the `labby` crate, never in `labby-apis`.

### The Golden Rule

Business logic lives in `labby-apis/src/<service>/client.rs`. Shared product semantics live in `crates/labby/src/dispatch/<service>/`. CLI, MCP, and HTTP are thin adapters over dispatch unless a surface has a genuine protocol-specific exception. If you're writing business logic in a CLI command, MCP handler, or API route, you're doing it wrong — move it to the client or shared dispatch layer.

The crate split enforces this structurally: `labby-apis` doesn't depend on `clap` or `rmcp`, so you literally cannot reach for them while writing business logic.

### One Tool Per Service (MCP) — action + subaction dispatch

Each service exposes exactly **one** MCP tool, named after the service. Operations dispatch via a flat dotted `action` string + free-form `params` object. This keeps total MCP tool count near the service count, not hundreds.

```jsonc
marketplace({ "action": "mcp.list", "params": { "search": "github", "limit": 10 } })
gateway({ "action": "gateway.list" })
marketplace({ "action": "help" })                        // built-in discovery
marketplace({ "action": "schema", "params": { "action": "mcp.install" } })  // per-action schema
```

- **Action naming:** `<resource>.<verb>`, lowercase, dot-separated.
- **Built-in actions:** every tool accepts `help` and `schema` without declaring them.
- **Discovery:** `lab://<service>/actions` MCP resource + `lab://catalog` resource.
- **Shared catalog.** `build_catalog()` is a single function feeding the `lab://catalog` MCP resource and the `labby help` CLI subcommand. Never duplicate catalog logic — extend the builder.
- **Multi-instance services.** When `{SERVICE}_{LABEL}_URL` env vars exist, callers pass `params.instance: "<label>"`. Unknown labels return a structured `unknown_instance` envelope listing valid labels.

### Destructive actions

`ActionSpec.destructive: bool` is the **single source of truth** for dangerous operations. It drives:

- **MCP:** 2026-07-28 MRTR elicitation — the dispatcher returns
  `input_required`, then validates the elicitation answer from the retried
  request's `inputResponses`.
- **CLI:** requires `-y` / `--yes` to run non-interactively. `--no-confirm` and `--dry-run` are also honored.

Mark actions `destructive: true` whenever they delete, overwrite, spawn local processes, or push state that cannot be trivially reversed, including `gateway.test`, `gateway.remove`, protected-route mutation, and setup repair actions.

### Structured error envelopes

Every MCP tool failure returns a JSON envelope with a stable `kind` tag so agents can react programmatically:

```jsonc
{ "kind": "unknown_action", "message": "...", "valid": ["movie.search", ...], "hint": "movie.serch" }
{ "kind": "missing_param",  "message": "...", "param": "query" }
{ "kind": "unknown_instance", "message": "...", "valid": ["default", "node2"] }
{ "kind": "rate_limited", "message": "...", "retry_after_ms": 5000 }
```

See `docs/surfaces/MCP.md` for the MCP surface and `docs/CONVENTIONS.md` for the canonical error vocabulary rules.

`docs/dev/ERRORS.md` is the canonical source of truth for stable kinds, envelope expectations, and status mapping.

### Adding a New Service

1. `mkdir crates/labby-apis/src/foo/`
2. Define types in `types.rs` from API spec/docs
3. Implement `FooClient` methods in `client.rs`
4. Add observability at the shared boundary and confirm it matches `docs/dev/OBSERVABILITY.md`
5. Implement `ServiceClient` trait for health checks
6. Add `#[cfg(feature = "foo")] pub mod foo;` to `labby-apis/src/lib.rs`
7. Add `foo = []` feature to `crates/labby-apis/Cargo.toml`
8. Create the shared dispatch layer in `crates/labby/src/dispatch/foo/` following the required layout in `crates/labby/src/dispatch/CLAUDE.md` (catalog.rs, client.rs, params.rs, dispatch.rs + entry `foo.rs`)
9. Create CLI subcommands in `crates/labby/src/cli/foo.rs` calling the dispatch layer
10. Create API route group in `crates/labby/src/api/services/foo.rs` calling the dispatch layer
11. Register in `crates/labby/src/registry.rs` (call `reg.register(RegisteredService { .. })` inside `build_default_registry()`, using the `dispatch_fn!` macro for the dispatch pointer), `crates/labby/src/cli.rs`, and `crates/labby/src/api/router.rs`
12. Add `foo = ["labby-apis/foo"]` passthrough to `crates/labby/Cargo.toml`

A service is not fully online until one successful path and one failing path are traceable end to end without leaking secrets.

### Auth

Use the `Auth` enum from `labby_apis::core`. Never hardcode auth handling in a service module.

```rust
use labby_apis::core::{Auth, HttpClient};

impl FooClient {
    pub fn new(base_url: &str, auth: Auth) -> Self {
        Self {
            http: HttpClient::new(base_url, auth),
        }
    }
}
```

### Config Loading

**`labby-apis` never reads files or env vars on its own.** Config loading lives entirely in `crates/labby/src/config.rs`. The library exposes optional `from_env()` helpers; the binary calls them.

Naming convention for env vars (read by `labby`, not `labby-apis`):
- `{SERVICE}_URL` — base URL
- `{SERVICE}_API_KEY` — API key (for ApiKey auth)
- `{SERVICE}_TOKEN` — token (for Token/Bearer auth)
- `{SERVICE}_USERNAME` / `{SERVICE}_PASSWORD` — credentials (for Basic auth)

**Multi-instance services:** append a label before the suffix — `UNRAID_URL` is the default instance, `UNRAID_NODE2_URL` / `UNRAID_NODE2_API_KEY` is an additional named instance `node2`. MCP callers select via `params.instance`; CLI selects via `--instance` or positional label. Never hardcode instance names — derive them from env at startup.

Loaded from `~/.labby/.env`. Product actions that mutate config or env files must use backup-first, atomic-write behavior and preserve unrelated keys/comments where the file format allows it.

### PluginMeta shape

Every service entry-point file that participates in generated metadata declares a `pub const META: PluginMeta` with:

- `category: Category` — one of 10 variants: `Media`, `Servarr`, `Indexer`, `Download`, `Notes`, `Documents`, `Network`, `Notifications`, `Ai`, `Bootstrap`.
- `required_env: &[EnvVar]` / `optional_env: &[EnvVar]` — each `EnvVar { name, description, example, secret }`. `secret: true` marks values to mask in logs, docs, and UI.
- `default_port: Option<u16>` — used by generated docs and doctor/setup hints.

### Error Handling

- `labby-apis`: use `thiserror` for typed errors per service; every service error wraps `ApiError` transparently.
- `labby` binary: use `anyhow` to wrap everything.
- Always return `Result<T>`, never panic.
- `docs/dev/ERRORS.md` is canonical for stable `kind` values, dispatcher-level kinds, MCP and HTTP envelope behavior, and status mapping.
- Do not invent service-local error vocabularies or drift MCP and HTTP error semantics apart.
- Adding or renaming an error `kind` is a spec change and must be reflected in the owning docs and surface code together.

### Logging

Use `tracing` everywhere. Never use `println!` for debug info.

`docs/dev/OBSERVABILITY.md` is the canonical source of truth. Do not invent per-service log shapes.

Minimum required rules:

- CLI, MCP, and HTTP dispatch must emit one structured dispatch event per user-visible action
- `HttpClient` must emit `request.start` and `request.finish` or `request.error` for every outbound request
- request logs must inherit caller context from the invoking surface
- health probes must be distinguishable from normal actions
- destructive actions must log intent and outcome
- secrets, auth headers, tokens, cookies, and secret env values must never be logged

**Standard dispatch fields** — all dispatch events must include these:

| Field | Type | Present when |
|-------|------|--------------|
| `surface` | `&str` | always |
| `service` | `&str` | always (MCP/HTTP/CLI dispatch) |
| `action` | `&str` | always |
| `elapsed_ms` | `u128` | always |
| `kind` | `&str` | errors only — from `ToolError::kind()` |

HTTP dispatch additionally carries `request_id` when available. Outbound request events carry `method`, `path`, `host`, and `status` on success.

**Level conventions:**
- `INFO` — successful dispatch
- `WARN` — user/caller errors (`missing_param`, `unknown_action`, `auth_failed`, etc.)
- `ERROR` — unhandled / fatal errors (panics, internal_error)

**Environment variables:**
- `LABBY_LOG` — tracing filter directive (default: `labby=info,labby_apis=warn`)
- `LABBY_LOG_FORMAT=json` — emit newline-delimited JSON (for prod/CI)
- `LABBY_LOG_COLOR=force` — force ANSI colors even without a TTY (e.g. `docker compose logs -f`); also accepts `plain`/`never`/`0` to disable colors

ANSI colors are enabled only when `stderr` is a TTY (`std::io::stderr().is_terminal()`), or when `LABBY_LOG_COLOR=force` is set.

The product API surface uses `surface = "api"` in dispatch logs. Keep docs, tests, and new instrumentation aligned with that label.

### Async trait style

Use **native `async fn in trait`** (stable in Rust 1.75+). Do **not** add the `async-trait` crate. Do **not** use `Box<dyn ServiceClient>` — prefer generics or concrete types. This is a hard rule; PRs that reintroduce `#[async_trait]` will be rejected.

### Output Formatting

All formatting lives in `crates/labby/src/output.rs`. `labby-apis` types are pure data.

`docs/design/SERIALIZATION.md` is the canonical source of truth for serde ownership, stable envelopes, and output boundaries.

- Support `--json` by serializing the underlying `labby-apis` type with `serde_json`
- Use `tracing` for debug/verbose output, never `println!` for debug info

## Tech Stack

| Crate | Purpose | Lives in |
|-------|---------|----------|
| tokio | async runtime | both |
| reqwest | HTTP client (rustls-tls) | labby-apis |
| serde + serde_json | serialization | labby-apis |
| thiserror | library errors | labby-apis |
| wiremock | HTTP mocking (tests) | labby-apis |
| clap | CLI parsing (derive) | labby |
| rmcp | MCP server | labby |
| dotenvy | .env loading | labby |
| toml | config parsing | labby |
| tracing | structured logging | labby |
| anyhow | binary errors | labby |

## Dev Commands

```bash
just check          # cargo check --workspace --all-features
just test           # cargo nextest run --workspace --all-features
just test-integration # nextest --run-ignored ignored-only (needs live services)
just lint           # skill-drift + cargo-wrapper smoke, then clippy -D warnings + fmt --check
just deny           # cargo deny check
just build          # cargo build --workspace --all-features --profile release-fast
just build-release  # release build + install to bin/labby + symlink ~/.local/bin/labby
just install        # build-release + symlink
just docs-generate  # labby docs generate — refresh docs/generated/*
just docs-check     # labby docs check — fail if generated artifacts are stale
just web-build      # cd apps/gateway-admin && pnpm build
just host-sync      # rebuild + install binary + restart the host labby service
just run -- help    # cargo run --all-features -- <args>
just fmt            # cargo fmt --all
just clean          # cargo clean
just mcp-token      # rotate LABBY_MCP_HTTP_TOKEN in ~/.labby/.env
```

`just build` uses the `release-fast` profile (optimized, no LTO / `codegen-units=1`)
rather than a debug build — run `cargo build --workspace --all-features` directly
when you need debug assertions and full unwinding. `just lint` is a superset of
clippy+fmt: it also runs `plugins/scripts/check-dozzle-skill` (skill drift) and
`scripts/test-cargo-rustc-wrapper.sh`, so a bare clippy run is not equivalent.

Generated artifacts under `docs/generated/` (service catalog, action catalog,
env reference, API routes, OpenAPI, feature matrix, MCP help, CLI help) are
code-owned. Never hand-edit them; run `just docs-generate` and verify with
`just docs-check` — CI enforces freshness in the `docs-check` job.

Releases: `release-please.yml` watches green CI runs on `main`, maintains a
release PR that bumps `[workspace.package] version` in `Cargo.toml` (and the
matching `Cargo.lock` entries) and updates `CHANGELOG.md` from Conventional
Commits (`release-please-config.json` / `.release-please-manifest.json`).
Merging that PR creates the `vX.Y.Z` tag and GitHub Release, which triggers
`release.yml` to build the Linux/Windows archives, publish the release, and
push the GHCR image. Requires the `RELEASE_PLEASE_TOKEN` repo secret (a PAT
or GitHub App token with `contents: write` + `pull-requests: write` — the
default `GITHUB_TOKEN` won't trigger the downstream tag-push workflow).

Default verification targets the all-features build. If you run a reduced feature set for a narrow task, treat any warning cleanup decisions from that mode as provisional until they are checked again with `--all-features`.

### Operator tooling

- **`labby doctor`** — comprehensive health audit: checks env vars, reachability, auth, version for every enabled service. Emits human-readable table by default, `--json` for CI. Exit code reflects worst severity.
- **`labby health`** — lightweight liveness/readiness probe against a running `labby serve`.
- **`plugins/scripts/health-check`** — repo-level shell helper for CI/CD smoke tests. (`bin/` holds built binaries, not scripts.)

### Labby gateway runtime

The recommended self-hosted Labby gateway runtime is an amd64 Ubuntu 24.04
Incus system container, with bare metal as the secondary supported shape for a
dedicated gateway host or VM. The host-side Incus entrypoint is
`scripts/incus-bootstrap.sh --version vX.Y.Z`; the in-box converger is
`labby setup --provision`. The provision command is intentionally local
CLI-only and must not be exposed through MCP, HTTP, Code Mode, or remote admin
actions.

The default service is a hardened system unit at
`/etc/systemd/system/labby.service`, running `User=labby`, `Group=labby`, and
`ExecStart=/usr/local/bin/labby serve`. Do not reintroduce `systemd --user`,
linger, `%h` unit paths, or `~/.local/bin/labby` as the supported self-hosted
gateway service model. Preserve a user-service fallback only if it is explicit
and clearly non-default.

The Docker Compose stack is supported only for explicit dev-container and
prod-like image smoke. The image may include pinned provider CLIs for stdio
upstreams, but it must not reintroduce retired protocol adapters or product state.
Use `just dev-container` or `just dev-container-debug` when testing that path.

### Bearer auth in dev (driving the UI with agent-browser)

When OAuth is configured (`LABBY_AUTH_MODE=oauth`), browser users still hit the Google login flow. Automation tooling (e.g. `agent-browser`, curl) can pass the static bearer token as a header and be treated as an admin session for both `/v1/*` API calls AND the AuthBootstrap session-state endpoint.

```bash
TOKEN=$(awk -F= '/^LABBY_MCP_HTTP_TOKEN=/{print $2}' ~/.labby/.env)

# Generic /v1/{service} action dispatch
curl -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"gateway.list","params":{}}' \
  http://localhost:8765/v1/gateway

# /auth/session — returns synthetic admin session for the bearer holder.
# Without this the UI's AuthBootstrap renders the sign-in page even though
# the underlying API calls succeed.
curl -H "Authorization: Bearer $TOKEN" http://localhost:8765/auth/session

# agent-browser carries the header into every same-origin request.
agent-browser --session test set viewport 1280 800
agent-browser --session test open http://localhost:8765/gateways \
  --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
```

The bearer-via-`/auth/session` path returns `sub: "static-bearer"` so admin-gated UI is reachable. OAuth users see no behavior change — the cookie path is still primary.

Scoped to a single crate:

```bash
cargo nextest run -p labby-apis        # client tests only (fast, wiremock-based)
cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features  # CLI/MCP/API tests
```

## Testing

- Unit tests: mock HTTP with `wiremock` in `labby-apis`, run in CI
- Integration tests: hit real services, run locally only (marked `#[ignore]`)
- Test runner: `cargo-nextest` (parallel execution)
- The authoritative test/build signal is the all-features workspace run, not a partial-feature slice
- If a helper or module looks unused in a reduced build, confirm with an all-features search/build before removing it

```bash
# Unit tests (CI-safe)
just test

# Integration tests (requires running services)
just test-integration
```

## CI

- GitHub Actions, `.github/workflows/ci.yml`, gated behind a single `ci-gate` job.
- Platforms: Linux x86_64 for the main jobs, plus dedicated `test-windows` and
  `palette-windows` jobs. There is no aarch64 CI or release target.
- Rust checks: `fmt`, `clippy` (`-D warnings`), `deny`, `check`, `msrv` (1.92.0),
  `test` / `test-fork`, `rust-coverage`.
- Slice checks: `feature-slices` (`gateway`, `fs`) and `extracted-crate-slices`
  (per-feature checks of `labby-auth`, `labby-runtime`, and friends).
- MCP checks: `mcp-regressions`, `mcp-conformance`, `codemode-runner-smoke`,
  and the separate `mcp-upstream-drift.yml` workflow.
- Other: `docs-check`, `frontend-assets`, `gateway-admin-browser`, `npm-launcher`,
  `actionlint`, `unraid-plugin-check`, `container`, `release-smoke`.
- Release: `release-please.yml` maintains the release PR; merging it tags
  `vX.Y.Z` and triggers `release.yml`, which builds
  `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc` archives with
  checksums, publishes the GitHub Release, and pushes the GHCR image.
  aarch64 Linux was dropped because `rquickjs-sys` (Code Mode QuickJS) cannot
  cross-compile for it and no fleet host is aarch64.

## Style

- Rust 2024 edition; toolchain pinned to 1.94.1, MSRV 1.92
- `cargo fmt` with default settings
- `cargo clippy` with no allowed warnings
- `unsafe_code = "forbid"` workspace-wide. The one exception is `labby-winjob`,
  which is a separate crate precisely so the main workspace can keep the ban.
- No `#[async_trait]`. `disallowed_macros = "deny"` in `[workspace.lints.clippy]`
  plus `/clippy.toml` enforce this at compile time, not by review.
- Treat all-features warnings as real; treat narrow feature-slice warnings as diagnostic only until confirmed in the normal all-features build
- Prefer `impl Trait` over `Box<dyn Trait>` where possible
- Prefer concrete types over generics unless sharing demands it
- Never add `clap`, `rmcp`, `axum`, or `anyhow` to `labby-apis` — they belong in product/runtime crates only
- **No `mod.rs` files.** Modern Rust module style only: a module `foo` is declared in `foo.rs` sibling to its `foo/` directory, never in `foo/mod.rs`

## Plugin setup and install flow

Plugin setup is owned by the binary. `labby setup check` is read-only,
`labby setup repair` is idempotent, and `labby setup plugin-hook --no-repair`
is audit mode. Those commands remain part of the CLI surface and are exercised
by `just validate-plugin`.

**`plugins/labby` ships skills, an MCP config, and `userConfig` only — no
Claude Code hooks and no binary.** The former `plugins/labby/hooks/hooks.json`
(SessionStart / ConfigChange shims) has been removed; do not reintroduce a
`hooks/` directory or a `hooks` key in `.claude-plugin/plugin.json`. Operators
run `labby setup` themselves.

Installation is explicit: `scripts/install.sh` (release download →
`~/.local/bin/labby`, cargo fallback) or `cargo install`, then `labby setup`
for the first-run flow. Never bundle a binary into `plugins/labby/bin/`,
reference `${CLAUDE_PLUGIN_ROOT}/bin/labby`, or add Docker Compose, systemd, or
service bootstrap logic to any plugin asset.

Note: `apps/gateway-admin/hooks/` and `apps/gateway-admin/lib/hooks/` are React
hooks (application source code). They are unrelated to Claude Code hooks and
must not be touched by plugin-hook cleanup.
