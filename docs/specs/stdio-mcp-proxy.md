---
title: "Spec: Stdio MCP Proxy"
created: "2026-07-31"
updated: "2026-07-31"
---

# Spec: Stdio MCP Proxy

Status: implementation
Owner: Labby runtime
Surfaces: CLI, Streamable HTTP, OAuth, Tailscale Serve
Related: [contract](../contracts/stdio-mcp-proxy.md), [research](../reports/2026-07-31-stdio-mcp-proxy-research.md), [implementation plan](../superpowers/plans/2026-07-31-stdio-mcp-proxy-implementation.md)

## Problem

Many MCP servers are distributed only as stdio programs. Publishing one for a remote tailnet client currently requires the operator to understand the child runtime, MCP transport translation, HTTP binding, authentication, Tailscale Serve, port selection, and cleanup.

Labby should make that operation one command:

```console
labby proxy /path/to/dist.js
```

The command launches the child, presents its own MCP surface faithfully over Streamable HTTP, applies configured authentication, publishes it through Tailscale Serve, and owns every resource until shutdown.

## Goals

- Zero required flags after defaults are configured.
- Reuse Labby's existing stdio process ownership, MCP bridge, HTTP, auth, config, output, and test infrastructure.
- Preserve the child server's names, capabilities, schemas, metadata, results, errors, tasks, MRTR, custom methods, notifications, and subscriptions.
- Bind only to loopback and publish only through the selected exposure controller.
- Support tailnet-only, static bearer, OAuth, and explicit no-auth policies.
- Select a random unused high external port by default.
- Cleanly remove only the mapping and processes created by this invocation.
- Provide deterministic unit, integration, conformance, fault-injection, and live verification.

## Non-goals

- Aggregate multiple MCP servers.
- Add Labby built-ins or Code Mode to the proxied catalog.
- Rename or filter the child's primitives.
- Persist running proxy definitions.
- Run detached proxies or provide list/stop commands in v1.
- Expose through Tailscale Funnel.
- Modify tailnet ACLs or grants.
- Deploy the child executable to another machine.

## User experience

### Normal invocation

```console
labby proxy /path/to/dist.js
```

### Explicit command

```console
labby proxy npx -y @modelcontextprotocol/server-filesystem /srv/data
```

### One-run overrides

```console
labby proxy --port 52177 /path/to/dist.js
labby proxy --auth bearer --bearer-token-stdin /path/to/dist.js
labby proxy --auth oauth /path/to/dist.js
labby proxy --local --auth none /path/to/dist.js
```

Labby options must appear before the first child token. All remaining tokens belong to the child. An explicit `--` remains accepted but is not required.

## Configuration model

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

A numeric `port` selects a fixed external port. Secret values never belong in TOML.

### Precedence

1. CLI override.
2. Process environment and `~/.labby/.env`.
3. `[proxy]` TOML.
4. Built-in defaults.

### Defaults

- Exposure: Tailscale.
- Authentication: tailnet.
- Path: `/mcp`.
- External port: random from 49152 through 65535.
- Internal listener: `127.0.0.1:0`.
- Foreground lifetime.

## Domain types

```rust
pub struct ProxyPreferences {
    pub exposure: ProxyExposure,
    pub auth: ProxyAuthMode,
    pub path: String,
    pub port: ProxyPortPreference,
    pub port_range_start: u16,
    pub port_range_end: u16,
    pub bearer_token_env: String,
    pub oauth_scopes: Vec<String>,
    pub inherit_env: Vec<String>,
    pub shutdown_grace_ms: u64,
}

pub enum ProxyExposure {
    Tailscale,
    Local,
}

pub enum ProxyAuthMode {
    Tailnet,
    Bearer,
    Oauth,
    None,
}

pub enum ProxyPortPreference {
    Random,
    Fixed(u16),
}

pub struct ProxyCommand {
    pub program: std::ffi::OsString,
    pub args: Vec<std::ffi::OsString>,
    pub cwd: std::path::PathBuf,
    pub display: String,
}

pub struct ProxyEndpoint {
    pub local_addr: std::net::SocketAddr,
    pub public_url: url::Url,
    pub external_port: u16,
}

pub struct ProxyRuntimeInfo {
    pub endpoint: ProxyEndpoint,
    pub child: ProxyChildInfo,
    pub exposure: ProxyExposure,
    pub auth: ProxyAuthMode,
    pub protocol_version: rmcp::model::ProtocolVersion,
}
```

### OAuth lease types

```rust
pub struct ResourceLease {
    pub id: uuid::Uuid,
    pub resource: String,
    pub scopes: Vec<String>,
    pub expires_at_unix: i64,
}

pub struct ResourceLeaseRequest {
    pub resource: String,
    pub scopes: Vec<String>,
    pub ttl_secs: u64,
}
```

Configured protected resources and ephemeral leases are separate collections. Replacing configured resources must never remove active leases.

## Command resolution

1. Existing executable file: execute directly.
2. Existing file with a valid shebang: execute through the shebang interpreter when direct execution is unavailable.
3. `.js`, `.mjs`, or `.cjs`: resolve `node` through PATH.
4. `.py`: resolve `python3` through PATH.
5. Bare token: resolve through PATH.
6. Unknown non-executable file: fail with suggested explicit commands.

The command is always passed as an argv vector to `tokio::process::Command`. It is never interpolated through a shell.

## Architecture

