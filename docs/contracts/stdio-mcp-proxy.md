---
title: "Contract: Stdio MCP Proxy"
created: "2026-07-31"
updated: "2026-07-31"
---

# Contract: Stdio MCP Proxy

Status: implementation
Surfaces: CLI, Streamable HTTP, internal HTTP API
Related: [spec](../specs/stdio-mcp-proxy.md), `docs/dev/ERRORS.md`, `docs/design/SERIALIZATION.md`

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
GET /.well-known/oauth-protected-resource<mcp-path>
GET <mcp-path>/.well-known/oauth-protected-resource
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
WWW-Authenticate: Bearer resource_metadata="https://node.example.ts.net:53147/.well-known/oauth-protected-resource/mcp", scope="mcp:read mcp:write"
```

A token with insufficient scope returns HTTP 403 and an `insufficient_scope` challenge. Issuer and audience mismatches return 401.

## Bearer mode

Missing or invalid static bearer credentials return HTTP 401. Static token comparison is constant-time. The bearer challenge may advertise the MCP resource URL but does not expose the configured token source.

## Internal resource lease API

These routes are under existing authenticated `/v1/*` middleware and require `lab:admin`:

### Create

`POST /v1/internal/proxy-resource-leases`

Request:

```json
{
  "resource": "https://node.example.ts.net:53147/mcp",
  "scopes": ["mcp:read", "mcp:write"],
  "ttl_secs": 120
}
```

Response: HTTP 201 with a `ResourceLease` document.

### Renew

`PUT /v1/internal/proxy-resource-leases/<uuid>`

Request:

```json
{ "ttl_secs": 120 }
```

Response: HTTP 200 with the renewed lease. Unknown or expired lease returns 404.

### Release

`DELETE /v1/internal/proxy-resource-leases/<uuid>`

Response: HTTP 204. Releasing an unknown lease is idempotent and also returns 204.

### Lease document

```json
{
  "id": "uuid",
  "resource": "https://node.example.ts.net:53147/mcp",
  "scopes": ["mcp:read", "mcp:write"],
  "expires_at_unix": 1785555600
}
```

Resource URLs must be absolute HTTPS URLs without credentials, query, or fragment. Loopback HTTP resources are allowed only for explicitly local development mode and are never accepted by the public daemon route.

## Tailscale ownership

Labby records the external port and exact loopback target it requested. Cleanup may issue an `off` command only if the current mapping still matches that ownership record. It must not call `tailscale serve reset`.

## Error kinds

Stable proxy-specific error kinds used in JSON or API envelopes:

| Kind | Meaning |
|---|---|
| `proxy_invalid_config` | Proxy preference validation failed. |
| `proxy_command_not_found` | Program or inferred runtime could not be resolved. |
| `proxy_child_start_failed` | Child spawn or MCP lifecycle failed. |
| `proxy_auth_unavailable` | Selected auth policy cannot be established. |
| `proxy_oauth_lease_failed` | Lease create or renewal failed. |
| `proxy_tailscale_unavailable` | Tailscale is missing, disconnected, or unsupported. |
| `proxy_port_unavailable` | Fixed port is occupied or random attempts were exhausted. |
| `proxy_mapping_conflict` | Mapping changed and ownership-safe cleanup refused. |
| `proxy_runtime_failed` | A supervised component exited unexpectedly. |

Messages may improve; kinds are stable.

## Compatibility

- The public endpoint supports the modern stateless revision targeted by the pinned RMCP SDK.
- Legacy stdio children are adapted internally.
- The CLI may gain new options without breaking this contract.
- Removing or renaming the listed options, JSON fields, config keys, API routes, auth modes, or error kinds is breaking.
