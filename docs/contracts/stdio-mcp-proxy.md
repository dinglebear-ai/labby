---
title: "Contract: Stdio MCP Proxy"
created: "2026-07-31"
updated: "2026-08-01"
---

# Contract: Stdio MCP Proxy

Status: implemented
Surfaces: CLI, Streamable HTTP, internal HTTP API
Related: [guide](../guides/STDIO_MCP_PROXY.md), [spec](../specs/stdio-mcp-proxy.md), [research](../reports/2026-07-31-stdio-mcp-proxy-research.md), [implementation plan](../superpowers/plans/2026-07-31-stdio-mcp-proxy-implementation.md), `docs/dev/ERRORS.md`, `docs/design/SERIALIZATION.md`

This contract pins the stable CLI grammar, configuration vocabulary, output shape, HTTP discovery behavior, auth challenges, and internal OAuth lease API for `labby proxy`.

## CLI grammar

```text
labby proxy [LABBY_OPTIONS] <PROGRAM_OR_SCRIPT> [CHILD_ARGUMENTS...]
```

Stable options:

| Option | Meaning |
|---|---|
| `--port <u16>` | Override the external port for this invocation. |
| `--auth <tailnet|bearer|oauth|none>` | Override the auth policy. |
| `--bearer-token <secret>` | One-run static token; implies bearer. |
| `--bearer-token-stdin` | Read one-run static token from stdin; implies bearer. |
| `--local` | Override exposure to a local loopback URL. |
| `--cwd <path>` | Child working directory. |
| `--env <NAME=VALUE>` | Explicit child environment entry; repeatable. |
| `--inherit-env <NAME>` | Inherit one ambient environment variable; repeatable. |

The global `--json` flag applies normally.

After the first program or script token, all remaining tokens are child arguments. An explicit `--` before the program is accepted. Unknown Labby-looking flags after the program are not rejected by Labby.

## Exit codes

- `0`: clean shutdown after a successful startup.
- `1`: runtime, auth, child, HTTP, or exposure failure.
- `2`: Clap usage or validation error.

## Configuration

Stable TOML keys:

```toml
[proxy]
exposure = "tailscale"        # tailscale | local
auth = "tailnet"              # tailnet | bearer | oauth | none
path = "/mcp"
port = "random"               # random or integer
port_range_start = 49152
port_range_end = 65535
bearer_token_env = "LABBY_PROXY_BEARER_TOKEN"
oauth_scopes = ["mcp:read", "mcp:write"]
inherit_env = []
shutdown_grace_ms = 3000
```

Unknown future keys are rejected by TOML deserialization only when the surrounding Labby config policy requires it; callers must not depend on unknown-key acceptance.

## Human startup output

The human output includes these labels when startup succeeds:

```text
MCP proxy ready

  Server   <resolved command display>
  URL      <public MCP URL>
  Exposure <Tailscale Serve|Local>
  Auth     <Tailnet|Bearer token|OAuth|None>

Press Ctrl+C to stop.
```

Whitespace, color, and symbols are not contractual. Labels and values are.

## JSON startup output

`--json` writes one object after readiness:

```jsonc
{
  "url": "https://node.example.ts.net:53147/mcp",
  "exposure": "tailscale",
  "auth": "oauth",
  "external_port": 53147,
  "local_addr": "127.0.0.1:38417",
  "command": ["node", "/path/to/dist.js"],
  "child_pid": 12345,
  "protocol_version": "2026-07-28"
}
```

The object never includes bearer tokens, authorization headers, JWTs, OAuth codes, or child environment values.

## Public MCP endpoint

The configured path accepts MCP Streamable HTTP traffic. The public URL includes the selected external port when it is not 443.

The endpoint preserves child MCP results and errors. Labby-generated failures use MCP JSON-RPC error data consistent with the existing bridge error mapping.

## OAuth metadata

For OAuth mode, the proxy serves:

```text
GET /.well-known/oauth-protected-resource
```

The response shape is:

