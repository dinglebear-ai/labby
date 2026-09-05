---
title: "Stdio MCP Proxy Guide"
created: "2026-08-01"
updated: "2026-08-01"
---

# Stdio MCP Proxy

`labby proxy` runs one stdio MCP server in the foreground and exposes that
server, unchanged, as a Streamable HTTP endpoint. It is a direct bridge, not the
aggregate Labby gateway: it does not add Labby tools, Code Mode, prefixes,
filters, or catalog normalization.

## Zero-flag quickstart

Run the one-time setup, check the host, then launch a JavaScript server:

```console
labby setup proxy
labby doctor proxy
labby proxy /path/to/dist.js
```

The built-in defaults are Tailscale Serve exposure, tailnet authentication,
`/mcp`, and a random port from 49152 through 65535. After setup, the normal run
needs no proxy flags. Startup prints the resolved child command, public URL,
exposure, and auth policy, then waits for Ctrl+C.

```text
MCP proxy ready

  Server   node /path/to/dist.js
  URL      https://node.example.ts.net:53147/mcp
  Exposure Tailscale Serve
  Auth     Tailnet

Press Ctrl+C to stop.
```

Use `--json` for one machine-readable readiness object. It contains `url`,
`exposure`, `auth`, `external_port`, `local_addr`, `command`, `child_pid`, and
`protocol_version`. It never contains a bearer token, OAuth token, lease ID,
authorization header, or child environment value.

## Command and launcher resolution

Labby parses its options only before the first child token. Everything after
that token belongs to the child, including values beginning with `-`:

```console
labby proxy /path/to/dist.js --workspace /srv/data --read-only
labby proxy --port 52177 /path/to/dist.js --child-flag
```

An explicit separator is accepted but is not normally required:

```console
labby proxy -- npx -y @modelcontextprotocol/server-filesystem /srv/data
```

The first child token resolves in this order:

1. An existing executable file runs directly.
2. An existing file with a valid shebang runs directly when executable; on
   Unix, a non-executable file may use a standards-compliant interpreter plus
   one optional shebang argument.
3. `.js`, `.mjs`, and `.cjs` files use `node` from `PATH`.
4. `.py` files use `python3` from `PATH`.
5. A bare command is resolved through `PATH`.
6. An unknown, non-executable file fails with an explicit-command suggestion.

Labby never invokes a shell. TypeScript has no inferred launcher; use a
shebang or an explicit command such as `labby proxy -- npx tsx server.ts`.
Arguments are retained as `OsString`, including non-UTF-8 Unix arguments.
`--cwd` changes the child working directory; otherwise the caller's current
directory is used.

## Exposure, authentication, ports, and output

| Setting | Behavior |
| --- | --- |
| `tailscale` exposure | Binds HTTP to loopback, publishes one HTTPS port with Tailscale Serve, and prints the tailnet URL. This is the default. |
| `local` exposure | Binds and prints a loopback HTTP URL only. Use `--local`; it never binds a LAN wildcard. |
| `tailnet` auth | Adds no application token. Reachability and grants are owned by Tailscale. Valid only with Tailscale exposure. This is the default. |
| `bearer` auth | Requires the separate proxy bearer token on every MCP request and SSE stream. |
| `oauth` auth | Validates Labby-issued JWTs for the exact proxy resource URL and configured scopes. Requires Tailscale exposure and a live OAuth daemon. |
| `none` auth | Adds no application authentication. Intended for explicit loopback use such as `--local --auth none`. |
| random port | Chooses an unused external Serve port from the configured range, with collision retries. |
| fixed port | Uses the numeric `proxy.port` or one-run `--port`; startup fails rather than replacing an existing mapping. |

One-run examples:

```console
labby proxy --port 52177 /path/to/dist.js
labby proxy --auth oauth /path/to/dist.js
printf '%s\n' "$TOKEN" | labby proxy --auth bearer --bearer-token-stdin /path/to/dist.js
labby proxy --local --auth none /path/to/dist.js
```

`--bearer-token` also implies bearer mode, but shell history and process
inspection can expose literal command-line secrets. Prefer setup-generated
storage, the environment, or `--bearer-token-stdin`.

