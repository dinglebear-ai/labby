# Operations

This document covers operator-facing workflows, verification surfaces, CI, and release behavior.

## Repo-Level Helpers

The repo includes helper tooling outside the shipped binary.

### `bin/health-check`

Purpose:

- smoke-test configured services from the repo env file
- validate reachability quickly
- provide operator-friendly shell output

It is distinct from the product-level `labby health` surface.

It is intended as a repo-local smoke test, not as the canonical SDK-level health API.

### `scripts/check-oauth.sh`

Purpose:

- verify OAuth/auth configuration against a **running server** from outside the process
- confirm all protected endpoints return 401 without auth and accept valid tokens
- validate OAuth discovery metadata, issuer, JWKS, and RFC 9728 WWW-Authenticate header
- confirm public endpoints (health, node self-registration, OAuth callbacks) are not auth-blocked

Usage:

```bash
./scripts/check-oauth.sh                          # auto-loads ~/.labby/.env, defaults to localhost:8080
./scripts/check-oauth.sh https://lab.example.com  # explicit URL
LABBY_BASE_URL=https://lab.example.com ./scripts/check-oauth.sh
```

Exit codes: `0` = pass, `1` = one or more failures. Suitable for post-deploy CI gates.

Complements `labby doctor`, which checks internal state (config, file permissions, SQLite) before a server is running. `scripts/check-oauth.sh` is the external black-box probe; `labby doctor` is the internal pre-flight check.

### `just mcp-token`

Purpose:

- generate or rotate `LABBY_MCP_HTTP_TOKEN`
- update the env file safely

## OAuth Auth State

When `LABBY_AUTH_MODE=oauth`, `lab` persists local auth state on disk:

- SQLite database: `~/.labby/auth.db` by default
- JWT signing key: `~/.labby/auth-jwt.pem` by default

Rules:

- `LABBY_AUTH_ADMIN_EMAIL` must be set to the bootstrap admin's Google email; startup fails closed if it is missing so no Google account can authenticate without explicit permission
- both files must use restrictive permissions; on Unix, `lab` requires they are not group- or world-readable
- new files are created with `0600` permissions on Unix
- the SQLite store is opened in WAL mode with a non-zero busy timeout
- the current auth store opens a small local SQLite pool, so login/code/token traffic is no longer funneled through one in-process mutex lane
- Google tokens stay server-side only; clients always receive `lab` access tokens and receive `lab` refresh tokens only when Google granted an upstream refresh token

Recovery guidance:

- deleting `auth-jwt.pem` invalidates every previously issued `lab` access token and refresh token exchange path tied to those access tokens
- deleting `auth.db` removes registered clients, pending authorization requests, authorization codes, and refresh tokens
- if you back up either file, back up both together to preserve a coherent auth state snapshot

## Browser-Local OAuth Callback Forwarding

Some MCP clients can pin the OAuth callback port but still redirect the browser to
`http://127.0.0.1:<port>/...`. When the real callback listener lives on another machine, run
`labby oauth relay-local` on the browser machine to accept that loopback redirect and forward it to
the actual listener.

Named-machine workflow:

```bash
labby oauth relay-local --machine node-a --port 38935
```

Ad hoc workflow:

```bash
labby oauth relay-local \
  --forward-base http://node.internal.example:38935/callback/node-a \
  --port 38935
```

Operational rules:

- the remote callback listener must already be running
- the helper is transport-only; it does not exchange codes or mint tokens
- the listener is loopback-only and normally run on demand for the active login flow
- startup output shows the resolved forwarding target before the first callback arrives
- failures map to HTTP responses on the local callback port: unreachable target -> `502`, timeout -> `504`

Recommended setup checklist:

1. Configure the browser-side machine target in `~/.labby/config.toml`:

```toml
[oauth.machines.node-a]
target_url = "http://node.internal.example:38935/callback/node-a"
description = "node-a Codex callback listener"
default_port = 38935
```

2. Start the real OAuth client listener on the remote machine.
3. Start `labby oauth relay-local` on the browser machine.
4. Complete the OAuth login flow in the browser before either listener exits.