```text
remote MCP client
    -> HTTPS over tailnet
    -> Tailscale Serve foreground process
    -> 127.0.0.1 ephemeral HTTP listener
    -> proxy auth middleware
    -> RMCP StreamableHttpService
    -> generalized transparent BridgeServerHandler
    -> reusable direct stdio connector
    -> child MCP process
```

## Reuse boundaries

### Generalize existing MCP bridge

`crates/labby/src/mcp/bridge.rs` already forwards tools, prompts, resources, resource templates, completion, tasks, custom requests, custom notifications, subscriptions, and cancellation. It becomes the shared transparent bridge used by both:

- `labby mcp` when bridging to a live daemon;
- `labby proxy` when bridging to a stdio child.

The generalized bridge adds per-request metadata forwarding, progress correlation, request-ID cancellation mapping, and legacy interaction serialization.

### Expose existing stdio connector

`crates/labby-gateway/src/upstream/pool/connect_stdio.rs` remains the source of truth for:

- environment clearing and runtime allowlist;
- explicit environment injection;
- stderr draining;
- package-runner spawn locking and repair;
- modern discovery and legacy initialization fallback;
- Unix process groups;
- Windows Job Objects;
- descendant cleanup.

A narrow public direct-connection wrapper exposes a peer and owned shutdown without exposing pool internals.

### Reuse HTTP and auth

Extract loopback listener and RMCP service construction patterns from `cli/serve.rs`. Reuse `labby-auth::AuthLayer`, extending it with an explicit expected-audience override for ephemeral OAuth resources.

## MCP fidelity

### Lifecycle

The child connection uses RMCP `ClientLifecycleMode::Auto`, preferring `2026-07-28` and falling back to legacy initialize only on method-not-found.

The public endpoint advertises the modern stateless lifecycle. A legacy child is adapted behind that endpoint.

### Requests

For each downstream request the bridge preserves:

- method and typed or custom params;
- protocol version;
- client implementation;
- client capabilities and extensions;
- trace context and unknown metadata;
- MRTR input responses;
- task fields;
- result and error variants.

Proxy-owned request IDs and progress tokens are translated and never exposed upstream or downstream incorrectly.

### Progress and cancellation

The bridge records:

- downstream request ID to upstream request ID;
- upstream progress token to downstream progress token and peer.

HTTP SSE disconnect or explicit downstream cancellation sends an upstream `notifications/cancelled` with the translated request ID. Upstream progress is emitted only on the originating downstream request stream and uses the downstream token.

### Subscriptions

A downstream `subscriptions/listen` opens one upstream listen request. The bridge forwards the acknowledgment first, preserves the accepted filter, and rewrites subscription identity through RMCP's downstream subscription sink. Cancellation closes the upstream subscription.

Multiple subscriptions remain isolated.

### Legacy child interactions

When a legacy child issues sampling, elicitation, or roots requests, the bridge forwards them to the downstream peer associated with the currently serialized request. Requests that could create ambiguous association are serialized for legacy children only.

### Custom extensions

RMCP `CustomRequest`, `CustomNotification`, and `CustomResult` are forwarded without interpreting method names or payloads.

## Authentication

### Tailnet

No application bearer challenge is added. Reachability is controlled by Tailscale policy. The local listener remains loopback-only.

### Bearer

- Secret source: CLI stdin/literal override or the configured environment key.
- Constant-time comparison through `AuthLayer`.
- No token in logs, JSON output, errors, process titles generated by Labby, or evidence artifacts.
- Every MCP POST and SSE stream is protected.

### OAuth

- Stable issuer: configured Labby public URL.
- Protected resource and JWT audience: exact proxy URL including external port and MCP path.
- The live daemon creates a short-lived resource lease before publication.
- The proxy renews the lease while alive and releases it during normal shutdown.
- Expired leases are ignored and pruned.
- The proxy serves RFC 9728 Protected Resource Metadata and a matching `WWW-Authenticate` challenge.
- Metadata is served unauthenticated at the proxy origin root,
  `/.well-known/oauth-protected-resource`; the challenge points to that exact
  root document rather than a path beneath `/mcp`.
- Failure to create or renew a lease terminates OAuth startup or the running proxy; there is no downgrade.

## Tailscale exposure

The real controller executes:

```console
tailscale serve --yes --https=<port> http://127.0.0.1:<local-port>
```

It verifies the exact mapping in `tailscale serve status --json`, watches the child, and cleans up only its owned port. `tailscale serve reset` is forbidden.

## Supervision

Startup order:

1. Validate configuration and command.
2. Resolve auth prerequisites.
3. Spawn and discover the child.
4. Bind loopback HTTP.
5. Choose the external URL and create the OAuth lease when required.
6. Start the HTTP listener.
7. Publish with Tailscale Serve.
8. Verify unauthenticated and authenticated readiness.
9. Print the endpoint.

Shutdown is idempotent and runs on Ctrl+C, SIGTERM, child exit, HTTP failure, Tailscale exit, or OAuth renewal failure.

## Observability

Required fields include surface, service, action, phase, exposure, auth mode, external port, local port, child PID, and lifecycle mode. Tokens, authorization headers, JWTs, child secret environment values, and OAuth codes are forbidden.

## Verification

The feature is accepted only after:

- parser and resolver unit tests;
- config and auth tests;
- bridge metadata, progress, cancellation, MRTR, task, custom-extension, and subscription tests;
- process-tree tests on Linux and Windows;
- fake Tailscale collision and cleanup tests;
- live ignored Tailscale test;
- OAuth lease and exact-audience tests;
- MCP conformance through the proxy;
- all-features build, nextest, Clippy, formatting, docs, and deny gates;
- remote CI success on the PR.
