# Runtime Configuration

Labby separates non-secret preferences from secrets and endpoint credentials.

## Files And Precedence

Configuration lookup stops at the first existing TOML file:

1. `./config.toml`
2. `~/.labby/config.toml`
3. `~/.config/labby/config.toml`

Runtime precedence is:

1. CLI flags
2. Environment variables, including `~/.labby/.env`
3. `config.toml`
4. Built-in defaults

Keep secrets, tokens, passwords, OAuth client secrets, and upstream credential
values in `~/.labby/.env`. Keep product preferences in TOML. The annotated
example in [../../config/config.example.toml](../../config/config.example.toml)
is the canonical hand-written configuration sample. Generated environment
metadata lives in [../generated/env-reference.md](../generated/env-reference.md).

## Supported Sections

- `[output]`: CLI rendering defaults.
- `[log]` and `[local_logs]`: tracing and local server-log storage.
- `[mcp]`: default transport (`stdio`, `http`, or `unix_socket`), HTTP/TCP bind
  host/port, Unix-socket path/mode/ownership and optional Linux peer-credential
  allowlists, and allowed hosts.
- `[api]`: CORS preferences.
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

Top-level gateway timeouts, import mode, tombstones, pending imports, and
quarantined virtual servers are serialized alongside those sections.

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
[../references/retired-labby](../references/retired-labby/) and must not be
reintroduced as compatibility aliases.
