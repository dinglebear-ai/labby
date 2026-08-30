---
title: "Operations"
created: "2026-07-30"
updated: "2026-08-18"
---

# Operations

This document covers operator-facing workflows, verification surfaces, CI, and release behavior.

## Repo-Level Helpers

The Justfile is the source of truth for repo-local operator/developer helpers. High-value current helpers include:

- `just mcp-token` — generate or rotate `LABBY_MCP_HTTP_TOKEN` and update the env file safely
- `just docs-check` — verify code-generated documentation remains fresh
- `just validate-plugin` — validate the checked-in Labby plugin setup lifecycle against a temporary `LABBY_HOME`
- `just host-sync` — rebuild/reinstall/restart the source checkout on the supported system-container host path

For health/auth verification, use the shipped `labby health` and `labby doctor ...` commands plus focused integration tests. The repository does not currently ship `bin/health-check` or `scripts/check-oauth.sh`; do not document them as supported operator interfaces.

## OAuth Auth State

When `LABBY_AUTH_MODE=oauth`, Labby persists local auth state on disk:

- SQLite database: `~/.labby/auth.db` by default
- JWT signing key: `~/.labby/auth-jwt.pem` by default
- Secret files use single-user permissions on every supported host: mode 0600
  on Unix and a protected DACL containing only a FullControl rule for the
  current user on Windows. This includes `.env`, drafts/backups, the auth
  database and WAL/SHM sidecars, and signing keys.

Rules:

- `LABBY_AUTH_ADMIN_EMAIL` must be set to the bootstrap admin's Google email; startup fails closed if it is missing so no Google account can authenticate without explicit permission
- both files must use restrictive permissions; on Unix, Labby requires they are not group- or world-readable
- new files are created with `0600` permissions on Unix
- the SQLite store is opened in WAL mode with a non-zero busy timeout
- the current auth store opens a small local SQLite pool, so login/code/token traffic is no longer funneled through one in-process mutex lane
- Google tokens stay server-side only; clients always receive Labby access tokens and receive Labby refresh tokens only when Google granted an upstream refresh token

Recovery guidance:

- deleting `auth-jwt.pem` invalidates every previously issued `labby` access token and refresh token exchange path tied to those access tokens
- deleting `auth.db` removes registered clients, pending authorization requests, authorization codes, and refresh tokens
- if you back up either file, back up both together to preserve a coherent auth state snapshot

Do not copy a live `auth.db` file by itself: WAL-mode writes may still reside in
`auth.db-wal`. Take a SQLite online backup, or stop Labby cleanly and copy the
database, signing key, and any access-control database as one encrypted,
access-controlled snapshot. Restore into an isolated `LABBY_HOME`, restore the
original owner and secret-file permissions, run SQLite integrity checks, and
exercise login, token validation, refresh, and revocation before returning the
instance to service. Treat a backup containing both the database and signing
key as credential material; define retention and destruction accordingly.
The snapshot must also preserve the provider-token encryption key configured by
`LABBY_TOKEN_ENCRYPTION_KEY` (or its secret-manager version), separately
encrypted under the backup recovery key. Without it, persisted Google provider
credentials are intentionally unrecoverable. CI runs
`scripts/ci/auth_backup_restore_drill.py` on every auth conformance change to
exercise SQLite's online backup API, `integrity_check`, isolated restore, row
recovery, and byte-identical signing/provider key recovery. Production
operations should run the same drill against a sanitized snapshot on a regular
schedule and alert on any failed integrity or restore assertion.

For subject containment, revoke that subject's browser/provider credentials and
dependent Labby grants, drain its initialized upstream peers, and verify the
count-only `session.invalidate` audit event. Signing-key removal is a global
last resort, not a substitute for subject-scoped revocation.

## Browser-Local OAuth Callback Forwarding

Some MCP clients can pin the OAuth callback port but still redirect the browser to
`http://127.0.0.1:<port>/...`. When the real callback listener lives on another machine, run
`labby oauth relay-local` on the browser machine to accept that loopback redirect and forward it to
the actual listener.

Codex has two distinct setup shapes:

- normal desktop clients usually need only the MCP server entry and
  `codex mcp login <server>`
- remote, SSH, WSL/browser-split, container, or headless clients need a reachable
  callback URL plus a fixed callback port, for example:

```toml
mcp_oauth_callback_port = 38935
mcp_oauth_callback_url = "https://callback.example.com/callback/<machine>"
```

On headless Linux, add `mcp_oauth_credentials_store = "file"` if the default
keyring-backed store stalls or cannot reach the user D-Bus session. This is a
client-side Codex config setting, not a Labby server setting.

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
mcp_oauth_callback_url = "https://callback.example.com/callback/<machine>"
```

Regular desktop clients should keep local loopback callbacks. The public relay
does not exchange tokens or own PKCE; it forwards the final callback to the
machine target registered in `~/.labby/oauth-public-relay/registry.json`.

Operational commands:

```bash
labby oauth relay-registry list --json
labby oauth relay-registry import --file /tmp/callback-relay-registry.json --json
curl -fsS --max-time 5 https://callback.example.com/healthz
```

For the full cutover and rollback runbook, see
[runtime/CALLBACK_RELAY.md](./runtime/CALLBACK_RELAY.md).

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
| Prod | `${LABBY_IMAGE:-ghcr.io/dinglebear-ai/labby:latest}` |

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
2. Delete only the Code Mode catalog cache under the Labby home if the on-disk
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
Rust coverage is a required pull-request and push gate for Rust changes. Its
aggregate floors complement, but do not replace, the MCP conformance job's
authoritative auth requirement matrix and focused auth contract tests.

## OAuth Conformance Rollout

Treat authentication changes as a staged security rollout. Before production,
run the complete MCP normative denominator, OpenAI authentication denominator,
repository auth tests, documentation checks, and the backup/restore drill. Then
deploy to trusted testers and prove the real ChatGPT callback/CIMD flow, root
`lab:read` discovery, execution step-up, refresh, revocation, and fail-closed
destructive behavior without elicitation. Expand the rollout only after those
wire behaviors and redacted observability signals are stable.

The MCP Inspector and Labby's Code Mode Inspector are useful staged diagnostics,
not conformance authorities. Use them to inspect metadata, catalog visibility,
scope challenges, and structured errors. Do not paste bearer tokens, OAuth
codes, client assertions, refresh tokens, or provider credentials into an
Inspector capture, issue, or report. The executable conformance matrices and
their pinned provenance remain the release evidence.

## OAuth Connector Incident Triage

Classify the failing layer before retrying:

1. Verify protected-resource and authorization-server metadata from the public
   edge and origin. A missing `/register` request can be an edge/WAF block; a
   metadata-advertised but unmounted route is an origin capability defect.
2. Separate proxy 5xx/timeouts from Labby authorization status codes. Check DNS,
   TLS, Google token, and JWKS phases rather than treating them as one timeout.
3. Treat Google `invalid_grant` as reauthorization-required. Verify the
   compare-and-delete invalidation counts and `session.invalidate` peer-drain
   counts before declaring containment complete.
4. Treat refresh-token replay as a security signal, not a transient retry.
   Stop automatic retry loops, preserve redacted correlation IDs, and inspect
   the subject/client/resource tuple.
5. After recovery, repeat metadata, authorization callback, token rotation,
   audience, insufficient-scope, and protected MCP request probes.

Alerting should group bounded rates and latency by `action`, `phase`, `kind`,
provider, and status without logging tokens, codes, email addresses, or raw
subjects. Alert on sustained provider/JWKS failures, refresh replay, incomplete
credential invalidation, and peer-drain failures.

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