Loopback redirects (`http://127.0.0.1`, `localhost`) and native-app private-use URI
scheme redirects (RFC 8252 §7.1, e.g. `com.raycast:/oauth`,
`warp://mcp/oauth2callback`) never need an allowlist entry — only an app the OS has
registered for that scheme can receive them, so DCR clients using them are
auto-allowed. When no explicit redirect allowlist is configured, the Labby
gateway product seeds common ChatGPT/Claude HTTPS callback patterns. Use
`LABBY_AUTH_ALLOWED_REDIRECT_URIS` or `[auth].allowed_client_redirect_uris` to
replace those defaults with a narrower or broader list. Use `https://*` only
when you intentionally trust any HTTPS DCR callback. Arbitrary non-loopback
`http://` callbacks remain blocked.

## Public OAuth Callback Relay

Use Labby's public callback relay when a remote, headless, or cross-namespace
MCP client needs a stable HTTPS callback:

```toml
mcp_oauth_callback_url = "https://callback.tootie.tv/callback/<machine>"
```

Regular desktop clients should keep local loopback callbacks. The public relay
does not exchange tokens or own PKCE; it forwards the final callback to the
machine target registered in `~/.labby/oauth-public-relay/registry.json`.

Operational commands:

```bash
labby oauth relay-registry list --json
labby oauth relay-registry import --file /tmp/callback-relay-registry.json --json
curl -fsS --max-time 5 https://callback.tootie.tv/healthz
```

For the full cutover and rollback runbook, see
[deploy/CALLBACK_RELAY.md](./deploy/CALLBACK_RELAY.md).

## Dev/Prod Container Drift

The dev and prod Docker stacks intentionally differ in several places. This section documents
the known drift points and the reasoning, so they are not silently "fixed" by accident.

### Upstream discovery concurrency

| Surface | Value | Why |
|---------|-------|-----|
| `docker-compose.yml` (dev) | `LABBY_UPSTREAM_DISCOVERY_CONCURRENCY=16` | Fast local warmup; developer wants all ~20 upstreams ready quickly |
| `docker-compose.prod.yml` (prod default) | `LABBY_UPSTREAM_DISCOVERY_CONCURRENCY=3` | Conservative rate-limit budget; a misconfigured upstream causes one timeout slot, not a 16× fan-out storm |

The 5× difference hides spawn-storm bugs in dev that only surface at scale. To test prod-like
behavior locally, use `just prod-run` (see below) — it starts the image with prod defaults.

### Binary source

| Surface | Binary origin |
|---------|---------------|
| Dev | `./bin/labby` bind-mounted from the host (`just build-release` output) — no image rebuild needed for Rust changes |
| Prod | Binary baked into the image at build time via `COPY bin/labby` |

### Frontend assets

| Surface | Assets source |
|---------|---------------|
| Dev | Bind-mounted from `apps/gateway-admin/out` on the host; `pnpm build` changes are reflected immediately |
| Prod | Assets baked into the image or served from the embedded binary's include_dir |

### Image

| Surface | Image tag |
|---------|-----------|
| Dev | `labby:dev` (local build, `Dockerfile.fast`) |
| Prod | `${LABBY_IMAGE:-ghcr.io/jmagar/lab:latest}` |

### Testing prod parity locally

Run `just prod-run` to start the prod image (or a locally built equivalent) with prod-like
env defaults. This validates that spawn-storm safeguards, discovery timeouts, and binary
embedding all behave the same as in production before a merge.

```bash
just build-release     # build fresh binary
just prod-run          # start prod-like container, prints health URL
```

The target runs detached, waits for `/health` to return 200, and prints the container ID.
Stop it with `docker stop lab-prod-test`.

## Product-Level Health Tooling

### `labby doctor`

`labby doctor` is the main read-only validation command.

It should audit:

- required env vars
- URL validity
- connectivity
- auth
- version visibility

It should support:

- all services
- single-service runs
- JSON output
- quick mode

Typical checks include:

- required env presence
- optional env visibility
- DNS/URL validity
- TCP reachability
- health endpoint success
- auth acceptance
- version reporting

### `labby health`

`labby health` should expose normalized health status using shared service contracts.

## Code Mode Operations

Use these checks when Code Mode search, execution, or inspector output drifts
from expected gateway behavior.

### Stale Runner Or MCP Session

Symptoms:

- `codemode.search()` shows old tools after an upstream config change.
- `callTool()` succeeds for a tool that search does not list, or the inspector
  shows old trace shapes.

Actions:

1. Run `labby gateway reload` to rebuild the active upstream runtime pool.
2. Reconnect the MCP client session so it receives the current gateway manager
   state and widget assets.
3. If the issue is CLI-only, rerun `labby gateway code exec`; CLI executions
   build a fresh host-side execution envelope per process.

### Runner Pool Overflow Or Timeout Storms