```jsonc
{
  "resource": "https://node.example.ts.net:53147/mcp",
  "authorization_servers": ["https://labby.example.com"],
  "scopes_supported": ["mcp:read", "mcp:write"],
  "bearer_methods_supported": ["header"]
}
```

An unauthenticated request returns HTTP 401 with:

```text
WWW-Authenticate: Bearer resource_metadata="https://node.example.ts.net:53147/.well-known/oauth-protected-resource", scope="mcp:read mcp:write"
```

A token with insufficient scope returns HTTP 403 and an `insufficient_scope` challenge. Issuer and audience mismatches return 401.

## Bearer mode

Missing or invalid static bearer credentials return HTTP 401. Static token comparison is constant-time. The bearer challenge may advertise the MCP resource URL but does not expose the configured token source.

## Internal resource lease actions

Lease operations use the existing authenticated generic gateway route:

```http
POST /v1/gateway
```

The request envelope is `{ "action": "...", "params": { ... } }`. The route
and each action require `lab:admin`; there are no dedicated
`/v1/internal/proxy-resource-leases` routes.

### Create

`gateway.oauth.resource_lease.create`

Request:

```json
{
  "resource": "https://node.example.ts.net:53147/mcp",
  "scopes": ["mcp:read", "mcp:write"],
  "ttl_secs": 120,
  "owner": "bounded-redacted-owner-label"
}
```

Response: HTTP 200 with a `ResourceLease` result.

### Renew

`gateway.oauth.resource_lease.renew`

Request:

```json
{ "id": "opaque-lease-id", "ttl_secs": 120 }
```

Response: HTTP 200 with the renewed lease. Unknown or expired IDs return the
gateway's structured `invalid_param` error.

### Release

`gateway.oauth.resource_lease.release`

Request:

```json
{ "id": "opaque-lease-id" }
```

Response: HTTP 200 with `ResourceLeaseReleaseView`. Releasing an unknown or
expired ID returns `invalid_param`; the proxy guard itself avoids a second
release after a successful release.

### Lease document

```json
{
  "id": "opaque-lease-id",
  "resource": "https://node.example.ts.net:53147/mcp",
  "scopes": ["mcp:read", "mcp:write"],
  "expires_at_unix": 1785555600
}
```

Resource URLs must be absolute HTTPS URLs without credentials, query, or fragment. The current daemon lease action does not accept loopback HTTP resources, so `labby proxy --local --auth oauth` fails clearly until an explicit local-development lease policy is added; it never downgrades auth.

The proxy creates a 120-second lease, renews it every 40 seconds plus bounded
jitter, and releases it on normal shutdown. The daemon prunes expired leases
every 30 seconds, which is the forced-termination recovery path. The lease ID
must be treated as secret diagnostic material and never appears in readiness
output or logs.

## Tailscale ownership

Labby records the external port and exact loopback target it requested. Cleanup may issue an `off` command only if the current mapping still matches that ownership record. It must not call `tailscale serve reset`.

The exact owned command is
`tailscale serve --yes --https=<port> http://127.0.0.1:<local-port>`. Startup
and runtime status checks require that exact backend. Ctrl+C performs normal
cleanup; after an uncatchable crash, an operator may use exact-port `off` only
after verifying ownership in `tailscale serve status --json`.

## Setup and doctor

`labby setup proxy` owns comment-preserving preference writes and secret-safe
`.env` storage. On Unix the Labby home and secret file are mode `0700` and
`0600`. `labby doctor proxy` with no route parameters runs local proxy
preflight. Supplying app/MCP URLs and a route retains the routed public
reverse-proxy doctor contract.

Proxy startup and supervisor failures are CLI failures with contextual stderr;
the current CLI does not claim a complete stable proxy-specific JSON error-kind
vocabulary. Gateway lease calls retain the shared structured action error
contract, including `proxy_auth_unavailable` when issuer or lease support is
missing.

## Compatibility

- The public endpoint supports the modern stateless revision targeted by the pinned RMCP SDK.
- Legacy stdio children are adapted internally.
- The CLI may gain new options without breaking this contract.
- Removing or renaming the listed options, JSON fields, config keys, gateway
  lease actions, or auth modes is breaking.
