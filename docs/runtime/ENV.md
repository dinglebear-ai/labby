# Environment Variables

This document lists the `lab` environment variables that matter for transport
and auth setup. The complete per-service env inventory is generated from
`PluginMeta` and lives in
[generated/env-reference.md](./generated/env-reference.md) and
[generated/env-reference.json](./generated/env-reference.json).

## HTTP Auth

Bearer mode:

```env
LABBY_AUTH_MODE=bearer
LABBY_MCP_HTTP_TOKEN=replace-me
```

OAuth mode:

```env
LABBY_AUTH_MODE=oauth
LABBY_PUBLIC_URL=https://lab.example.com
LABBY_GOOGLE_CLIENT_ID=google-client-id
LABBY_GOOGLE_CLIENT_SECRET=google-client-secret
LABBY_AUTH_ADMIN_EMAIL=admin@example.com
```

Optional auth overrides:

```env
LABBY_AUTH_SQLITE_PATH=/var/lib/labby/auth.db
LABBY_AUTH_KEY_PATH=/var/lib/labby/auth-jwt.pem
LABBY_AUTH_ALLOWED_REDIRECT_URIS=https://callback.example.com/callback/*
LABBY_GOOGLE_CALLBACK_PATH=/auth/google/callback
LABBY_GOOGLE_SCOPES=openid,email,profile
LABBY_AUTH_ACCESS_TOKEN_TTL_SECS=3600
LABBY_AUTH_REFRESH_TOKEN_TTL_SECS=2592000
LABBY_AUTH_CODE_TTL_SECS=300
```

These non-secret overrides can also live in `config.toml` under `[auth]`.

Rules:

- `LABBY_AUTH_MODE` defaults to `bearer`
- bearer mode keeps using `LABBY_MCP_HTTP_TOKEN`
- oauth mode requires `LABBY_PUBLIC_URL`, `LABBY_GOOGLE_CLIENT_ID`, `LABBY_GOOGLE_CLIENT_SECRET`, and `LABBY_AUTH_ADMIN_EMAIL`
- `LABBY_AUTH_ADMIN_EMAIL` is the bootstrap admin Google email; startup fails closed if unset under oauth mode so no Google account can authenticate without explicit permission. Future SQLite-backed allowlist (web-UI managed) will grant access to additional users.
- the old external issuer variables (`LABBY_OAUTH_ISSUER`, `LABBY_OAUTH_AUDIENCE`, `LABBY_OAUTH_CLIENT_ID`) are no longer used
- `LABBY_PUBLIC_URL` also feeds RFC 9728 metadata, JWT issuer/audience, and HTTP allowed-host derivation

## Remote Gateway CLI Usage

`labby gateway <subcommand>` (add/update/remove/reload/enable/disable/list/
mcp auth */protected-route */discover/import/code *) prefers the live
`labby serve` daemon's HTTP API over its own local `config.toml` mutation --
see `docs/services/GATEWAY.md` for why that split exists. To reach a daemon
running on a different host (not just the one the CLI happens to run on),
the invoking machine needs exactly two things in its `~/.labby/.env`:

```env
LABBY_MCP_HTTP_TOKEN=same-token-as-the-daemon
LABBY_PUBLIC_URL=https://labby.example.com
```

- `LABBY_MCP_HTTP_TOKEN` must be the *same* token the daemon itself uses for
  bearer auth (copy it from the daemon host's `~/.labby/.env`). Without it,
  the CLI still finds and reaches the daemon but every dispatch fails with
  `auth_failed`.
- `LABBY_PUBLIC_URL` (or `LABBY_MCP_GATEWAY_URL` if the gateway is split onto a
  separate hostname from the main app) is how the CLI locates the daemon
  when it isn't on the same host. Detection tries the local bind address
  first (`LABBY_MCP_HTTP_HOST`/`LABBY_MCP_HTTP_PORT`, then `config.toml`'s
  `[mcp]` section, then `127.0.0.1:8765`) and falls through to
  `LABBY_MCP_GATEWAY_URL` then `LABBY_PUBLIC_URL` in order, using whichever
  responds first to `/health`.
- If neither URL is reachable (or the token is missing/wrong), the CLI
  falls back to mutating its own local `config.toml` instead of erroring --
  which keeps bootstrap flows (`labby setup --provision`, the very first
  `gateway add` before `labby serve` exists) working, but means a config
  change made this way won't show up on a *different* running daemon until
  one of the above is fixed.

Verified against a completely bare `~/.labby/` containing nothing but the
two lines above (no `config.toml`, no local databases): both a read
(`gateway list`) and a mutation (`gateway add`) reached and used the live
remote daemon correctly, and the mutation did not write a local
`config.toml` at all. The local `GatewayManager` (and its `~/.labby/auth.db`)
is now built lazily and only comes into existence if remote detection
genuinely fails -- a successful remote dispatch touches no local files at
all.

## Remote MCP Stdio Usage

`labby serve --transport stdio` (a.k.a. `labby mcp`) applies the same
principle at the protocol level, not just for gateway-management actions:
before building anything local, it probes for a live daemon exactly like the
CLI does above. If one is reachable, the stdio process runs as a pure
bridge -- every `tools/`, `resources/`, and `prompts/` request coming in over
stdio is forwarded to the live daemon's own MCP endpoint and the response
piped straight back, with no local `GatewayManager`, upstream pool, or OAuth
state of its own. This is what keeps a locally spawned stdio MCP client
(e.g. an editor or agent configured to run `labby mcp` instead of connecting
to the daemon directly over HTTP) from becoming a second, silently-diverging
gateway instance. Same two env vars as above make this reachable from a
different host; same fallback (standalone, full local instance) applies if
no daemon is reachable.

## Service Environment Variables

Service credentials follow the standard pattern `{SERVICE}_URL`,
`{SERVICE}_API_KEY`, `{SERVICE}_TOKEN`, `{SERVICE}_USERNAME`, and
`{SERVICE}_PASSWORD`, with service-specific exceptions declared in
`PluginMeta`.

Named instances insert the label before the suffix, for example:

```env
JELLYFIN_NODE2_URL=http://node2.local:8096
JELLYFIN_NODE2_API_KEY=replace-me
OPENACP_NODE2_URL=http://node2.local:21420
OPENACP_NODE2_TOKEN=replace-me
```

Use [generated/env-reference.md](./generated/env-reference.md) for the current
required/optional env var matrix, default ports, secret flags, and examples.

## Provisioning Environment

`labby setup --provision` and `scripts/incus-bootstrap.sh` also honor:

```env
TS_AUTHKEY=tskey-auth-...
```

When set, provisioning installs Tailscale and joins the host/container to the
tailnet using `tailscale up --auth-key=file:/run/labby-ts-authkey`. The key is
written only to a root-owned runtime file for the join, then removed. Leave it
unset to skip Tailscale join.
