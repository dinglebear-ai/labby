# labby

`labby` composes the Labby product: the operator CLI, MCP server, HTTP API,
static web serving, configuration, installation lifecycle, and shared product
operations. This crate builds the `labby` binary and a library used by that
binary, integration tests, and product composition code.

Start here when changing how a caller reaches a Labby operation. For reusable
SDK contracts, use [labby-apis](../labby-apis/README.md); for upstream transport
and gateway state, use [labby-gateway](../labby-gateway/README.md). The library's
public modules expose product wiring rather than a separate host-neutral SDK.

Read the [crate instructions](AGENTS.md), nearest nested instructions, and
[workspace architecture](../../docs/ARCH.md) before changing ownership.

## Architecture and ownership

CLI, MCP, and HTTP adapters invoke shared dispatch or the owning extracted
runtime. Validation, authorization, destructive classification, retry policy,
and typed errors belong below the adapters. A surface can have its own
transport behavior, such as HTTP byte streaming or MCP elicitation.

[src/lib.rs](src/lib.rs) declares the public library entrypoints:

| Module or interface | Responsibility |
| --- | --- |
| `run()` | Process entrypoint: parse arguments, load configuration, initialize diagnostics, and dispatch the command. |
| `cli` | Clap command graph, offline help/completion, operator commands, and presentation adapters. `Cli` and `Command` define the public CLI grammar. |
| `dispatch` | Shared product operations and adapters to extracted runtimes; `dispatch::error::ToolError` re-exports the shared runtime error. |
| `registry`, `catalog` | Service registration, dispatch capabilities, and shared action discovery. `ToolRegistry` and `build_catalog` connect metadata to surface projections. |
| `mcp` | MCP server, descriptors, resources, prompts, elicitation, and upstream projection. `mcp::server` defines `LabMcpServer`. |
| `api` | Axum routes, authentication/caller middleware, HTTP status mapping, and static asset serving. Public types include `AppState`, `ActionRequest`, and `ApiError`. |
| `config`, `installation`, `durable_state` | Configuration loading, installation path/lock ownership, and offline recovery bundles. |
| `live_gateway` (`gateway`) | Detect and call an existing daemon while preserving selected remote authority. |
| `proxy`, `oauth` | Foreground proxy composition and product OAuth relay/upstream wiring. Shared auth primitives remain in `labby-auth`. |
| `output`, `log_fmt`, `observability` | Aurora human output, log formatting, and product activity events. JSON output stays unstyled. |
| `net`, `process` | Product networking backoff and platform process helpers. Windows FFI remains in `labby-winjob`. |
| `workspace` (`fs`) | Filesystem-browser runtime configuration and HTTP mount policy. |
| `docs` | Code-owned documentation generation and freshness checking. |
| `testkit`, `test_support` | Hidden integration-test support and unit-test-only support; not product extension APIs. |

The process composition root in [cli/serve.rs](src/cli/serve.rs) assembles
transports and runtime managers. Private `composition` seams inject protocol
implementations without making ordinary CLI or dispatch modules import sibling
surfaces. [Architecture boundary tests](tests/architecture_boundaries.rs)
enforce this direction; [orchestration tests](tests/architecture_orchestrator.rs)
track justified cross-service dependencies.

Reusable ownership stays in the extracted crates: `labby-runtime` owns shared
runtime/error contracts, `labby-gateway` owns upstream pools and gateway state,
`labby-codemode` owns the host-neutral runner, `labby-auth` owns authentication
middleware, and `labby-web` owns bundled web assets. Product dispatch adapts
these owners rather than copying their implementations. `labby-model` is a
development dependency only.

## Features and build shapes

[Cargo.toml](Cargo.toml) owns the feature graph. The default is
`gateway-host → gateway → web-ui + skills`, plus the gateway, Code Mode, and
OpenAPI runtime dependencies. Both `gateway-host` and `integrated-gateway`
enable the same `gateway` feature graph; the integrated feature name alone
does not exclude web assets or Skills.