There is no silent fallback. A Tailscale, bearer, OAuth, port, child, or
publication failure stops startup; a runtime failure begins owned cleanup.

## Configuration and precedence

The effective order is:

1. one-run CLI options;
2. existing process environment, then values loaded from exactly
   `$LABBY_HOME/.env` (normally `~/.labby/.env`) for unset names;
3. exactly `$LABBY_HOME/config.toml` when the absolute override is set,
   otherwise `~/.labby/config.toml`;
4. built-in defaults.

Only secret and executable/logging controls use environment variables. Proxy
preference keys do not have implicit one-to-one environment aliases. The
working directory does not participate in configuration discovery. See
[Runtime Configuration](../runtime/CONFIG.md) for the canonical path contract.

Complete `[proxy]` table:

```toml
[proxy]
exposure = "tailscale"
auth = "tailnet"
path = "/mcp"
port = "random"
port_range_start = 49152
port_range_end = 65535
bearer_token_env = "LABBY_PROXY_BEARER_TOKEN"
oauth_scopes = ["mcp:read", "mcp:write"]
inherit_env = []
shutdown_grace_ms = 3000
```

| Key | Accepted values and validation |
| --- | --- |
| `proxy.exposure` | `tailscale` or `local`; default `tailscale`. |
| `proxy.auth` | `tailnet`, `bearer`, `oauth`, or `none`; default `tailnet`. `local` plus `tailnet` is rejected. |
| `proxy.path` | Absolute, non-root path without query, fragment, `.` segments, or `..` segments; default `/mcp`. |
| `proxy.port` | `"random"` or a nonzero integer. It is the external HTTPS port for Tailscale publication. |
| `proxy.port_range_start` | First random candidate; default `49152`, minimum `1024`. |
| `proxy.port_range_end` | Last random candidate; default `65535`, and not below the start. |
| `proxy.bearer_token_env` | Valid environment-variable name containing the bearer secret; default `LABBY_PROXY_BEARER_TOKEN`. |
| `proxy.oauth_scopes` | Non-empty, whitespace-free scope tokens; default `mcp:read` and `mcp:write`. |
| `proxy.inherit_env` | Extra valid ambient variable names copied into the otherwise scrubbed child environment. |
| `proxy.shutdown_grace_ms` | Validated range `1..=60000`; default `3000`. |

Generated machine-readable and Markdown inventories are in
[`proxy-config-reference`](../generated/proxy-config-reference.md) and the
[`environment reference`](../generated/env-reference.md).

Proxy-relevant environment variables:

| Variable | Purpose |
| --- | --- |
| `LABBY_PROXY_BEARER_TOKEN` | Default secret source. If `bearer_token_env` names another key, that configured key is used instead. |
| `LABBY_TAILSCALE_BIN` | Overrides the `tailscale` executable used by publication and proxy preflight. |
| `LABBY_GW_UPSTREAM_STDERR` | Controls the redacted child-stderr forwarding level; default `debug`, and `off` discards it. |
| `LABBY_HOME` | Absolute durable-state root. Relocates config/secrets and fixes the access store at `$LABBY_HOME/access.db`; relative values fail closed. |
| `LABBY_MCP_HTTP_HOST`, `LABBY_MCP_HTTP_PORT` | First live-daemon candidate for OAuth lease actions. |
| `LABBY_MCP_HTTP_TOKEN` | Authenticates the proxy CLI to the daemon's admin gateway action route; it is not accepted by the proxied OAuth resource. |
| `LABBY_PUBLIC_URL`, `LABBY_MCP_GATEWAY_URL` | Public live-daemon fallback candidates and stable OAuth issuer configuration, as described in the OAuth guide. |
| `LABBY_PROXY_TEST_RENEW_MS` | Test-only renewal interval available with `proxy-testkit`; never use it as production configuration. |

`PATH` is consulted for launcher inference. Runtime-essential variables are
copied by the stdio spawn policy; other ambient variables require
`proxy.inherit_env`, `--inherit-env NAME`, or an explicit `--env NAME=VALUE`.
The latter two are child-process inputs, not persisted proxy preferences.

## Setup, secrets, and doctor

`labby setup proxy` is interactive on a terminal. For automation use `--yes`
and explicit flags; `--dry-run` previews without mutation. The setup action
preserves unrelated TOML keys, comments, and `.env` entries and is byte-stable
on a second identical run.

