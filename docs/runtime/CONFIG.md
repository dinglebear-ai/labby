---
title: "Runtime Configuration"
created: "2026-07-30"
updated: "2026-08-26"
---

# Runtime Configuration

Labby separates non-secret preferences from secrets and endpoint credentials.

## Files And Precedence

Configuration lookup stops at the first existing TOML file:

1. `./config.toml`
2. `$LABBY_HOME/config.toml`, when `LABBY_HOME` is set
3. `~/.labby/config.toml`
4. `~/.config/labby/config.toml`

`LABBY_HOME` must be an absolute path. When set, it replaces the two user-home
candidates rather than adding another fallback. The access-control store is
always `$LABBY_HOME/access.db`; without an override it is
`~/.labby/access.db`. Relative state roots fail closed. Keep the database and
its `-wal`/`-shm` sidecars together under a service-owned directory; there is
no separate access-database path override.

Runtime precedence is:

1. CLI flags
2. Environment variables, including `~/.labby/.env`
3. `config.toml`
4. Built-in defaults

Existing process values win over dotenv files. Labby then loads
`$LABBY_HOME/.env` (normally `~/.labby/.env`) and finally a current-directory
`.env` for names that remain unset. Proxy CLI overrides are applied after the
TOML model loads.

Keep secrets, tokens, passwords, OAuth client secrets, and upstream credential
values in `~/.labby/.env`. Keep product preferences in TOML. The annotated
example in [../../config/config.example.toml](../../config/config.example.toml)
is the canonical hand-written configuration sample. Generated environment
metadata lives in [../generated/env-reference.md](../generated/env-reference.md).
The code-owned proxy key inventory lives in
[../generated/proxy-config-reference.md](../generated/proxy-config-reference.md).

## Supported Sections

- `[output]`: CLI rendering defaults.
- `[log]` and `[local_logs]`: tracing and local server-log storage.
- `[mcp]`: default transport (`stdio`, `http`, or `unix_socket`), HTTP/TCP bind
  host/port, Unix-socket path/mode/ownership and optional Linux peer-credential
  allowlists, and allowed hosts.
- `[proxy]`: foreground direct stdio-proxy exposure, auth, endpoint path,
  external port selection, bearer secret key name, OAuth scopes, explicit
  child-environment inheritance, and shutdown preference.
- `[api]`: CORS preferences and the explicit trusted-forwarded-authority opt-in.
- `[web]`: exported asset location and development-only auth bypass.
- `[workspace]`: root for the optional filesystem browser. Default:
  `~/.labby/workspace`.
- `[gateway]`: stdio spawn guard and extra allowed commands.
- `[code_mode]`: sandbox execution and result-envelope limits.
- `[[openapi.specs]]`: allowlisted local Code Mode OpenAPI providers.
- `[oauth]`: callback relay targets.
- `[auth]`: bearer/OAuth mode and auth-store preferences.
- `[admin]`: runtime opt-in for `lab_admin`.
- `[setup]`: provisioning preferences.
- `[services]`: supported per-service preference overrides.
- `[[upstream]]`: proxied MCP upstreams.
- `[[protected_mcp_routes]]`: route-scoped OAuth resource servers.
- `[[virtual_servers]]`: virtual servers backed by registered Labby services.
- `[public_urls]`: canonical external URLs.
- `[[skill_library.sources]]`: server-owned exact Artifact acquisition
  connections used by durable Skill Library imports.

Top-level gateway timeouts, import mode, tombstones, pending imports, and
quarantined virtual servers are serialized alongside those sections.

## Durable Depot Skill Imports

`proxy_skills` is live MCP catalog federation; it does not install anything.
To make a Depot Skill survive a Depot outage or Labby restart, configure an
exact acquisition source and call `skill_library.import`, followed by the
separate `skill_library.activate` mutation.

```toml
[[skill_library.sources]]
id = "unraid-team-depot"
kind = "depot"
endpoint = "https://depot.example.invalid/artifacts/acquire"
pinned_addresses = ["203.0.113.10", "2001:db8::10"]
bearer_token_env = "UNRAID_TEAM_DEPOT_TOKEN"
```

The `id` must exactly match Depot's configured Artifact source identity. The
endpoint and IP addresses are operator configuration, not request parameters.
Resolve the deployed hostname immediately before configuration and include its
accepted public A/AAAA addresses; Labby pins the connection to those addresses
and rejects redirects, private peers, DNS rebinding, cross-origin component
URLs, digest mismatches, and oversized responses. Put the bearer value in
`$LABBY_HOME/.env`:

```sh
UNRAID_TEAM_DEPOT_TOKEN=replace-with-the-worker-machine-token
```