Symptoms:

- Code Mode calls queue behind long-running snippets.
- Logs show repeated `timeout`, pool overflow, or runner start failures.

Actions:

1. Split large snippets into smaller executions and reduce tool fan-out.
2. Inspect `[code_mode]` timeout and pool settings in `~/.labby/config.toml`.
3. Temporarily disable pooling only for diagnosis by restarting with the
   smallest configured pool size and watching whether failures become runner
   startup failures or snippet timeouts.
4. Restart the gateway service if pooled child processes are wedged.

### Semantic Search Degradation

Symptoms:

- Search still returns lexical/catalog results but semantic ranking disappears.
- Logs show `tei_unavailable`, `network_error`, or embedding decode failures.

Actions:

1. Check the configured TEI endpoint in `[code_mode.semantic_search]`.
2. Verify the TEI service health and response size; oversized or malformed
   responses are rejected and Code Mode fails open to lexical search.
3. Wait through the semantic-search cooldown, then run a small
   `codemode.search()` query to confirm recovery.

### Catalog Cache Reloads

Symptoms:

- CLI Code Mode cold-starts after upstream changes.
- One-shot executions miss newly enabled upstream tools.

Actions:

1. Run `labby gateway reload` or a targeted catalog refresh path.
2. Delete only the Code Mode catalog cache under the Lab home if the on-disk
   cache is suspected corrupt; do not delete auth or gateway config state.
3. Re-run a small `labby --json gateway code exec --code 'async () => 1'`
   smoke to repopulate the cache.

### Snippet Caveats

Built-in and user snippets merge their declared input before execution. A
missing snippet returns `snippet_not_found`; malformed input returns
`invalid_param` or `validation_failed`. Snippets do not bypass route scope,
schema validation, response caps, or destructive-tool permission checks.

### Rollback

To roll back Code Mode behavior quickly:

1. Disable Code Mode in config or route the affected MCP clients away from the
   gateway instance.
2. Restart the gateway service so runner pools and in-memory catalog state are
   dropped.
3. Re-enable only after `labby doctor`, `labby gateway list`, and a one-line
   `gateway code exec` smoke pass.

## Device Runtime Operations

In the current Linux `x86_64` v1 target, every supported fleet member runs `labby serve` as a node runtime.

Setup order:

1. Pick one machine as the master and start it first with `labby serve`.
2. If you use bearer auth, set `LABBY_MCP_HTTP_TOKEN` on the master before starting it and reuse that same token on every non-master device that reports to it.
3. On each non-master, set the master machine name in `~/.labby/config.toml`:

```toml
[node]
controller = "controller"
```

4. Start each non-master with `labby serve`.
5. Only use `labby mcp` when you explicitly want a local stdio MCP session instead of the default HTTP runtime.

Operationally:

- one device is the `master`
- non-controller nodes report to the master over `/v1/nodes/*`
- node inventory and node logs are queried from the master

Useful commands:

```bash
labby nodes list
labby nodes get node-a
labby logs search node-a oauth
```

Useful HTTP checks:

```bash
curl http://<device>:8765/health
curl -H "Authorization: Bearer $LABBY_MCP_HTTP_TOKEN" http://<controller>:8765/v1/nodes/devices
```

Current operational limits:

- fleet state is in-memory on the master
- non-master background uploads reuse the shared static bearer token when bearer auth is enabled
- non-controller nodes intentionally do not expose Web UI, gateway management, or MCP
- the master should be reachable on its configured HTTP port before non-masters start reporting to it

## Install and Patch Workflows

Install and uninstall operations should:

- validate env requirements
- prompt for missing values when appropriate
- patch `.mcp.json` atomically
- back up before write
- support dry-run behavior

## CI

CI should verify:

- workspace builds
- formatting
- linting
- deny checks
- CI-safe tests
- docs when rustdoc verification is enabled

Expected job split:

- fast correctness and style checks on pushes and PRs
- release builds on tags
- publishing after successful release builds

Live service integration tests are intentionally excluded from normal CI.

## Release Process

Locked release expectations:

- single workspace version
- tagged releases
- release artifacts per supported platform
- GitHub Releases as the artifact distribution surface
- `cargo-release` for version bumps and tagging
- GitHub-generated release notes

Tag format should stay `vX.Y.Z`.

## Privacy Rule

Operator workflows must respect the project-wide privacy rule:

- no telemetry
- no analytics
- no phone-home traffic except explicit service calls or explicit update operations