Bearer setup has two safe paths:

```console
# Generate a new 64-character random hex token when none exists.
labby setup proxy --yes --auth bearer

# Store a supplied token from stdin; the literal is never written to TOML.
printf '%s\n' "$TOKEN" | labby setup proxy --yes --bearer-token-stdin
```

The non-secret preferences go to `$LABBY_HOME/config.toml`; the bearer secret
goes to the environment key named by `proxy.bearer_token_env` in
`$LABBY_HOME/.env`. Existing secrets are reused unless stdin supplies a
replacement. On Unix, setup creates or repairs `$LABBY_HOME` to mode `0700`
and `.env` to mode `0600`. Output, debug formatting, and JSON report only that
a secret changed, never its value.

`labby doctor proxy` with no route arguments performs the zero-route local
preflight: proxy config validation, Node/Python launcher presence, selected
auth prerequisites, Tailscale version/connectivity/DNS/Serve capability, and
OAuth issuer/daemon/create-renew-release action checks where applicable.

The older routed reverse-proxy doctor remains available and unchanged:

```console
labby doctor proxy \
  --app-url https://lab.example.com \
  --mcp-url https://mcp.example.com \
  --route /telemetry
```

Supplying route arguments selects that public Labby/protected-route check; it
does not run the local stdio-proxy preflight.

## OAuth resource lifecycle

The OAuth authorization server has a stable issuer such as
`https://lab.example.com`. The ephemeral proxy is a separate protected
resource. Its identity is the exact public URL, including port and path:

```text
issuer:   https://lab.example.com
resource: https://node.example.ts.net:53147/mcp
```

Changing either `53147` or `/mcp` changes the JWT audience. Each random-port
run therefore creates a distinct resource URL. Use a fixed `proxy.port` for a
long-lived connector configuration; otherwise update the connector to the URL
printed by every run.

OAuth startup requires `LABBY_AUTH_MODE=oauth`, a stable
`LABBY_PUBLIC_URL` (or equivalent `[auth]`/`[public_urls]` configuration), the
same-host signing keys, and a reachable `labby serve` daemon. The CLI verifies
the daemon's authorization-server metadata and JWKS, checks the three lease
actions, then dispatches through authenticated `POST /v1/gateway`:

- `gateway.oauth.resource_lease.create`
- `gateway.oauth.resource_lease.renew`
- `gateway.oauth.resource_lease.release`

These are gateway actions, not dedicated `/v1/internal/*` routes. They require
`lab:admin`; lease IDs are opaque secrets and are redacted from diagnostics.

The default lease TTL is 120 seconds. The proxy renews every 40 seconds plus up
to four seconds of jitter. Renewal failure terminates the proxy. Normal
shutdown releases the lease. Forced termination cannot perform an async
release, so the daemon ignores the expired lease and prunes expired entries on
its 30-second sweep. Configured protected resources and ephemeral leases are
separate registries, so route refresh does not erase active proxy resources.

The proxy serves RFC 9728 metadata at the public origin root:

```text
GET https://node.example.ts.net:53147/.well-known/oauth-protected-resource
```

That document advertises the exact resource with port and `/mcp`. The
`WWW-Authenticate` challenge points to the same root metadata URL. This direct
proxy does not use the path-suffixed metadata URLs used by configured Gateway
protected routes. Local HTTP OAuth is intentionally rejected because daemon
leases accept HTTPS resources only; there is no bearer or no-auth downgrade.

## Tailscale Serve ownership and cleanup

Before publication Labby checks `tailscale version`, connected status, the
node DNS name, and `tailscale serve status --json`. It treats ports present in
either Serve TCP or web maps as occupied. A fixed-port collision fails. Random
mode retries collision-shaped claim failures, up to 32 candidates.

The owned command is exact:

```console
tailscale serve --yes --https=<external-port> http://127.0.0.1:<local-port>
```

Readiness requires the exact DNS-name, port, root handler, and loopback backend
to appear in Serve status. While running, Labby watches both the foreground
Serve child and that exact mapping. A disappeared or changed mapping is a
runtime failure.

