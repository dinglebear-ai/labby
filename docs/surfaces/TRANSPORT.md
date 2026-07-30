---
title: "Transport Contract"
created: "2026-07-30"
updated: "2026-07-30"
---

# Transport Contract

Labby supports three MCP transports over one execution layer: stdio, streamable
HTTP over TCP, and streamable HTTP over a Unix-domain socket. Transport choice
does not change the catalog, schemas, envelopes, or destructive-op policy.

## Stdio

Run:

```bash
labby mcp
```

Stdio is intended for local editor and desktop clients. Protocol messages use
stdin/stdout; logs must never be written to stdout.

## Streamable HTTP

Run:

```bash
labby serve --host 127.0.0.1 --port 8765
```

The native MCP endpoint is `/mcp`. The hosted runtime also mounts supported
`/v1/*` APIs, auth routes, health routes, protected MCP routes, and static web
assets when available.

The generated route inventory in
[../generated/api-routes.md](../generated/api-routes.md) is authoritative.

## Streamable HTTP Over A Unix-Domain Socket

`transport = "unix_socket"` serves the same router and middleware stack as
HTTP/TCP. It is a same-host (or shared-namespace) transport, not a cross-node
one.

```toml
[mcp]
transport = "unix_socket"
socket_path = "/run/labby/labby.sock"
socket_mode = "0660"
peer_uid = 1000
```

- Filesystem listeners require an absolute path whose existing directory chain
  contains only real directories owned by root or the process effective UID.
  Non-sticky group/world-writable ancestors are rejected.
- Labby binds inside a private `0700` staging directory, applies the configured
  mode and ownership there, then atomically publishes the hardened socket.
  Startup reclaims only a verified stale socket; shutdown removes only the exact
  inode this process created.
- Linux abstract sockets use `@name` and have no filesystem owner or mode, so
  `socket_mode`, `socket_uid`, and `socket_gid` must be omitted.
- Bearer and OAuth continue to work over the socket. Linux peer-credential
  authorization (`peer_uid` / `peer_gid`) is an alternative that cannot be
  combined with bearer or OAuth, and is interpreted in the listener's user
  namespace. A Unix listener with none of the three is refused at startup.

## Authentication

- Operator/admin routes use the configured bearer or OAuth mode.
- The downstream MCP endpoint implements only the stateless `2026-07-28`
  lifecycle. The gateway-to-upstream boundary attempts `server/discover` first
  and performs one bounded fallback to legacy `initialize` for recognized
  lifecycle-compatibility failures; HTTP/TCP and Unix-socket upstreams share
  that policy.
- Protected MCP routes validate route-specific OAuth resources and scopes.
- OAuth metadata and callback routes are public by protocol design.
- Browser session cookies are separate from MCP authorization headers.

## Reverse Proxy

A reverse proxy must preserve the request host/scheme information used to build
OAuth metadata and callback URLs, pass authorization headers, support streaming
responses, and avoid buffering Streamable HTTP traffic. See
[../runtime/REVERSE_PROXY.md](../runtime/REVERSE_PROXY.md).

## Host Validation

State-changing same-origin web/API requests are subject to host validation and
CSRF protections. Do not make a route public merely to simplify browser calls.

## Unsupported Legacy Shapes

The hosted runtime does not expose Fleet/node WebSockets, node enrollment APIs,
Marketplace preview routes, ACP session endpoints, Stash APIs, or MCP Registry
compatibility endpoints. Their historical transport contracts are archived under
[../references/retired-labby](../references/retired-labby/).
