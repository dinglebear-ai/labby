<!-- Absolute raw URL, not relative: unraid/ca/labby.xml points Community Applications at this file's raw URL, and CA renders the markdown outside any repo context where a relative path would 404. Markdown image syntax, not <img>: the fleet repository contract's readme_lead() skips lines starting with '![' when locating the lead paragraph, but not raw HTML tags. Keep this comment on ONE line for the same reason -- only a comment's first line is skipped. -->
![Labby](https://raw.githubusercontent.com/dinglebear-ai/labby/main/docs/assets/brand/labby-banner.png)

# Labby

Rust MCP gateway with Code Mode, authentication, setup, logs, CLI, HTTP API, and operator web UI.

Canonical remote: `git@github.com:dinglebear-ai/labby.git`.

The root README is the public entrypoint. The topic docs in
[docs/](./docs/README.md) own the detailed contracts; when this file and a topic
doc disagree, fix the topic doc first and then refresh this summary.

## Contents

- [What Labby Does](#what-labby-does)
- [Quick Start](#quick-start)
- [Core Workflows](#core-workflows)
- [Runtime Surfaces](#runtime-surfaces)
- [Configuration](#configuration)
- [Current Catalogs](#current-catalogs)
- [Architecture](#architecture)
- [Development](#development)
- [Documentation](#documentation)

## What Labby Does

Labby is centered on the current gateway/operator surface:

- **MCP gateway** - connect HTTP and stdio upstream MCP servers, inspect their
  tools/resources/prompts, apply exposure filters, publish protected MCP routes,
  and optionally collapse the upstream catalog into Code Mode `search` and
  `execute`.
- **Direct stdio proxy** - launch one stdio MCP server with
  `labby proxy /path/to/dist.js` and expose its unmodified MCP surface over
  loopback or an owned Tailscale Serve HTTPS port with tailnet, bearer, OAuth,
  or explicit no-auth policy.
- **Authentication and protected routes** - run bearer or OAuth authentication,
  manage route-scoped access, authorize upstream OAuth connections, and publish
  protected MCP endpoints.
- **Code Mode snippets** - author, store, and run reusable JavaScript snippets
  against the upstream catalog, with artifacts persisted under `$LABBY_HOME`.
- **Setup and doctor** - bootstrap `~/.labby`, provision the host service, and
  run a health audit across env, reachability, auth, and versions.
- **Filesystem service** - scoped, path-safety-checked file operations exposed
  through the same action dispatch as every other service.
- **Server logs** - search and tail the local `labby serve` log stream.
- **Incus and bare-metal setup** - provision and operate a dedicated Labby
  gateway host without introducing a separate fleet or deployment product.
- **Generated discovery** - publish code-owned service, action, environment,
  proxy configuration, API route, OpenAPI, MCP help, CLI help, and
  feature-matrix artifacts under
  [docs/generated](./docs/generated/README.md).

The registered services on this branch are exactly `doctor`, `fs`, `gateway`,
`lab_admin`, `server_logs`, `setup`, and `snippets`. The older
`marketplace`/`stash`/`acp`/`nodes`/`deploy`/`logs`/`device` product services
were removed by the "Slim labby gateway host" pass; their domain types survive
in `labby-runtime`/`labby-apis` but are no longer registered services and have
no CLI commands. Use the generated catalogs below for the current surface
instead of copying command or action lists by hand.

## Quick Start

### Proxy One Stdio MCP Server

After installing Labby, configure proxy defaults once and launch a JavaScript
stdio server without proxy flags:

```bash
labby setup proxy
labby doctor proxy
labby proxy /path/to/dist.js
```

The built-in zero-flag policy is Tailscale Serve plus tailnet authorization on
a random high port. Child flags follow the first child token unchanged, and an
explicit separator is available for unusual commands:

```bash
labby proxy /path/to/dist.js --workspace /srv/data --read-only
labby proxy -- npx -y @modelcontextprotocol/server-filesystem /srv/data
```

Use `labby proxy --local --auth none ...` for explicit loopback-only
development. Bearer and OAuth setup, exact-port resource audiences, safe Serve
ownership, configuration precedence, output modes, and recovery are covered in
the [stdio MCP proxy guide](./docs/guides/STDIO_MCP_PROXY.md).

### Install A Release

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/dinglebear-ai/labby/main/scripts/install.sh | sh
labby setup
labby serve --host 127.0.0.1 --port 8765
```

MCP clients that prefer npm launchers can run Labby through the Node wrapper:

```bash
npx -y @dinglebear/labby mcp
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/dinglebear-ai/labby/main/scripts/install.ps1 | iex
labby setup
labby serve --host 127.0.0.1 --port 8765
```

The install scripts download the requested GitHub Release asset, verify its
checksum, and install `labby` onto the user PATH. They do **not** perform
operator provisioning or environment setup. The scripts only install the binary
(from a release or fallback source build); all first-run provisioning is handled
inside `labby` via `labby serve` bootstrap and `labby setup`.

Override install behavior with `LABBY_INSTALL_DIR`, `LABBY_INSTALL_VERSION`, or
`LABBY_INSTALL_REPO`.

### Build From Source

Prerequisites:

- Rust 1.97.1 or newer. CI/release verifies with Rust 1.97.1.
- `just` for repo commands.
- `cargo-nextest` for the main test suite.
- `pnpm 9.15.9` for the Labby web UI. The repo pins this in
  [.mise.toml](./.mise.toml) and
  [apps/gateway-admin/package.json](./apps/gateway-admin/package.json).
- `openssl` if you want to generate a bearer token manually.

```bash
git clone git@github.com:dinglebear-ai/labby.git
cd labby
just install
just web-build
labby serve --host 127.0.0.1 --port 8765
```

`just install` builds the all-features release binary and symlinks it to
`~/.local/bin/labby`.

On macOS, install the gateway as a persistent per-user service instead of
running `labby serve` in a terminal:

```bash
just macos-service-install
just macos-service-status
```

This installs a `launchd` LaunchAgent that keeps Labby listening on
`127.0.0.1:8765`, restarts it after login or exit, and writes logs under
`~/.labby/`. Use `just macos-service-restart` after changing service settings
or `just macos-service-uninstall` to remove it. The cross-platform
`just service-install`, `just service-status`, `just service-restart`, and
`just service-uninstall` bindings select launchd on macOS and systemd on
Linux. This is suitable for a Tailscale Serve/Funnel route whose OAuth
callback targets the local gateway.

### First Run

For loopback development, `labby serve` can bootstrap a missing bearer token for
you. If `LABBY_MCP_HTTP_TOKEN` is absent and `LABBY_AUTH_MODE` is not `oauth`, it
generates a token, writes a minimal `~/.labby/.env`, reloads it into the running
process, prints the setup URL, and continues. The token itself is stored in
`~/.labby/.env` rather than printed.

Bootstrap writes these required `setup` keys if no env exists yet:

- `LABBY_MCP_HTTP_TOKEN` (generated random 64-character hex token)
- `LABBY_MCP_TRANSPORT=http`
- `LABBY_MCP_HTTP_HOST=127.0.0.1`
- `LABBY_MCP_HTTP_PORT=8765`
- `LABBY_AUTH_MODE=bearer`

It also enforces secure file creation via Labby's `env_merge` path (`0600` perms on
Unix) and then skips creating anything else until the web wizard runs.

For explicit setup:

```bash
mkdir -p ~/.labby
printf 'LABBY_AUTH_MODE=bearer\nLABBY_MCP_HTTP_TOKEN=%s\n' "$(openssl rand -hex 32)" > ~/.labby/.env
chmod 600 ~/.labby/.env
labby setup
labby serve --host 127.0.0.1 --port 8765
```

Open `http://127.0.0.1:8765/`.
Build static Labby assets with `just web-build` first when running from a source
checkout.

### Self-Host The Gateway

The recommended self-hosted gateway substrate is an amd64 Ubuntu 26.04 Incus
system container. Bare metal is the secondary supported shape for a dedicated
gateway host or VM. Docker is retained for explicit development/image smoke,
but it is not the recommended production boundary for Labby because stdio MCP
servers and agent CLIs are installed and launched at runtime.

```bash
scripts/incus-bootstrap.sh --version vX.Y.Z
incus exec labby -- systemctl status labby --no-pager
incus exec labby -- curl -fsS http://127.0.0.1:8765/ready
```

See [docs/runtime/INCUS.md](./docs/runtime/INCUS.md) for the full Incus
runbook, bare-metal variant, `/dev/net/tun` Tailscale passthrough, manual
`claude`/`codex`/`gemini` login checklist, and rollback commands.

## Core Workflows

### Start Labby

```bash
labby serve --host 127.0.0.1 --port 8765
labby mcp
```

`labby serve` starts the hosted HTTP runtime: `/v1` product APIs, `/mcp`
streamable HTTP MCP, auth routes, OAuth relay endpoints, and static Labby web
assets when an export is available. `labby mcp` is the stdio MCP entrypoint for
local MCP clients. A client configured to launch `labby mcp` does not need an
HTTP URL: when a `labby serve` daemon is reachable, the stdio process becomes a
transparent bridge to that daemon and uses its gateway configuration, upstream
connections, and OAuth state. If no daemon is found and no explicit target is
set, it starts a standalone local gateway instead. See the
[local bridge guide](./docs/surfaces/TRANSPORT.md#local-bridge-to-the-running-daemon)
for client configuration and `LABBY_SERVER_URL` fail-closed behavior.

### Manage Upstream MCP Gateways

```bash
labby gateway add \
  --name github \
  --url https://example.com/mcp \
  --bearer-token-env GITHUB_MCP_TOKEN \
  -y

labby gateway reload
labby gateway list
```

Stdio upstreams execute local commands when tested or reconciled, so gateway
tests and config mutations use the shared destructive-action confirmation gate.
The stdio spawn guard allows known runtimes such as `npx`, `uvx`, `docker`,
`node`, `python`, `python3`, `deno`, `pipx`, and `dnx`; customize it in
`[gateway]` inside `config.toml`.

### Use Code Mode

When `[code_mode].enabled = true`, Labby hides raw proxied upstream tools from MCP
`list_tools()` and exposes the canonical synthetic `codemode` tool.

```bash
labby gateway code status
labby gateway code enable
labby gateway code exec --code 'async () => tools.length'
```

MCP call shapes:

```json
{ "code": "async () => (await codemode.search(\"github issues\")).results" }
```

```json
{ "code": "async () => callTool(\"github::search_issues\", {\"query\":\"repo:dinglebear-ai/labby gateway\"})" }
```

```json
{ "code": "async () => codemode.run(\"gateway-summary\", {\"includeHealth\": true})" }
```

Code Mode can call exposed upstream MCP tools only. It cannot call Labby actions
from inside the sandbox.

### Work With Code Mode Snippets

```bash
labby snippets list
labby snippets get gateway-summary
labby snippets create --name my-snippet --file ./my-snippet.js
labby snippets validate my-snippet
labby snippets exec my-snippet
labby snippets test my-snippet
```

Snippets are stored per-user under `$LABBY_HOME` and executed through the
gateway Code Mode runner, so they can reach exposed upstream tools but not Labby
actions. The `snippets` service is gateway-gated: it is unavailable in builds
without the `gateway` feature.

### Audit Health And Logs

```bash
labby doctor            # audit every configured service
labby doctor system     # local env vars, Docker, disk, toolchain
labby doctor auth       # auth/OAuth env vars, files, permissions
labby doctor proxy      # zero-route stdio-proxy config/dependency preflight
labby doctor proxy --app-url URL --mcp-url URL --route /path
                        # routed public reverse-proxy checks remain available
labby doctor oauth-relay
labby health            # lightweight liveness/readiness probe
labby logs              # tail the active deployment's service journal
```

`labby doctor --json` is the CI-friendly form; the exit code reflects the worst
severity found.

> **Removed surfaces.** Earlier releases documented `labby marketplace`,
> `labby stash`, `labby nodes`, and `labby deploy`, along with ACP chat, the MCP
> Registry browser, and device/fleet runtimes. Those products have been deleted
> from source, manifests, packaging, and CI — not merely feature-gated.
> `scripts/check-retired-features.sh` guards against reintroduction, and the
> historical designs are archived under
> [docs/archive/retired-labby](./docs/archive/retired-labby/). Plugin
> marketplace assets now live in the separate
> [dendrite](https://github.com/dinglebear-ai/dendrite) repo.

### Drive The API

Generic action dispatch:

```bash
curl -s -X POST http://127.0.0.1:8765/v1/gateway \
  -H "Authorization: Bearer $LABBY_MCP_HTTP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"gateway.list","params":{}}'
```

Dedicated product routes also exist for catalog discovery (`/v1/{service}/actions`),
setup, doctor, snippets, filesystem, server logs, gateway OAuth
(`/v1/gateway/oauth/*`), OAuth relay, auth allowlists (`/v1/auth/allowed-emails`),
`/v1/openapi`, and the browser session routes under `/auth/*`. See
[generated API routes](./docs/generated/api-routes.md) and
[OpenAPI](./docs/generated/openapi.json).

## Runtime Surfaces

| Surface | Entry Point | Notes |
| --- | --- | --- |
| CLI | `labby <command>` | Current commands are generated in [docs/generated/cli-help.md](./docs/generated/cli-help.md). Use `--json` for machine-readable output and `--color auto|plain|color` for human output styling. |
| MCP stdio | `labby mcp` | Local editor/desktop MCP clients. |
| MCP HTTP | `labby serve` plus `/mcp` | Streamable HTTP MCP with bearer or OAuth JWT auth. |
| HTTP API | `labby serve` plus `/v1/*` | Generic `POST /v1/{service}` action dispatch plus dedicated product routes. |
| Web UI | `labby serve` plus exported assets | Main routes are `/` (overview), `/gateways`, `/gateway`, `/snippets`, `/usage`, `/settings` (with `core`, `services`, `services/[service]`, `surfaces`, `features`, `doctor`, `extract`, `advanced` subpages), `/docs`, `/design-system`, and `/mcp/code-mode`. |

MCP service tools use the shared action shape:

```json
{
  "action": "mcp.list",
  "params": { "search": "postgres", "limit": 10 }
}
```

Every service tool also supports `help` and `schema` through the shared
dispatcher. Destructive MCP actions use elicitation when the client supports it;
headless clients pass `"confirm": true` inside `params`.

## Configuration

Configuration is split deliberately:

| Data | Location | Examples |
| --- | --- | --- |
| Secrets and endpoint values | `~/.labby/.env` | `LABBY_MCP_HTTP_TOKEN`, `LABBY_GOOGLE_CLIENT_SECRET`, upstream bearer token env values |
| Preferences | `config.toml` | transport, CORS, auth mode, workspace root, gateway spawn guard, and upstream behavior |

`config.toml` is searched in this order:

1. `./config.toml`
2. `~/.labby/config.toml`
3. `~/.config/labby/config.toml`

Startup loads the first `config.toml`, initializes tracing, then loads
`~/.labby/.env` and a CWD `.env` if present. Runtime precedence is:

1. CLI flags
2. Environment variables
3. `config.toml`
4. Built-in defaults

Useful environment variables:

| Variable | Purpose |
| --- | --- |
| `LABBY_MCP_HTTP_TOKEN` | Static bearer token for protected admin/API/MCP routes. |
| `LABBY_AUTH_MODE` | `bearer` or `oauth`. |
| `LABBY_PUBLIC_URL` | Public base URL for OAuth metadata, issuer/audience, callbacks, and allowed-host derivation. |
| `LABBY_GOOGLE_CLIENT_ID` / `LABBY_GOOGLE_CLIENT_SECRET` | Google OAuth credentials for OAuth mode. |
| `LABBY_AUTH_ADMIN_EMAIL` | Bootstrap admin email; required in OAuth mode. |
| `LABBY_OAUTH_ENCRYPTION_KEY` | Base64 32-byte key required for encrypted upstream OAuth credentials. Rotation requires reauthorizing affected upstreams. |
| `LABBY_WEB_ASSETS_DIR` | Override static Labby export directory. |
| `LABBY_WEB_UI_AUTH_DISABLED` | Development-only browser auth bypass. |
| `LABBY_LOG` / `LABBY_LOG_FORMAT` / `LABBY_LOG_COLOR` | Tracing filter, text/json format, and non-TTY color policy. |
| `LABBY_LOG_DIR` | Optional rolling JSON file log directory. |
| `LABBY_ACTOR_KEY_SECRET` | Stable secret for redacted actor correlation in logs. |
| `LABBY_ADMIN_ENABLED` | Runtime opt-in for the `lab_admin` tool. |

Bearer auth is an operator/admin shortcut for Labby routes. Public protected MCP
routes validate route-scoped Labby OAuth JWTs; do not treat `LABBY_MCP_HTTP_TOKEN` as
a public resource credential.

When driving the web UI with automation while OAuth is enabled, pass the bearer
token as a same-origin header. `/auth/session` recognizes that token and returns a
synthetic admin session:

```bash
TOKEN=$(awk -F= '/^LABBY_MCP_HTTP_TOKEN=/{print $2}' ~/.labby/.env)
agent-browser open http://127.0.0.1:8765/gateways \
  --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
```

See [runtime configuration](./docs/runtime/CONFIG.md),
[environment variables](./docs/runtime/ENV.md), and
[OAuth](./docs/runtime/OAUTH.md).

## Current Catalogs

Do not maintain action, feature, env, or coverage inventories by hand in this
README. The generated artifacts are authoritative for the current branch:

| Artifact | Purpose |
| --- | --- |
| [service-catalog.md](./docs/generated/service-catalog.md) | Registered services, exposure, features, categories, and surfaces. |
| [action-catalog.md](./docs/generated/action-catalog.md) | Per-service actions and destructive metadata. |
| [env-reference.md](./docs/generated/env-reference.md) | Env vars generated from service metadata. |
| [api-routes.md](./docs/generated/api-routes.md) | Mounted HTTP routes. |
| [openapi.json](./docs/generated/openapi.json) | OpenAPI 3.1 schema. |
| [feature-matrix.md](./docs/generated/feature-matrix.md) | Cargo feature invariants. |
| [mcp-help.md](./docs/generated/mcp-help.md) | MCP help projection. |
| [cli-help.md](./docs/generated/cli-help.md) | Clap command help snapshot. |

Refresh and verify them with:

```bash
just docs-generate
just docs-check
```

`docs-check` verifies generated-artifact freshness and invariants. It is not a
Markdown link checker, live health check, or onboarding policy audit.

## Architecture

The workspace has 12 members and uses Rust 2024, resolver 3, a single
`[workspace.package]` version, shared `[workspace.dependencies]`, and shared
`[workspace.lints]` (`unsafe_code = "forbid"`, `mod_module_files = "deny"`,
`disallowed_macros = "deny"`). The MCP SDK is pinned exactly as
`rmcp = "=3.1.0"`.

| Path | Role |
| --- | --- |
| [crates/labby-primitives](./crates/labby-primitives) | Dependency-free leaf crate: `ActionSpec`/`ParamSpec`, `PluginMeta`/`EnvVar`/`Category`, `UiSchema`, static SSRF checks. |
| [crates/labby-apis](./crates/labby-apis) | Shared SDK contracts for core HTTP behavior, setup, and doctor. |
| [crates/labby-auth](./crates/labby-auth) | OAuth/JWT/session middleware, route support, and upstream OAuth runtime. |
| [crates/labby-runtime](./crates/labby-runtime) | Surface-neutral contracts and helpers: `ToolError`, gateway config DTOs, dispatch helpers, redaction, path safety, and security helpers. |
| [crates/labby-codemode](./crates/labby-codemode) | Client-neutral Code Mode runner kernel, broker, result shaping, snippets, and TypeScript descriptor generation. |
| [crates/labby-gateway](./crates/labby-gateway) | Gateway manager, upstream MCP proxy pool, Code Mode host adapter, discovery/imports, virtual servers, protected routes, and OAuth lifecycle. |
| [crates/labby-oauth-wire](./crates/labby-oauth-wire) | Host-neutral OAuth provider/client wire DTOs, extension media types, and endpoint transport policy. |
| [crates/labby-openapi](./crates/labby-openapi) | OpenAPI 3.1 schema assembly for the HTTP surface. |
| [crates/labby-web](./crates/labby-web) | Embedded/filesystem web asset serving with symlink escape defense. |
| [crates/labby](./crates/labby) | Product binary crate: CLI, MCP, HTTP API, config loading, gateway dispatch, logs, setup, snippets, filesystem access, and output rendering. |
| [crates/labby-winjob](./crates/labby-winjob) | Windows Job Object process-tree support, isolated so the main workspace can keep `unsafe_code = "forbid"`. |
| [crates/xtask](./crates/xtask) | Repo automation tasks; not published. |
| [apps/gateway-admin](./apps/gateway-admin/README.md) | Labby web UI, statically exported and served by `labby serve`. |
| [packages/labby-mcp](./packages/labby-mcp) | npm launcher wrapper behind `npx -y @dinglebear/labby mcp`. |
| [plugins](./plugins) | Claude/Codex plugin assets and skills. |
| [docs](./docs/README.md) | Topic documentation and generated inventories. |

Shared behavior belongs in the shared execution layer. Upstream/domain logic
belongs in `labby-apis`; reusable gateway/runtime/code-mode behavior belongs in
the extracted `labby-*` crates; product dispatch belongs in
`crates/labby/src/dispatch`; CLI, MCP, HTTP, and web adapters stay thin. See
[Architecture](./docs/ARCH.md) and [Dispatch](./docs/dev/DISPATCH.md).

## Development

Prefer the `just` aliases:

```bash
just check            # cargo check --workspace --all-features
just test             # cargo nextest run --workspace --all-features
just test-integration # cargo nextest run --workspace --all-features --run-ignored ignored-only
just lint             # skill drift + cargo wrapper smoke + clippy -D warnings + fmt check
just deny             # cargo deny check
just build            # cargo build --workspace --all-features
just build-release    # release build, bin/labby install, ~/.local/bin symlink
just service-install  # build and install the native persistent gateway service
just service-status   # inspect the native service manager state
labby setup host-service install --install-self -y # install current binary + start system service
labby setup host-service restart --install-self -y # reinstall current binary + restart service
labby setup host-service status --json # inspect the host Labby gateway service
just host-sync        # repo dev shortcut: rebuild + install binary + restart host service
just dev-container    # explicit Docker compatibility/prod-like smoke path
just dev-container-debug # explicit Docker debug binary path
just web-build        # cd apps/gateway-admin && pnpm build
just web-watch        # rebuild web assets when frontend files change
just run -- help      # cargo run --all-features -- <args>
just chat-local       # local Labby admin UI workflow with browser auth disabled
just dev-up           # start the explicit Docker compatibility stack
just dev              # alias for just dev-container
just dev-debug        # alias for just dev-container-debug
just install          # build-release + symlink ~/.local/bin/labby
just prod-run         # local prod-like image smoke on port 18765
just mcp-token        # rotate LABBY_MCP_HTTP_TOKEN in .env
```

Authoritative Rust verification is all-features:

```bash
cargo check --workspace --all-features
cargo clippy --workspace --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo build --workspace --all-features
```

CI uses the same posture and runs nextest with its CI profile. Use `cargo test`
only for narrow local slices or when a tool specifically requires it.

Frontend changes should also run the relevant `pnpm` scripts under
`apps/gateway-admin`, and `just web-build` when exported assets matter.

### Host Gateway Runtime

The recommended self-hosted gateway runtime is the Incus system container
provisioned by `scripts/incus-bootstrap.sh --version vX.Y.Z` and converged
in-box with `labby setup --provision`. Bare metal uses the same provisioner and
system unit when the host or VM is dedicated to Labby. The default service is
`/etc/systemd/system/labby.service`, running as `User=labby`, `Group=labby`, with
`ExecStart=/usr/local/bin/labby serve`. From a source checkout, `just host-sync`
remains the rebuild-and-restart developer shortcut. Docker remains available
for prod-like image smoke and adapter-container work, but it is no longer the
recommended agent gateway runtime.

### Dev Container

The Compose stack is a trusted local operator environment, not a hardened
generic deployment. It bind-mounts Labby state, the repository, and built web
assets; secrets are loaded from the mounted `/home/labby/.labby/.env`. The image
installs pinned Claude, Codex, and Gemini CLIs for stdio upstreams that invoke
provider tools. It does not install ACP adapters or mount ACP-specific state.

### Releases

Release Please maintains the version/changelog pull request and creates the
stable tag plus draft GitHub release when that pull request merges. Publishing
the release triggers the heavy GitHub-hosted workflows. They build Linux,
macOS, and Windows archives with checksums, build and scan the GHCR image, build and
smoke the Incus image, publish the npm launcher, and publish Labby's
`server.json` metadata to the official MCP Registry.

### Plugin Setup

The `plugins/labby` plugin ships skills, an MCP config, and `userConfig` — not a
`labby` binary, and no Claude Code hooks. The former
`plugins/labby/hooks/hooks.json` (SessionStart / ConfigChange shims) has been
removed; operators run `labby setup` themselves. Do not reintroduce a `hooks/`
directory, bundle a binary under `plugins/labby/bin/`, or add
Docker/systemd bootstrap logic to plugin assets.

`labby setup plugin-hook` remains a CLI command for on-demand audit and settings
sync (`--no-repair` for read-only), exercised by `just validate-plugin`.

## Related Servers

- [soma](https://github.com/dinglebear-ai/soma) - RMCP runtime for provider-backed MCP servers.
- [unifi-rmcp](https://github.com/dinglebear-ai/runifi) - UniFi controller REST API bridge.
- [tailscale-rmcp](https://github.com/dinglebear-ai/rtailscale) - Tailscale API bridge for devices, users, and tailnet operations.
- [unraid](https://github.com/dinglebear-ai/unraid) - Unraid monorepo; the Rust GraphQL bridge (`runraid`) lives in `unraid-rs/`.
- [apprise-rmcp](https://github.com/dinglebear-ai/rapprise) - Apprise notification fan-out bridge for many delivery backends.
- [gotify-rmcp](https://github.com/dinglebear-ai/rgotify) - Gotify push notification bridge for sends, messages, apps, and clients.
- [arcane-rmcp](https://github.com/dinglebear-ai/rarcane) - Arcane Docker management bridge for containers and related resources.
- [ytdl-rmcp](https://github.com/dinglebear-ai/rytdl) - Media download and metadata workflow server.
- [synapse-rmcp](https://github.com/dinglebear-ai/synapse) - Local Synapse workflow server for scout and flux actions.
- [cortex](https://github.com/dinglebear-ai/cortex) - Syslog and homelab log aggregation MCP server.
- [axon](https://github.com/dinglebear-ai/axon) - RAG, crawl, scrape, extract, and semantic search project.
- [lumen](https://github.com/dinglebear-ai/lumen) - Local semantic code search MCP server.

## Documentation

Start at [docs/README.md](./docs/README.md). High-value entrypoints:

- [Architecture](./docs/ARCH.md)
- [Configuration](./docs/runtime/CONFIG.md)
- [Environment](./docs/runtime/ENV.md)
- [OAuth](./docs/runtime/OAUTH.md)
- [Transport](./docs/surfaces/TRANSPORT.md)
- [Gateway](./docs/services/GATEWAY.md)
- [Setup](./docs/services/SETUP.md)
- [Server Logs](./docs/services/SERVER_LOGS.md)
- [Testing](./docs/dev/TESTING.md)
- [Operations](./docs/OPERATIONS.md)

## License

Original Dinglebear-authored portions of this project are licensed under [AGPL-3.0-only](LICENSE). Separate commercial licensing is available for organizations that need terms outside the AGPL. Third-party material remains under its original license. See [LICENSING.md](https://github.com/dinglebear-ai/labby/blob/main/LICENSING.md).