On Ctrl+C or a supervised component failure, cleanup stops HTTP, terminates the
Serve child, waits for the mapping to disappear, reaps the stdio child, and
releases the OAuth lease. If the owned mapping remains, Labby re-reads status
and runs only:

```console
tailscale serve --yes --https=<external-port> off
```

It does that only while the mapping still points to its recorded loopback
backend. If ownership changed, cleanup refuses to remove it. Labby never calls
`tailscale serve reset` and never rewrites unrelated mappings.

After an uncatchable process crash or power loss, inspect status before manual
recovery. Remove only the printed port and only after confirming its backend is
the dead proxy's `127.0.0.1:<local-port>` target. Never use `serve reset` as a
proxy cleanup shortcut. The OAuth lease recovers independently through TTL
expiry; restart the proxy only after resolving any surviving exact-port
mapping.

## Security rationale

- The HTTP listener binds only to `127.0.0.1`; remote exposure belongs to
  Tailscale Serve. There is no LAN wildcard or public Funnel fallback.
- RMCP Host validation admits loopback authorities and, when applicable, only
  the exact public resource host plus port. Origin validation similarly admits
  loopback origins and the exact public origin. Unexpected Host or Origin
  values are rejected to resist DNS rebinding and browser cross-origin abuse.
- The child is launched as argv without a shell. Its environment starts with
  `env_clear`, then a small cross-platform runtime allowlist is restored,
  followed by explicitly inherited and explicit values. Ambient Labby/OAuth
  and upstream secrets are not inherited by default.
- Child stderr is continuously drained so a full pipe cannot deadlock the MCP
  server. Diagnostic tails pass through central stdio redaction before logs.
- Bearer tokens use constant-time comparison and are separate from
  `LABBY_MCP_HTTP_TOKEN`. OAuth disables static-admin-token fallback.
- Unix process groups and Windows Job Objects own descendants so normal and
  supervised shutdown reap package-runner trees, not only the immediate PID.

## Troubleshooting

`proxy command resolution failed`
: Install `node`/`python3`, add the executable to `PATH`, use a valid shebang,
  or provide the launcher explicitly after `--`. `.ts` is never guessed.

`tailnet auth requires Tailscale exposure`
: `--local` does not silently weaken `tailnet`. Select `--auth bearer` or
  `--auth none` explicitly for loopback use.

`Tailscale Serve port ... is already configured`
: Choose another fixed port or return to `port = "random"`. Inspect
  `tailscale serve status --json`; do not reset unrelated routes.

`Tailscale ... offline`, missing DNS name, or Serve capability failure
: Run `labby doctor proxy`, then repair Tailscale connectivity and HTTPS Serve
  support. Labby does not fall back to local exposure.

`bearer auth requires ...`
: Run `labby setup proxy --yes --auth bearer`, export the configured key, or
  pipe the secret to `--bearer-token-stdin`. Confirm a current-directory config
  is not changing `bearer_token_env`.

`proxy OAuth requires a stable Labby public issuer`
: Configure OAuth and `LABBY_PUBLIC_URL`, start `labby serve`, and verify the
  authorization-server metadata issuer exactly matches. A random proxy URL is
  the resource, never the issuer.

`live Labby daemon does not support proxy OAuth leases`
: The CLI reached an older daemon. Upgrade/restart the daemon and confirm all
  three lease actions appear in `GET /v1/gateway/actions`.

OAuth 401 for a token that works elsewhere
: Obtain a token whose audience is the exact printed URL, including external
  port and `/mcp`, and whose scopes cover `proxy.oauth_scopes`. A token for the
  same host on another port is intentionally rejected.

Host or Origin rejected
: Connect to the exact printed URL. Reverse proxies and browser clients must
  preserve its authority and origin; do not replace the port or MCP path.

Proxy exits after startup
: Inspect redacted logs for child closure, HTTP exit, Serve ownership drift, or
  OAuth renewal failure. Cleanup errors are attached to the primary failure
  rather than replacing it.

## Related documents

- [Stable contract](../contracts/stdio-mcp-proxy.md)
- [OAuth runtime](../runtime/OAUTH.md)
- [Transport security](../surfaces/TRANSPORT.md)
- [Generated CLI help](../generated/cli-help.md)