First call `skill_library.list` to obtain its current `library_version`. Then
submit the immutable Depot Artifact and `sha256:` revision through `POST
/v1/skills`:

```json
{
  "action": "skill_library.import",
  "params": {
    "source": {
      "kind": "depot",
      "connection_id": "unraid-team-depot",
      "artifact_id": "art_skill_team_example",
      "revision_id": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    },
    "expected_library_version": 0,
    "idempotency_key": "depot-import-art_skill_team_example-v1"
  }
}
```

The import receipt identifies the locally persisted revision but leaves it
inactive. Use the returned library version as `expected_library_version` in an
explicit `skill_library.activate` request. Reuse the same idempotency key when
reconciling an uncertain response; do not mint a new key until the prior result
is known.

### Trusted forwarded authority

Protected-route selection uses the request's `Host` header by default and
ignores `X-Forwarded-Host`. A deployment whose reverse proxy cannot preserve
the public `Host` may set:

```toml
[api]
trust_forwarded_headers = true
```

This setting makes `X-Forwarded-Host` authoritative for protected-route and
route-metadata selection; it does not change client-IP attribution or trust
`X-Forwarded-Proto`. Enable it only when direct access to Labby's listener is
blocked and every trusted proxy overwrites, rather than appends or preserves,
the inbound `X-Forwarded-Host` value. Otherwise a client can choose the virtual
protected resource by supplying that header. Prefer preserving the original
`Host` and leaving this setting at its secure default of `false`.

Gateway-subset protected routes may set `target.project_id` to bind the route
to one access-control Project. The value is opaque authorization context, not a
display name. CLI updates preserve an existing binding when `--project-id` is
omitted and remove it only when `--clear-project-id` is explicit.

## Gateway Upstreams

An upstream is HTTP, stdio, or a Unix-domain socket. HTTP credentials reference
environment variable names; secret values never belong in TOML. Stdio commands
pass through the spawn guard unless the operator explicitly extends or disables
it. A Unix-socket upstream requires `transport = "unix_socket"`, a `socket_path`
(absolute, or a Linux abstract `@name`), and an HTTP(S) `url` supplying the
request path and `Host` authority; a custom `Authorization` header is rejected so
credentials stay in `bearer_token_env` or `[upstream.oauth]`.

Use `labby gateway add`, `update`, `remove`, `reload`, and related
commands rather than editing active gateway state concurrently by hand.

### Upstream OAuth (authorization_code + PKCE)

OAuth upstreams use the encrypted credential store and the shared gateway
subject. The upstream remains an HTTP MCP endpoint; Labby's stdio mode is the
downstream transport to the MCP client.

```toml
[[upstream]]
name = "example"
transport = "http"
url = "https://mcp.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"

[upstream.oauth.registration]
strategy = "dynamic"
```

For `labby mcp`, configure `LABBY_OAUTH_ENCRYPTION_KEY` in `~/.labby/.env`.
The first request that needs the upstream opens the browser and completes the
provider callback on a listener bound only to `127.0.0.1`. Set
`LABBY_STDIO_OAUTH_CALLBACK_PORT` only when the provider requires a fixed
loopback port; `0` (the default) uses an ephemeral port. Do not put OAuth
tokens, authorization codes, or client secrets in TOML.

## Direct Stdio Proxy

`labby setup proxy` writes all ten non-secret `[proxy]` keys to
`$LABBY_HOME/config.toml`. Bearer material is stored separately in
`$LABBY_HOME/.env` under the configured `proxy.bearer_token_env` key. The
default key is `LABBY_PROXY_BEARER_TOKEN`; it is separate from the daemon
administrator token.

There are no implicit `LABBY_PROXY_EXPOSURE`, `LABBY_PROXY_AUTH`, path, port,
range, scopes, inheritance, or shutdown environment aliases. Those preferences
come from one-run CLI options where offered, then TOML, then defaults. Proxy
environment controls and the complete table are documented in the
[stdio MCP proxy guide](../guides/STDIO_MCP_PROXY.md).

## Authentication

`LABBY_AUTH_MODE` selects bearer or OAuth behavior. OAuth deployments also
require a canonical public URL, Google OIDC credentials, the bootstrap admin
identity, and the configured signing/encryption material described in
[OAUTH.md](./OAUTH.md) and the generated environment reference.

The web-auth bypass is development-only. Do not enable it on a publicly reachable
host or use it as a substitute for reverse-proxy authentication.

## Removed Configuration

Current Labby does not accept MCP Registry browser settings, ACP providers or
sessions, Marketplace sources, Fleet/node roles, Deploy-product policies, or
Stash workspace configuration. Historical schemas are preserved under
[../archive/retired-labby](../archive/retired-labby/) and must not be
reintroduced as compatibility aliases.