| Feature | Effect |
| --- | --- |
| `gateway` | Gateway runtime, Code Mode, OpenAPI upstream support, web assets, and Skills. |
| `gateway-host` | Default gateway product feature; enables `gateway`. |
| `integrated-gateway` | Enables `gateway`; integration identity/transport behavior also needs its runtime configuration. |
| `web-ui` | Optional `labby-web` asset dependency; can also be selected independently. |
| `skills` | Product Skill/Artifact surfaces; weakly enables `labby-gateway/skills` only when that dependency is present. |
| `fs` | Workspace filesystem browser and its walk/glob dependencies. |
| `lab-admin` | Compiles internal onboarding audit support; runtime registration also requires enablement. |
| `api-docs` | Utoipa-backed API schema generation; distinct from gateway OpenAPI upstream support. |
| `systemd` | Optional Unix `sd-notify` integration. |
| `tailcat` | Gateway plus native Tailcat transport composition; native availability is checked separately. |
| `codemode-microsandbox` | Gateway plus the Code Mode Microsandbox SDK backend. |
| `proxy-testkit` | Gateway/auth test hooks and the `stdio-mcp-fixture` binary; test support only. |
| `all` | `lab-admin`, `api-docs`, `gateway-host`, `fs`, `systemd`, `skills`, and `tailcat`. |

Cargo's `--all-features` also enables `codemode-microsandbox` and
`proxy-testkit`; it is broader than `--features all`. No-default-feature builds
retain the product shell and ungated services. A `skills`-only build does not
implicitly enable the gateway.

Run commands from the workspace root with the repository's pinned toolchain:

```bash
cargo check -p labby --all-targets
cargo check -p labby --no-default-features --features gateway --all-targets
cargo check -p labby --no-default-features --features fs --all-targets
cargo build -p labby --features all
```

The feature-slice contract is executable in
[check-feature-slices.sh](../../scripts/check-feature-slices.sh), including
empty, gateway, integrated, filesystem, Skills, and optional backend slices.
Passing an all-feature build does not prove those smaller slices compile.

Raw Cargo commands compile against available web assets; they do not rebuild
the web static export. The root [Justfile](../../Justfile) distinguishes these
operations:

| Recipe | Result |
| --- | --- |
| `just build` | Build web assets, then all-feature workspace binaries with `release-fast`; no installation. |
| `just build-release` | Build web assets and release binaries, then install through `link-bin`. |
| `just install` | Build and install the local binary through the repository's install recipe. |
| `just host-sync` | Build, copy into the system service location, and restart the service. |

Use [host gateway guidance](../../docs/runtime/HOST_GATEWAY.md) for installation
and supervised runtime changes. A successful compile does not update an
installed executable or running service.

## Configuration and durable state

`LabConfig` in [config.rs](src/config.rs) owns the typed TOML model.
[InstallationPaths](src/installation.rs) selects one root from an absolute
`LABBY_HOME`, otherwise `$HOME/.labby` with `USERPROFILE` as the fallback.
An invalid explicit root fails rather than selecting another installation.

Runtime settings resolve from CLI flags, process environment, the selected
root's `.env`, TOML preferences, then defaults. Existing process values win
over dotenv. Labby reads the selected root's `config.toml` and `.env`; it does
not discover these files from the current directory. TOML is loaded before
runtime dotenv resolution, and metadata-only commands have an offline path.

Keep secrets and credential values in the selected `.env`; keep non-secret
preferences in TOML. Supported configuration writers preserve unrelated
entries, validate the schema, and manage recovery copies. Configuration
versions, secret authority on reload, and settings commit warnings are covered
by [runtime configuration](../../docs/runtime/CONFIG.md). Start from the
[annotated configuration sample](../../.config/config.example.toml), and use
the [generated environment reference](../../docs/generated/env-reference.md)
for exact variable names.

The installation root also contains durable stores such as `access.db`,
`usage.db`, `task-routes.db`, and `codemode_journal.db`; individual runtime
owners manage their schemas. Workspace and File Stash roots have separate
configuration. Preserve database sidecars and installation identity when
moving or recovering state.

`InstallationLifecycleLock` excludes concurrent daemon and offline lifecycle
work. `durable_state::{export_bundle, verify_bundle, restore_bundle}` acquire
offline ownership, validate paths and payload integrity, and authenticate
bundles with an external `LABBY_RECOVERY_KEY_PATH`. These operations reject
Windows until owner-only ACL enforcement exists. The public CLI commands are
`labby state export`, `labby state verify`, and `labby state restore`;
inspect their help before recovery.
See [recovery tests](tests/durable_state_cli.rs) and
[installation lifecycle tests](tests/installation_lifecycle.rs).

