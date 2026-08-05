# Stdio MCP Proxy Research Report

- **Date:** 2026-07-31
- **Implementation documentation:** 2026-08-01
- **Labby base:** `eff39c79d97c907f6c9956f4711ecab5cd8df62f`
- **Target command:** `labby proxy <stdio-server-command>`
- **Primary UX:** `labby proxy /path/to/dist.js`
- **Related:** [operator guide](../guides/STDIO_MCP_PROXY.md),
  [spec](../specs/stdio-mcp-proxy.md),
  [contract](../contracts/stdio-mcp-proxy.md), and
  [implementation plan](../superpowers/plans/2026-07-31-stdio-mcp-proxy-implementation.md)

## Executive finding

The feature is feasible and fits Labby's product boundary, but it is not raw port forwarding. It is a supervised MCP transport adapter with four independent responsibilities:

1. Launch and own a stdio MCP child process.
2. Preserve MCP requests, notifications, results, extensions, cancellation, progress, MRTR, tasks, and subscriptions while translating stdio to Streamable HTTP.
3. Protect the HTTP resource with tailnet policy, a static bearer token, or OAuth.
4. Publish the loopback HTTP listener through Tailscale Serve without clobbering existing Serve configuration.

The normal user flow can remain one command after setup:

```console
labby proxy /path/to/dist.js
```

The implementation must be a dedicated direct-proxy runtime. Reusing the aggregate gateway path would alter names, catalogs, capability reporting, resource URIs, result metadata, and extension behavior, which violates the meaning of proxying one specific server.

## Research anchors

### Labby

- Source tree: current `origin/main` at `eff39c79d97c907f6c9956f4711ecab5cd8df62f`.
- Workspace RMCP pin: `=3.0.0-beta.2` at the research base.
- Active, separate RMCP upgrade worktree: branch `chore/pin-rmcp-3.1.0-20260731`. It already changes the workspace pin and lockfile but was uncommitted when inspected.
- Active, separate MCP capability worktree: branch `audit/mcp-2026-07-28-capabilities`. It contains uncommitted aggregate-gateway work and a capability audit. This proxy plan does not modify or depend on those uncommitted files.

### RMCP

- Release: `rmcp-v3.1.0`.
- Source commit inspected: `1f9358eddca42d3a510c70ae6446dd6548c7c856`.
- Release date: 2026-07-31.
- Relevant changes:
  - authorization-required error classification;
  - strict stateless request metadata validation;
  - receive-side request-association enforcement;
  - MRTR input-required result decoding fixes;
  - modern HTTP metadata enforcement;
  - protocol negotiation fixes.

### MCP specification

- Source tag inspected: `2026-07-28-RC`.
- Source commit inspected: `9d700ed62dcf86cb77475c9b81930611a9182f46`.
- Status on 2026-07-31: release candidate. The latest stable dated specification remains 2025-11-25.
- The implementation should deliberately target the pinned 2026-07-28 RC source while retaining compatibility negotiation for older servers. The live draft has already evolved beyond the tag, so implementation and conformance must never rely on an unpinned moving page without an explicit drift review.

### Tailscale

- Live devhost binary inspected: Tailscale 1.98.10.
- The node was connected and already had unrelated Serve mappings.
- No existing Serve mapping was changed during research.
- A temporary foreground-cleanup experiment did not return a usable tool result envelope. A post-check confirmed that the temporary port was absent and the existing mappings remained. Exact signal and fallback cleanup behavior therefore remains an explicit implementation spike and release gate, not an assumed contract.

## Protocol findings

### 1. The 2026 lifecycle is stateless, including stdio

The 2026-07-28 RC states that an open connection or stdio process is not a session. Each request carries protocol version, client identity, and client capabilities in `_meta`. Unrelated requests may be interleaved on the same stdio process.

Consequences for the proxy:

- It must forward each downstream request's metadata, not reuse one synthetic startup identity.
- It must not infer a client session from the HTTP connection, bearer token, Tailscale identity, or child process.
- Persistent task or subscription state must be represented by explicit protocol identifiers.
- Request IDs, progress tokens, and subscription IDs must be correlated independently.

### 2. Streamable HTTP is one POST per JSON-RPC message

For 2026-07-28:

