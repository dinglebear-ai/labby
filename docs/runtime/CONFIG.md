---
title: "Runtime Configuration"
created: "2026-07-30"
updated: "2026-08-26"
---

# Runtime Configuration

Labby separates non-secret preferences from secrets and endpoint credentials.

## Files And Precedence

With an explicit `LABBY_HOME`, Labby reads exactly one TOML file:

1. `$LABBY_HOME/config.toml`

It does not inspect `./config.toml`, current-directory `.env`, or user-home
fallbacks. This makes the selected installation root authoritative for TOML,
dotenv credentials, and durable state. Without `LABBY_HOME`, the one canonical
root is `~/.labby`, including `~/.labby/config.toml`.

`LABBY_HOME` must be an absolute path. The access-control store is always
`$LABBY_HOME/access.db`; without an override it is `~/.labby/access.db`.
Relative or insecure state roots fail closed. Keep the database and its
`-wal`/`-shm` sidecars together under a service-owned directory; there is no
separate access-database path override.

Runtime precedence is:

1. CLI flags
2. Environment variables, including `~/.labby/.env`
3. `config.toml`
4. Built-in defaults

Existing process values win over dotenv files. With an explicit root, Labby
loads only `$LABBY_HOME/.env`; otherwise it loads only `~/.labby/.env`. Any
selected dotenv read or parse error fails visibly. Development commands that
need different state should set an absolute `LABBY_HOME` instead of relying on
the shell's current directory. Proxy CLI overrides are applied after the TOML
model loads.

Keep secrets, tokens, passwords, OAuth client secrets, and upstream credential
values in `$LABBY_HOME/.env` (normally `~/.labby/.env`). Gateway credential
mutations write beside the selected `config.toml`; they never independently
fall back to another home. Keep product preferences in TOML. The annotated
example in [../../config/config.example.toml](../../config/config.example.toml)
is the canonical hand-written configuration sample. Generated environment
metadata lives in [../generated/env-reference.md](../generated/env-reference.md).
The code-owned proxy key inventory lives in
[../generated/proxy-config-reference.md](../generated/proxy-config-reference.md).

Persisted TOML uses `config_version = 1`. Legacy files without the key migrate
to version 1 on their next supported mutation; a future/unsupported version
fails closed. Unknown keys inside Labby-owned sections are errors with the
offending field path. Unknown top-level scalar keys (for example `mcpp = 1` or
`config_verzion = 1`) are also errors. Foreign extensions must be explicitly
namespaced as TOML tables (for example `[vendor.example]`); those sections
remain accepted and are preserved by supported gateway/settings mutations.

Supported settings mutations create a mode-`0600` recovery copy beside
`config.toml` before replacing it. Labby retains at most 10 copies, 30 days,
and 64 MiB, while always preserving the newest recovery point. A successful
mutation can therefore return `maintenance_warning` when the new configuration
was durably committed but backup pruning or directory synchronization failed;
do not retry the mutation as though it were unapplied. Verify the active file,
run `labby doctor system --json`, preserve the newest
`config.toml.bak.*`, then correct directory permissions/capacity and remove only
older verified copies. Restore by stopping Labby, copying the selected backup
over `config.toml` with mode `0600`, and restarting before running doctor again.

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

Local Artifact persistence flushes file contents before atomic publication.
On Unix it also synchronizes the containing directories. Windows validates
those directories and rejects reparse points, but this path does not provide
the equivalent Unix directory-entry crash-durability guarantee. Transaction
journals and recovery checks remain active on both platforms.

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

With a Labby OAuth or configured static bearer identity, call `skill_library.list`
to obtain its current `library_version`. Then submit the immutable Depot Artifact
and `sha256:` revision through `POST /v1/skills`, including both required request
headers:

```sh
curl --request POST https://labby.example.invalid/v1/skills \
  --header "Authorization: Bearer $LABBY_CLIENT_TOKEN" \
  --header "X-Labby-Project-Id: $LABBY_PROJECT_ID" \
  --header "Content-Type: application/json" \
  --data @import-request.json
```

`LABBY_PROJECT_ID` must name a project that the inbound caller is authorized to
access. The bearer above is the inbound caller credential, not
`UNRAID_TEAM_DEPOT_TOKEN`; the latter remains server-held and is used only for
Labby-to-Depot acquisition. A project product credential is bound to its
configured protected MCP resource and therefore must invoke the same
`skill_library.import` tool through that protected MCP route, not through the
generic `/v1/skills` endpoint.

The ProductCredential route must expose the skills service through its bound
loadout:

```toml
[[loadouts]]
name = "team-skills"
upstreams = []
services = ["skills"]
expose_skills = true

[[protected_mcp_routes]]
name = "team-skills"
enabled = true
public_host = "labby.example.invalid"
public_path = "/mcp/team-skills"
scopes = ["lab"]

[protected_mcp_routes.target]
kind = "gateway_subset"
project_id = "replace-with-project-id"
loadout = "team-skills"
```

The ProductCredential resource and audience must exactly match this route's
public URL (`https://labby.example.invalid/mcp/team-skills`).
The `lab:read` scope is sufficient for listing and reading Skills, but imports,
activation, and other mutations require `lab` or `lab:admin`.

`import-request.json` contains:

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