## Product surface entrypoints

Offline help comes from Clap and does not start a service:

```bash
cargo run -p labby --bin labby -- help --all
cargo run -p labby --bin labby -- --json help server list
```

For a development daemon, select a separate absolute installation root, then
run an explicit loopback listener. Port `0` requests an available port:

```bash
LABBY_HOME=/absolute/path/to/dev-state cargo run -p labby --bin labby -- serve --host 127.0.0.1 --port 0
```

This is a long-running runtime action: startup opens state and can bootstrap a
local bearer token in the selected `.env`. `labby serve` hosts HTTP API routes,
streamable HTTP MCP at `/mcp`, authentication routes, and the static operator
UI when its assets are available. There is no production Node server.

`labby mcp` selects stdio. With gateway support it can bridge to an existing
daemon; otherwise it can compose a standalone runtime. Explicit `--server`,
`--context`, or configured remote targets must preserve destination-bound
credentials and return an error when that authority fails. They do not silently
execute against local state. See [remote gateway authority](../../docs/design/REMOTE_GATEWAY_TARGET.md)
and [routing regressions](tests/remote_gateway_routing.rs).

Normal service routers accept `action` plus `params`, represented on HTTP by
`api::ActionRequest`. Per-action atomic MCP projections derive from the same
metadata. Input semantics are shared; each transport owns its response
envelope. Use the [generated CLI help](../../docs/generated/cli-help.md),
[generated catalog index](../../docs/generated/README.md),
[MCP contract](../../docs/surfaces/MCP.md), and
[HTTP API contract](../../docs/surfaces/HTTP_API.md) for exact exposed operations.

## Errors, security, and shutdown

`ToolError` comes from `labby-runtime` and retains stable kinds, causes,
origin, recovery, and side-effect information. Keep it typed until the surface
maps it: HTTP uses `ApiError`, MCP uses its tool envelope, and CLI reports
structured JSON under `--json`. Preserve custom serialization and redact
credentials before diagnostics. Follow the [error contract](../../docs/dev/ERRORS.md)
and [agent error envelope](../../docs/contracts/agent-error-contract.md).

Admin authorization and destructive classification are independent. Listing
filters do not replace direct-call authorization. Caller, Team, route,
loadout, and OAuth-subject boundaries must survive every adapter. MCP-owned
destructive elicitation follows the [MCP instructions](src/mcp/AGENTS.md);
HTTP and CLI apply their own shared-policy projections.

HTTP startup rejects a non-loopback bind without authentication. The browser
UI auth setting does not disable configured bearer/OAuth auth on `/v1` or
`/mcp`. With `fs`, HTTP mount policy also considers API authentication;
stdio exposure relies on controlling who can launch or reach the process.
File Stash registers only on Linux at runtime. Tailcat's native helper is
supported on Linux and macOS and requires its configured authentication and
validated helper artifacts; compiling the feature does not enable it.

Hosted shutdown drains the listener within bounded time and stops owned
runtime workers, including the Code Mode runner pool. The binary runs the
async entrypoint on a dedicated stack and applies a bounded Tokio runtime
shutdown. Changes to process ownership need platform-specific evidence,
including Windows Job Object tests where applicable.

## Verification

Focused product checks, run from the workspace root:

```bash
cargo nextest run -p labby --all-features --lib cli::
cargo nextest run -p labby --all-features --test cli_contract
cargo nextest run -p labby --all-features --test startup_config --test remote_gateway_routing
cargo nextest run -p labby --all-features --test architecture_boundaries --test architecture_orchestrator
cargo nextest run -p labby --all-features
```

[Testing](../../docs/dev/TESTING.md) owns live/ignored-suite prerequisites,
fixture supervision, and required checks. Nextest excludes doctests; use
`just rustdoc-check` for strict Rustdoc and workspace doctests. Feature changes
also require `bash scripts/check-feature-slices.sh`. Discovery, metadata, CLI,
and schema changes require `just docs-generate` followed by `just docs-check`.
Run workspace `just check`, `just test`, and `just lint` as required by the
change. Process tests and hermetic fixtures do not establish live provider or
browser qualification; the testing contract describes those separate tiers.