- one MCP endpoint accepts POST;
- every request, notification, or input response is its own POST;
- a request returns JSON or a request-scoped SSE response;
- progress and request-scoped notifications belong only on the originating response stream;
- `subscriptions/listen` owns its own long-lived SSE response;
- closing an SSE response cancels that request;
- protocol-level HTTP sessions and the standalone GET stream are removed;
- Origin validation and loopback binding are normative security requirements.

The local adapter must bind to `127.0.0.1` and must keep RMCP's Host, Origin, metadata-header, and response-stream enforcement enabled.

### 3. Modern server-to-client interactions use MRTR

Under 2026-07-28 Streamable HTTP, sampling, elicitation, and roots are not independent JSON-RPC requests on an SSE stream. They are returned in `InputRequiredResult`, and the client retries with input responses.

Consequences:

- For a modern 2026 stdio child, the proxy can preserve MRTR results directly.
- The direct runtime must not auto-consume or rewrite incomplete MRTR results.
- Legacy initialized children still require a client handler for server-to-client requests. Those requests are multiplexed over stdio without a transport stream identifier, so legacy forwarding needs a correctness gate that prevents ambiguous concurrent association.

### 4. Subscriptions require explicit ID translation

`subscriptions/listen`:

- is a long-lived request;
- must send `notifications/subscriptions/acknowledged` first;
- may acknowledge only the supported subset of the requested filter;
- tags every delivered notification with `io.modelcontextprotocol/subscriptionId`;
- permits multiple concurrent subscriptions;
- is cancelled by closing HTTP SSE or sending `notifications/cancelled` on stdio.

A proxy cannot treat subscriptions as ordinary request-response forwarding. It needs a dedicated bridge that maps downstream listen IDs to upstream listen IDs and rewrites notification metadata.

### 5. Custom extensions can be preserved by RMCP 3.1

RMCP 3.1's `ClientRequest`, `ServerRequest`, and notification unions include `CustomRequest` and `CustomNotification` variants. The raw method and params are preserved. `ServerResult` includes `CustomResult`.

This makes a typed low-level `Service<RoleServer>` bridge viable without dropping unknown extension methods. It is still necessary to add tests that prove unknown request, notification, metadata, and result fields round-trip unchanged.

## RMCP API findings

### Low-level service boundary

`rmcp::service::Service<RoleServer>` receives a complete `ClientRequest`, `RequestContext<RoleServer>`, complete client notifications, and returns `ServerResult`. This is a better fit than implementing each aggregate gateway handler separately.

The request context exposes:

- a cancellation token;
- the downstream request ID;
- the request metadata;
- transport extensions;
- the downstream peer needed to deliver legacy server-to-client requests or notifications.

### Upstream request metadata

`PeerRequestOptions::with_meta` lets the caller provide request metadata. RMCP first applies connection defaults, then extends them with explicit metadata, so the direct bridge can override synthetic startup values with the actual downstream request metadata.

The proxy must still replace collision-prone fields before forwarding:

- progress token;
- any proxy-owned request-correlation extension;
- subscription ID where applicable.

All other metadata and unknown extension keys should be preserved.

### Lifecycle negotiation

`ClientLifecycleMode::Auto`:

1. probes with `server/discover`;
2. falls back to legacy initialize only on method-not-found;
3. treats unsupported protocol version as modern negotiation, not a legacy signal.

This matches the RC's stdio compatibility rules and should replace Labby's current manual reconnect logic in the direct proxy runtime.

### Subscription client API

RMCP's client peer exposes `listen`, which:

- opens an upstream `subscriptions/listen` request;
- validates the first acknowledgment;
- creates a notification channel scoped to the upstream request ID;
- cancels the request when the handle is dropped.

The proxy should use this API for a modern child, then rewrite the upstream subscription ID to the downstream ID before delivering each notification.

## Labby architecture findings

### CLI

- `crates/labby/src/cli.rs` has no `proxy` command.
- There is no existing Clap trailing-command precedent.
- The parser contract requires dedicated tests for:
  - a script path with no flags;
  - child arguments beginning with `-`;
  - Labby options before the child target;
  - an explicit `--` separator;
  - non-UTF-8 arguments on Unix.

### Configuration

- `LabConfig` has no proxy section.
- Existing precedence is CLI/process environment, `~/.labby/.env`, `config.toml`, built-in default.
- Atomic, comment-preserving config mutation already exists and should be reused.
- Secrets belong in `~/.labby/.env`; non-secret proxy preferences belong in `[proxy]`.

### Existing HTTP runtime

`crates/labby/src/cli/serve.rs` already provides useful pieces:

- RMCP Streamable HTTP service construction;
- loopback listener and Axum serving;
- Host allowlisting;
- Origin enforcement through RMCP;
- graceful shutdown helpers;
- bearer and OAuth bootstrapping.

The file is large and tightly coupled to the complete Labby app router. The proxy should extract reusable HTTP-listener and shutdown primitives rather than copy the full `serve` path or start the aggregate gateway.

The proxy must not force JSON-only responses. It needs SSE for progress and `subscriptions/listen`.

### Existing stdio connector

`crates/labby-gateway/src/upstream/pool/connect_stdio.rs` already implements the difficult process-safety pieces:

- direct process execution without a shell;
- cleared ambient environment plus a runtime allowlist;
- explicit environment injection;
- continuous stderr draining;
- Unix process groups;
- Windows Job Objects;
- descendant cleanup;
- modern discovery with legacy compatibility.

The proxy should extract and reuse these pieces. A command typed directly by the local operator should use an `ExplicitLocalCli` spawn authority that bypasses the persisted-config executable allowlist while retaining every other safeguard.

### Existing relay is insufficient

`crates/labby-gateway/src/upstream/pool/relay.rs` explicitly relays interactive behavior for `call_tool` only. Prompt and resource calls use other paths and do not preserve all interaction or metadata behavior.

A transparent proxy therefore needs a separate direct runtime. Extending aggregate catalog relay incrementally is not an acceptable implementation shortcut.

### Existing OAuth route model is insufficient for random ports

Current protected MCP routes derive a public resource from host plus path and assume HTTPS default-port formatting. A random Tailscale port is part of the OAuth resource identifier and audience, so the proxy must use an exact full resource URL such as:

```text
https://node.tailnet.ts.net:53147/mcp
```

The existing protected-route config should not be overloaded for ephemeral proxy resources.

### OAuth audiences are runtime state

`labby-auth::AuthState` keeps additional accepted OAuth resource audiences in a shared in-memory map. The authorization and token endpoints reject an unregistered resource.

Current route refresh code replaces that entire map from persisted protected routes. Therefore ephemeral proxy resources require a first-class lease registry with separate configured and leased resources. Calling the current replace method from request handling would otherwise erase active proxy leases.

OAuth mode needs:

- a stable, already-running Labby authorization server;
- a short-lived lease for the exact proxy resource URL and scopes;
- periodic lease renewal;
- removal on normal shutdown;
- expiry after crashes;
- local JWT validation against the exact audience, issuer, expiry, and scopes;
- no fallback to bearer or no-auth if lease registration fails.

The existing `LiveGateway` thin-client path and generic action dispatch can carry lease create, renew, and release actions to the running daemon.

## Tailscale findings

The current CLI supports:

- foreground or background Serve;
- a selected HTTPS port through `--https`;
- non-interactive updates through `--yes`;
- reverse proxying to a loopback HTTP target;
- JSON status output;
- multiple existing ports and handlers on one node.

The proxy should use foreground Serve, owned as a child process. It should:

1. read status and choose a port absent from both TCP and Web maps;
2. spawn `tailscale serve --yes --https=<port> http://127.0.0.1:<local-port>`;
3. wait until status contains the exact expected backend mapping;
4. watch both the Serve process and mapping during runtime;
5. stop the foreground process on shutdown;
6. verify the exact mapping disappeared;
7. use exact-port `off` cleanup only if the mapping still points to the proxy's backend;
8. never use `tailscale serve reset`;
9. refuse to remove a mapping that no longer matches the proxy's ownership record.

A random port pre-check is not a reservation. Collision handling must be based on the actual Serve command result and verified status.

## Resolved product decisions

1. **Primary command:** `labby proxy /path/to/dist.js`.
2. **No required flags:** configured defaults and built-in defaults drive exposure, auth, and port selection.
3. **Built-in exposure default:** Tailscale Serve.
4. **Built-in auth default:** tailnet policy only. Bearer and OAuth become zero-flag after setup.
5. **No silent exposure fallback:** if Tailscale is selected but unavailable, fail. Do not bind to LAN.
6. **No silent auth fallback:** OAuth, bearer, tailnet, and none are distinct policies.
7. **Internal listener:** always `127.0.0.1:0` unless an explicit local-only development mode says otherwise.
8. **External random range:** IANA dynamic/private range, 49152 through 65535, configurable.
9. **Normal lifetime:** foreground. Ctrl+C owns and stops child, HTTP listener, OAuth lease, and Serve mapping.
10. **Transparent catalog:** no Labby built-ins, Code Mode, namespacing, filtering, or result normalization.
11. **Separate secret:** `LABBY_PROXY_BEARER_TOKEN`; never reuse `LABBY_MCP_HTTP_TOKEN`.
12. **OAuth issuer:** stable Labby daemon. Random-port proxy is the protected resource, not the issuer.
13. **Modern behavior:** full concurrent stateless forwarding with per-request metadata.
14. **Legacy behavior:** compatibility adapter with serialization where independent server-to-client request association would otherwise be ambiguous.
15. **Unknown extensions:** preserved and covered by round-trip tests.

## Integration constraints

### RMCP upgrade branch

The proxy implementation must begin from a committed RMCP 3.1.0 upgrade. Do not duplicate the uncommitted work in `chore/pin-rmcp-3.1.0-20260731`. Merge or cherry-pick its final commit after it passes the workspace and conformance gates.

### MCP capability branch

The direct proxy should live in new modules and minimize overlap with the active aggregate-gateway capability work. It may reuse generic fixes after those changes land, but it must not rely on aggregate catalog behavior for correctness.

### Conformance gate

The current conformance script is pinned to RMCP 3.0.0-beta.2 and a matching commit. The RMCP upgrade deliverable must update:

- script comments;
- RMCP version;
- RMCP tag;
- exact source commit;
- expected failure baselines where justified;
- upstream drift baseline.

## Implementation outcomes

The implementation resolved the research spikes with a dedicated direct-stdio
connector, the shared transparent bridge, a stateless loopback HTTP service,
explicit subscription/cancellation coverage, owned Tailscale Serve status
checks, and daemon-backed OAuth leases. Parser tests retain `OsString` child
arguments, and the stdio connector reuses Unix process-group and Windows Job
Object ownership.

The shipped operator boundary differs from two early research assumptions:

- lease create, renew, and release use authenticated gateway actions over
  `POST /v1/gateway`, not new route-local REST endpoints;
- direct-proxy OAuth metadata is always the origin-root
  `/.well-known/oauth-protected-resource` document, while configured Gateway
  protected routes keep their existing path-suffixed metadata contract.

The dated proof-pack harness and controlled live release evidence remain owned
by Task 14 of the implementation plan; they do not change the Task 13 operator
or generated-document contract.

## Authoritative sources

- MCP 2026-07-28 RC announcement: https://blog.modelcontextprotocol.io/posts/2026-07-28-release-candidate/
- MCP 2026-07-28 RC source tag: https://github.com/modelcontextprotocol/modelcontextprotocol/tree/2026-07-28-RC
- MCP versioning and compatibility: https://modelcontextprotocol.io/specification/draft/basic/versioning
- MCP stdio transport: https://modelcontextprotocol.io/specification/draft/basic/transports/stdio
- MCP Streamable HTTP transport: https://modelcontextprotocol.io/specification/draft/basic/transports/streamable-http
- MCP subscriptions: https://modelcontextprotocol.io/specification/draft/basic/patterns/subscriptions
- MCP discovery: https://modelcontextprotocol.io/specification/draft/server/discover
- MCP authorization: https://modelcontextprotocol.io/specification/draft/basic/authorization
- RMCP 3.1.0 release: https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.1.0
- Tailscale Serve CLI: https://tailscale.com/docs/reference/tailscale-cli/serve
- Tailscale Serve overview: https://tailscale.com/docs/features/tailscale-serve
- Clap trailing arguments: https://docs.rs/clap/latest/clap/struct.Command.html#method.trailing_var_arg

## Research conclusion

No unresolved protocol or product question prevents implementation. The remaining uncertainty is concentrated in testable adapter mechanics. The implementation plan turns each uncertain mechanism into an early spike with a binary pass/fail gate, then requires protocol conformance, adversarial fault injection, cross-platform process tests, and a live Tailscale/OAuth proof pack before release.
