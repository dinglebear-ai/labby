---
title: "Upstream MCP Proxy"
created: "2026-07-30"
updated: "2026-08-28"
---

# Upstream MCP Proxy

Lab can act as an MCP gateway, proxying tool calls and resource reads to upstream MCP servers. This lets a single `lab` instance aggregate tools from multiple MCP servers behind one authenticated endpoint.

Upstream servers are first-class providers in the merged MCP tool catalog. After discovery, their tools appear in `list_tools()` beside built-in `lab` tools. Callers do not need a separate tool or namespace to invoke proxied upstream tools themselves.

If gateway-wide `[code_mode].enabled = true`, raw upstream tools are hidden from
`list_tools()` and exposed through the primary synthetic `codemode` tool.
That mode is documented in [GATEWAY.md](./GATEWAY.md#gateway-code-mode).

`lab` also exposes a separate `gateway` management surface for editing and reloading upstream definitions. That management surface is documented in [GATEWAY.md](./GATEWAY.md).

Gateway-managed protected MCP routes are a different mode: they publish an
inline public MCP route with Lab-owned OAuth protected-resource metadata and
proxy the whole Streamable HTTP MCP route to a backend. Use
[GATEWAY.md — Gateway-Managed Protected MCP Routes](./GATEWAY.md#gateway-managed-protected-mcp-routes)
for that setup instead of `[[upstream]]` tool merging.

The reusable upstream pool lives in `crates/labby-gateway/src/upstream/`; `crates/labby/src/dispatch/upstream.rs` is the Labby product compatibility and adaptation boundary. The runtime proxy path described in this document is wired into the MCP surface. The HTTP API exposes `/v1/gateway` for gateway management and a separate authenticated `/v1/palette` projection for bounded discovery and exact upstream calls.

## Palette exact-call HTTP contract

Palette and other form-driven clients use the authenticated `/v1/palette/catalog`, `/search`, `/schema`, and `/descriptor` reads to discover compact entries and lazily hydrate bounded schemas. They execute a reviewed MCP entry with `POST /v1/palette/execute`, passing the provider-qualified `mcp:<upstream>::<tool>` ID, arguments, and the exact `expectedContractHash` returned by discovery. Destructive tools also require an authenticated administrator and `confirmDestructive: true`.

Execution does not evaluate JavaScript or invoke an LLM. It re-resolves the current OAuth-subject-scoped, exposure-filtered tool and atomically checks the executable schema, Labby-owned destructive classification, caller scopes, allowed upstreams, catalog generation, and contract hash before using the same bounded exact-call kernel as Code Mode and MCP. A removed or changed tool fails closed; calls are not automatically retried.

Successful receipts report `executionMode: "exact"`, `llmInvocations: 0`, the catalog revision and contract hash actually checked, and an `auditId` correlated with the request. MCP content and structured errors are preserved in the response. Upstream descriptions and schema display text are sanitized as untrusted data; schema depth is capped at 64, each schema at 64 KiB, descriptors at 160 KiB, search results at 100 entries, and upstream request/result bytes, queueing, timeout, cancellation, and concurrency retain the canonical gateway bounds.

## What Operators Configure

To proxy an upstream server through `lab`, you configure one or more `[[upstream]]` entries in `~/.config/labby/config.toml`, optionally provide bearer-token env vars in `~/.labby/.env`, then start `labby serve` normally.

`lab` will:

1. seed enabled upstream names into the gateway catalog at startup without opening connections
2. connect to an upstream lazily on first code mode, exact tool execution, Code Mode call, or explicit gateway test path that needs live discovery
3. merge discovered tools into its own MCP catalog after that upstream is first contacted
4. serve the combined catalog through whichever MCP transport you expose from `lab`

OAuth upstreams are discovered only when Lab has upstream OAuth runtime state
and an explicit subject for selecting the token set. Subject-less discovery
deliberately skips OAuth upstreams so a user-specific token view is not cached
globally.

That means the client connects only to `lab`:

- `labby mcp` for stdio clients such as Claude Desktop
- `labby serve` for streamable HTTP MCP clients over TCP or a configured Unix-domain socket

The client never connects directly to the upstreams once `lab` is acting as the gateway.

## Configuration

Upstream servers are configured in `config.toml` using `[[upstream]]` array entries.

### HTTP Upstream

```toml
[[upstream]]
name = "remote-lab"
url = "https://lab2.example.com/mcp"
bearer_token_env = "LABBY_UPSTREAM_TOKEN"
proxy_resources = true
expose_tools = ["search_repos", "github_*"]
```

### Unix-Socket Upstream

Unix-socket upstreams speak the same Streamable HTTP protocol as TCP upstreams. `socket_path` selects the local connection endpoint, while `url` supplies the request path and `Host` authority.

```toml
[[upstream]]
name = "cortex"
transport = "unix_socket"
socket_path = "/run/labby/cortex.sock"
url = "http://cortex.local/mcp"
bearer_token_env = "CORTEX_MCP_TOKEN"

[upstream.headers]
x-labby-tenant = "infrastructure"
```

Filesystem paths work on Unix targets. Linux also supports abstract `@name` notation, for example `socket_path = "@cortex-mcp"`. The gateway and upstream must share the same socket namespace, directly or through a bind mount. Unix sockets are same-host transports; cross-node and Tailscale traffic remains HTTP/TCP.

The Unix connector reuses the normal capped HTTP worker, so response-size limits, SSE event limits, timeouts, retry/lifecycle policy, bearer/OAuth handling, custom headers, and structured error mapping remain aligned with HTTP/TCP upstreams.

### Stdio Upstream

```toml
[[upstream]]
name = "local-server"
command = "my-mcp-server"
args = ["--port", "5000"]
proxy_resources = false
```

Stdio upstreams execute a local child process on the host running `lab`.
Gateway admin actions that test or reconcile stdio definitions are marked
destructive — including `gateway.test`, because probing a stdio gateway spawns
its local command. MCP clients confirm through elicitation when available;
clients without elicitation run without a parameter gate. See
[GATEWAY.md](./GATEWAY.md#stdio-gateways).

### Config Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Human-readable name. Must be non-empty, unique, and URI-safe (no `/`, `?`, `#`). |
| `transport` | string | Unix socket only | Explicit `http`, `websocket`, `stdio`, or `unix_socket`. Legacy URL/command inference remains supported when omitted. |
| `url` | string | HTTP/WebSocket/Unix | Network URL, or for `unix_socket`, the HTTP(S) request URI and `Host` authority. |
| `socket_path` | string | Unix socket | Filesystem socket path, or Linux abstract `@name` notation. |
| `headers` | table | no | Custom headers for HTTP and Unix-socket requests. Inline `Authorization` is forbidden; use `bearer_token_env` or OAuth. |
| `command` | string | stdio | Command to run for stdio transport. |
| `args` | string[] | no | Arguments to pass to a stdio command. |
| `env` | table | no | Environment variables injected into a stdio child process. |
| `bearer_token_env` | string | no | Name of an env var holding a bearer token for HTTP or Unix-socket transport. Not the token itself. |
| `proxy_resources` | bool | no | Whether to proxy resources from this upstream. Default: `true`. |
| `proxy_prompts` | bool | no | Whether to proxy prompts from this upstream. Default: `true`. |
| `proxy_skills` | bool | no | Whether to aggregate this upstream's Agent Skills (SEP-2640). Default: **`false`**, unlike the other `proxy_*` flags — see below. |
| `expose_tools` | string[] | no | Optional allowlist of tool names/patterns to expose from this upstream. Supports exact names and `*` wildcards. An empty list exposes nothing; omit the key to expose all. |
| `expose_resources` | string[] | no | Optional allowlist of bare upstream resource URIs/patterns to expose. Same matching rules as `expose_tools`. An empty list exposes nothing; omit the key to expose all. |
| `expose_prompts` | string[] | no | Optional allowlist of prompt names/patterns to expose. Accepts the bare or `{upstream}/{name}` spelling. An empty list exposes nothing; omit the key to expose all. |
| `expose_skills` | string[] | no | Optional allowlist of skill names/patterns to expose. An empty list exposes nothing; omit the key to expose all. |

When `transport` is omitted, an HTTP/WebSocket `url` or stdio `command` preserves legacy inference. `unix_socket` must be explicit and requires both `socket_path` and an HTTP(S) `url`; it cannot also configure `command`.

### Config File Locations

`lab` loads configuration from:

1. process environment
2. `~/.labby/.env`
3. `~/.config/labby/config.toml`

So a typical gateway setup looks like:

`~/.config/labby/config.toml`

```toml
[mcp]
transport = "http"
host = "127.0.0.1"
port = 8765

[[upstream]]
name = "remote-lab"
url = "https://lab2.example.com/mcp"
bearer_token_env = "LABBY_UPSTREAM_TOKEN"
proxy_resources = true
expose_tools = ["gateway", "search_*"]

[[upstream]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/srv/data"]
proxy_resources = false
```

`~/.labby/.env`

```bash
LABBY_UPSTREAM_TOKEN=replace-me
LABBY_MCP_HTTP_TOKEN=replace-this-too
```

### Config Validation

Validation runs before discovery. Invalid entries are skipped with a warning during startup discovery. The runtime `gateway` management surface rejects invalid mutations before writing them to disk.

| Condition | Result |
|-----------|--------|
| Empty name | Skipped |
| Duplicate name | Startup keeps the first and warns; runtime gateway mutations reject the write |
| Name contains `/`, `?`, or `#` | Skipped |
| URL scheme does not match the selected transport | Skipped |
| URL uses bind-all address (`0.0.0.0`, `::`) | Skipped |
| `socket_path` without `transport = "unix_socket"` | Skipped |
| Unix socket missing `socket_path` or HTTP(S) `url` | Skipped |
| Abstract `@name` socket on a non-Linux target | Skipped |
| Custom header name/value is invalid, or attempts to set `Authorization` | Skipped |
| Transport configures conflicting fields such as both `url` and `command` | Skipped |
| No transport can be inferred | Skipped |

### Bearer Token

The `bearer_token_env` field names an environment variable; it does not contain the token directly. At connection time, the pool reads the env var and sends the token as a bearer header for HTTP and Unix-socket upstreams. For stdio upstreams, the same named variable is injected into the child process after Labby clears the ambient environment and applies its allowlist.

HTTP and Unix-socket MCP transports never follow redirects. This applies to
JSON-RPC POSTs, streaming GETs, and session DELETEs, whether or not a bearer is
configured. A 3xx response fails with a redacted transport error; update the
configured upstream URL explicitly instead of relying on a redirect.

If the named env var is not set, HTTP and Unix-socket connections proceed without bearer auth and log a warning; stdio skips the optional injection. Stdio still rejects OAuth and custom HTTP headers because those require an HTTP transport.

Changing a bearer-token env var does not hot-apply by itself. Use `gateway.reload` when you want the live pool to re-read `bearer_token_env`.

## Upstream OAuth (authorization_code + PKCE)

OAuth-protected upstream MCP servers are authenticated for a shared gateway
credential rather than by a static bearer token. Configuration shape and examples live in
[CONFIG.md — Upstream OAuth](../runtime/CONFIG.md#upstream-oauth-authorization_code--pkce).
Operator browser flow lives in [GATEWAY.md](./GATEWAY.md).

### Scope

- HTTP upstream transport only at the wire level. `labby mcp` stdio mode now
  supports native upstream OAuth with the shared trusted subject `gateway`.
  It binds a callback listener to `127.0.0.1`, opens the authorization URL in
  the default browser on first use, and waits for the loopback callback. The
  hosted HTTP mode continues to use the public callback route.
- Subject-less discovery skips OAuth upstreams. Hosted gateway startup and
  `gateway.reload` only seed configured upstream names; live discovery happens
  later from a lazy path with an explicit subject. Shared background refresh and
  `gateway.test` use the explicit shared subject `gateway` so configured OAuth
  upstreams can be discovered after an operator completes the upstream OAuth
  flow. If no credential exists yet, they remain configured but report no
  discovered capabilities until authorization succeeds.
  The authorization initiation flow (`POST /v1/gateway/oauth/start`) requires
  an authenticated HTTP session.
- `/mcp` over HTTP, the hosted web UI, and `labby mcp` stdio are supported call
  surfaces. Stdio native OAuth requires `LABBY_OAUTH_ENCRYPTION_KEY` and uses
  `LABBY_STDIO_OAUTH_CALLBACK_PORT` when a fixed loopback port is required by
  an authorization-server registration; the default `0` selects an ephemeral
  port.

### Stdio Flow

1. Configure the upstream with an HTTP `url` and `[upstream.oauth]`; do not
   wrap it in `mcp-remote`.
2. Start `labby mcp`. The first connection that needs the upstream opens the
   provider authorization URL in the default browser.
3. The provider redirects to the process-local loopback listener. Labby
   validates the state against the encrypted SQLite store, exchanges the code,
   and retries the waiting MCP connection.
4. Later calls reuse the encrypted per-upstream credential and refresh it under
   the existing manager locks. Concurrent first calls share one browser flow.

The loopback listener is not exposed on the LAN, accepts callbacks only for a
pending state created by the current process, and never prints the callback
query or authorization code.

### Flow

1. Operator runs `POST /v1/gateway/oauth/start { "upstream": "<name>" }`; the
   server returns a JSON `{ "authorization_url": "..." }` body.
2. Browser navigates to that URL; the upstream AS authenticates the user.
3. AS redirects to `/auth/upstream/callback?code=...&state=...&upstream=<name>`
   on the same origin as `LABBY_PUBLIC_URL`.
4. `lab` validates the authenticated session, atomically takes the pending
   state row (`DELETE ... RETURNING`), exchanges the code for tokens, encrypts
   the token response with chacha20poly1305, and persists it keyed by
   `(upstream_name, "gateway")`.
5. Subsequent `/mcp` and UI requests find the persisted credential and proxy
   through a per-`(upstream, subject)` `AuthClient` cached in the gateway. The
   default shared subject is `gateway`.

CLI examples:

```bash
labby gateway mcp auth start chrome-devtools
labby gateway mcp auth open chrome-devtools --wait
labby gateway mcp auth status chrome-devtools
labby gateway mcp auth clear chrome-devtools
```

### Spec-Aligned Invariants

- **PKCE S256-only.** The AS metadata must advertise `S256` in
  `code_challenge_methods_supported`. Missing or `plain`-only metadata is
  refused with `oauth_unsupported_method`; `lab` never falls back to `plain`.
- **RFC 8707 `resource`.** The canonical upstream MCP URL (RFC 3986 §6.2.2
  normalized: lowercase scheme + host, normalized percent-encoding, default
  port elided, trailing slash preserved as configured) is sent on **both** the
  authorization request and the token request, byte-identical between the
  two. Canonicalization runs at config-validation time so the stored URL and
  the `resource` wire value are the same string. Mismatched `aud` claims on
  the returned token surface as `oauth_resource_mismatch`.

  **Known gap (upstream).** rmcp 1.4's refresh path does not re-emit the
  `resource` parameter on the `refresh_token` grant. Most authorization
  servers continue to honor the audience bound at initial exchange, so this
  is acceptable in practice today, but an AS that requires `resource` on
  every token-endpoint call will reject refreshes. Tracked for follow-up
  once rmcp exposes a refresh hook we can extend.
- **Issuer binding.** After AS metadata discovery, `metadata.issuer` is
  required — missing `issuer` surfaces as `oauth_issuer_mismatch`. The
  `authorization_endpoint`, `token_endpoint`, `revocation_endpoint`, and
  (when present) `registration_endpoint` and `userinfo_endpoint` origins
  (scheme + host + port) must match the issuer origin; any drift surfaces as
  `oauth_issuer_mismatch` (RFC 8414 §3.3). Known provider split endpoints are
  allowed when they are part of the provider's documented OAuth deployment;
  today Lab allows Google's `https://accounts.google.com` issuer to use the
  `https://oauth2.googleapis.com` token endpoint.
- **No Google reuse.** Outbound upstream OAuth is distinct from the inbound
  `labby-auth` Google provider used for user login to `lab`. They do not share
  code, clients, or tokens.

### Per-`(upstream, subject)` Client Cache

The gateway maintains a `DashMap<(upstream_name, subject), AuthClient>`
built atomically per key. Two subjects calling the same OAuth upstream get
two isolated `AuthClient` instances; one subject's tokens are never visible
to another.

Current operator surfaces default to the shared subject `gateway`, so the
common path is one cached `AuthClient` per upstream for the whole gateway.

The cache stores the `client_id` each entry was built with. A `gateway.reload`
that changes an upstream's `client_id` evicts cached entries with a stale
`client_id`; subsequent calls rebuild them. This closes a silent re-bind gap
where a config edit would otherwise keep old credentials attached to a new
upstream definition.

OAuth-tagged upstreams are never discovered by the subject-less
`discover_all` path. Gateway-owned startup/reload/test discovery uses an
explicit subject-scoped path with the shared `gateway` subject; MCP request
paths that need a real user subject use the per-request subject-scoped helpers.
The circuit breaker and catalog merging infrastructure applies to
static-bearer upstreams; OAuth upstreams are connected through the
subject-scoped auth client cache.

### Refresh Semantics

Refresh is single-flight per `(upstream_name, subject)` using a `tokio::sync::Mutex`
keyed on the pair. Lock entries are retained for the lifetime of the process.

Today the manager runs **proactive refresh only**:

- **Proactive:** before dispatching a request, if the cached access token is
  less than 30 seconds from expiry, refresh under the per-key lock first.
- **Reactive (401):** **deferred.** MCP traffic flows through rmcp's
  `StreamableHttpClientWorker`, which hides the raw HTTP response from the
  gateway, so a 401 on an MCP call currently surfaces as a generic transport
  error rather than `oauth_needs_reauth`. Operators recover by calling
  `POST /v1/gateway/oauth/start` to re-authorize. When this is wired, only
  idempotent methods (`GET`/`HEAD`/`OPTIONS`) will retry after refresh;
  non-idempotent methods (`POST`, including MCP `tool_call`) will surface
  the original 401 as `oauth_needs_reauth` without retry, because a retry
  could double-execute a destructive tool call.

On `invalid_grant` (refresh token revoked or rotated twice), `lab` returns
`oauth_needs_reauth` to the caller. The user re-initiates authorization.

### `oauth_needs_reauth` Triggers

A caller sees `oauth_needs_reauth` in any of these situations:

- no credential exists yet for `(upstream, subject)`
- the refresh token was rejected with `invalid_grant`
- decryption of the stored `token_blob` failed (operator rotated
  `LABBY_OAUTH_ENCRYPTION_KEY`)
- (future, once reactive 401 is wired) a 401 arrived on a non-idempotent
  request and retry is not safe

Recovery is identical in all cases: start a new authorization via
`POST /v1/gateway/oauth/start`.

### Token-At-Rest Encryption

Persisted token responses are sealed with chacha20poly1305 AEAD. A fresh 12-byte
nonce is generated on every `seal()` call; the refresh upsert stores the new
nonce and must never preserve the previous one. The key is loaded once at
startup from `LABBY_OAUTH_ENCRYPTION_KEY`; see [CONFIG.md](../runtime/CONFIG.md#environment-variables-2)
for rotation.

### Prior Art

The cache implementation still supports per-`(upstream, subject)` isolation
internally, but the current operator-facing flow defaults to the shared subject
`gateway` for all three surfaces.

## Discovery

At startup, lab seeds enabled upstream names into the shared gateway catalog
without opening upstream connections. Live tool discovery is lazy: the first
code mode, exact tool execution, or Code Mode upstream call connects only the
needed upstream. Background search-index refreshes use the same bounded
discovery concurrency as bulk discovery paths.

Each live discovery attempt gets a 15-second timeout for connection and tool
discovery (`list_tools()`). Failed upstreams are marked unhealthy. Healthy
upstreams continue operating. A single failed upstream does not prevent others
from connecting later.

After startup, proxied RMCP operations continue to use explicit per-RPC
timeouts. Tool calls, prompt reads, resource reads, and discovery/listing
operations must fail closed with logged timeout/error events rather than
blocking indefinitely behind one hung upstream.

```text
gateway lazy upstream catalog seeded  upstream_count=3
lazy upstream tools connected         upstream=remote-lab tool_count=12
gateway tool index reprobe failed     upstream=broken-server kind=upstream_reprobe_failed
```

## How Routing Works

The combined catalog is exposed as one MCP server, but ownership is still resolved internally.

For each incoming MCP tool call:

1. `lab` checks whether the tool name belongs to a built-in local service
2. if not, it checks the discovered upstream tool map
3. if an upstream owns that tool name, the request is proxied there using the original MCP arguments
4. the upstream result is normalized into `lab`'s usual success/error envelope shape

This internal precedence rule does not make upstream tools second-class. It is just how collisions are resolved.

## Tool Collision Handling

When upstream tools are merged into the lab tool catalog:

1. **Built-in lab services always take precedence.** If an upstream exposes a tool named `gateway`, the upstream tool is silently dropped (with a warning logged).
2. **Cross-upstream duplicates: first discovered wins.** If two upstreams expose a tool named `my-tool`, the second is skipped with a warning.

Upstream tools appear alongside built-in tools in `list_tools()`. Callers do not need to know whether a tool is built-in or proxied.

## Exposure Filtering

Each upstream may optionally restrict which discovered primitives become visible
downstream: `expose_tools`, `expose_resources`, and `expose_prompts`. All three
compile through the same allowlist matcher and behave identically.

- an unset allowlist means "expose everything discovered for that capability"
- exact entries match one name/URI
- entries containing `*` use simple wildcard matching
- malformed allowlists fail closed: the upstream stays connected, but nothing
  from that capability is exposed until the config is fixed

What each allowlist matches:

| Field | Matched against |
|-------|-----------------|
| `expose_tools` | the tool name the upstream advertises |
| `expose_resources` | the bare, upstream-native resource URI — the form reported by `gateway.discovered_resources`, **not** the `lab://upstream/{name}/…` rewrite |
| `expose_prompts` | the bare prompt name the upstream advertises, or the `{upstream}/{name}` namespaced form reported by `gateway.discovered_prompts` — either spelling works |

Every allowlist applies to both discovery and direct access, on the shared
catalog path, the OAuth subject-scoped path, and the MRTR relay path:

1. listing (`list_tools`, `resources/list`, `prompts/list`), so filtered items
   are never advertised
2. direct access (`tools/call`, `resources/read`, `prompts/get`) and
   `completion/complete`, so a filtered item cannot be reached by name or URI
   even by a caller that already knows it

Filtering only the listing would be a bypass rather than a restriction, so the
direct-access gate is the load-bearing half.

The cached inspection snapshots (`gateway.discovered_resources` /
`gateway.discovered_prompts`) deliberately stay **unfiltered** — they are what
the admin UI shows while an operator edits the allowlist, and hiding excluded
entries there would make the allowlist un-editable.

Resource *templates* (`resources/templates/list`) are not filtered by
`expose_resources`, and neither is `completion/complete` for a
`Reference::Resource`: a template is a URI pattern, not a concrete URI, so there
is nothing well-defined to match. Reads of any URI a template expands to are
still gated by `expose_resources`. `completion/complete` for a
`Reference::Prompt` *is* gated, because a prompt reference is an exact name.

## Circuit Breaker

Each upstream has independent health tracking.

| Constant | Value |
|----------|-------|
| `CIRCUIT_BREAKER_THRESHOLD` | 3 consecutive failures |
| `REPROBE_INTERVAL` | 30 seconds |

### State Transitions

- **Healthy** — upstream is routable. 0 consecutive failures.
- **Unhealthy (below threshold)** — upstream has 1-2 consecutive failures. Still routable and included in tool listings.
- **Unhealthy (at/above threshold)** — upstream has 3+ consecutive failures. Excluded from tool listings.

### What Counts as a Failure

- Connection errors
- Tool call errors (`is_error` responses)
- Prompt and resource proxy errors
- Dropped connections
- Timeouts
- Response size cap exceeded

### Recovery

- A successful proxied call resets the upstream to healthy (0 failures).
- The code defines a `REPROBE_INTERVAL` of 30 seconds and tracks when an upstream became unhealthy.
- Automatic scheduled re-probing is not currently wired into the runtime. In practice, recovery happens when a later proxied call or resource request succeeds.

## Response Size Cap

Upstream responses are subject to a size cap to prevent oversized payloads from consuming memory or being forwarded to callers.

| Setting | Default |
|---------|---------|
| `LABBY_UPSTREAM_MAX_RESPONSE_BYTES` | 10 MB (10,485,760 bytes) |

The check is **post-hoc** — rmcp materializes the full response in memory before lab can inspect it. The cap prevents forwarding oversized payloads to callers but cannot prevent the memory allocation itself. A streaming limit would require rmcp transport-level support.

The cap applies to both `call_tool` and `read_resource` responses.

## Resource Proxying

Resource proxying is opt-in per upstream via `proxy_resources = true`.

### URI Namespacing

Upstream resources are prefixed to avoid URI collisions with lab's own resources:

```text
lab://upstream/{name}/{original_uri}
```

For example, if upstream `remote-lab` exposes a resource `lab://gateway/actions`, it appears as:

```text
lab://upstream/remote-lab/lab://gateway/actions
```

### Operations

- `list_resources()` queries all resource-enabled upstreams and returns namespaced URIs.
- `read_resource()` strips the prefix, identifies the upstream by name, and forwards the read.

Failed resource listings from individual upstreams are logged as warnings. Other upstreams continue to serve.

The same graceful-degradation rule applies to prompt/resource discovery and
reads: one upstream failure must not prevent healthy upstreams from serving
partial results.

## Skills Aggregation

Skills aggregation implements [SEP-2640](../contracts/skills-extension.md), an
**unmerged** draft. The contract doc pins the exact revision this code was
written against; read it before changing anything here.

It is opt-in per upstream via `proxy_skills = true`, and unlike every other
`proxy_*` flag it defaults to **`false`**. That asymmetry is deliberate: a
skill is a set of instructions an agent will act on, so aggregating one is a
trust decision about the upstream, not a convenience toggle.

```toml
[[upstream]]
name = "acme"
command = "acme-mcp-server"
proxy_skills = true
expose_skills = ["refunds"]   # omit to expose all
```

Enable it from the CLI with:

```bash
labby gateway add --name acme --command acme-mcp-server --proxy-skills true
```

### Per-origin namespacing

Every aggregated skill is relabelled under the upstream's host-assigned origin,
so two upstreams may each publish a skill named `refunds` without collision:

```text
skill://{origin}/{skill}/{file}
skill://acme/refunds/SKILL.md
```

`labby` is **reserved** for Labby's own first-party skills and can never be
claimed by an upstream. Nothing is ever deduplicated by skill name — two
origins publishing the same name are two distinct skills.

Provenance travels in the entry's `_meta` under
`ai.dinglebear.labby/skillOrigin`, never in `frontmatter`. Frontmatter is
content the upstream authored; putting provenance there would let an upstream
forge its own origin.

### Verified reads

A skill entry publishes a per-file sha256 digest. When a client reads one of
those files, the bytes are hashed and compared against the digest the entry
published **before any byte reaches the client**:

- content that does not match its digest → `skill_digest_mismatch`, zero bytes
  served
- a file the manifest does not list → `skill_manifest_stale`; an unlisted file
  is a *changed skill*, not a fetchable one, so it is refused rather than
  fetched

Both classify as `validation` / `rediscover` / `same_arguments: never` — an
agent must refresh the entry rather than replay the identical read, because a
changed resource set revokes any approval bound to the previous content.

Digests are an integrity check against drift and corruption, **not a security
boundary**: an upstream that serves malicious content can publish a matching
digest for it. The trust decision is `proxy_skills`.

### Degradation

One unreachable upstream is skipped rather than emptying the listing, and
per-upstream errors surface in `gateway.skills.list` rather than being folded
into a silent empty result. Per the SEP, an empty or partial listing is never
proof that a server has no skills — an unlisted skill may still be loadable by
URI.

### Labby's own skills

Labby serves first-party skills under the reserved `labby` origin: those
embedded in the binary, plus any operator-provided skill directories under
`$LABBY_HOME/skills`. Operator skills are read and digested in a single pass at
startup, so adding one requires a restart — re-reading per request would let a
file change between publishing a digest and serving the file it describes,
which is exactly the mismatch a conforming client must refuse. A skill is
skipped, with a logged reason, if it contains a symlink at any depth, omits
`SKILL.md`, exceeds the size or file-count caps, or its directory name
disagrees with its frontmatter `name`.

## What Is Exposed Where

### MCP

The upstream gateway is active on both MCP transports exposed by `lab`:

- stdio
- streamable HTTP at `/mcp`

If an upstream tool is discovered successfully, MCP clients connected to `lab` can call it as a normal tool.

### HTTP API

The product HTTP API under `/v1/*` does not proxy arbitrary upstream MCP tools. It serves built-in `lab` routes plus `/v1/gateway` for gateway management.

Keep this distinction explicit in operator docs:

- use MCP when you want the upstream gateway behavior
- use `/v1/gateway` when you want to manage `[[upstream]]` entries over HTTP
- use the rest of `/v1/*` for `lab`'s built-in HTTP API surface

## End-to-End Setup

### 1. Configure upstreams

Add one or more `[[upstream]]` entries to `~/.config/labby/config.toml`.

### 2. Provide any required secrets

Set bearer-token env vars named by `bearer_token_env` in `~/.labby/.env` or the process environment.

### 3. Start `labby`

For local stdio clients:

```bash
labby mcp
```

For network MCP clients:

```bash
labby serve
```

### 4. Point the client at `labby`, not the upstreams

Example `.mcp.json` for stdio:

```json
{
  "mcpServers": {
    "labby": {
      "command": "labby",
      "args": ["mcp"]
    }
  }
}
```

This is the local stdio bridge: the client does not need an HTTP URL. If a
`labby serve` daemon is already running, `labby mcp` forwards the session to
that daemon; otherwise it starts a standalone local gateway. See the
[transport guide](../surfaces/TRANSPORT.md#local-bridge-to-the-running-daemon)
for explicit-target and fallback behavior.

Example HTTP MCP endpoint:

```text
https://lab.example.com/mcp
```

### 5. Verify discovery

Startup logs should show lazy seeding rather than live upstream discovery:

```text
phase="discovery.lazy" upstream_count=3
```

Then trigger a first search or invoke and verify live discovery for only the
requested upstream, for example `lazy upstream tools connected upstream=remote-lab`.

Then an MCP client connected to `lab` should see the upstream tools in `list_tools()`.

## Operational Notes

- Upstream tool schemas are cached from discovery and reused for MCP tool metadata.
- Upstream calls preserve the original MCP argument payload rather than forcing it through `lab`'s `action` + `params` wrapper.
- Upstream errors are normalized into `lab` envelopes and usually surface as `upstream_error`, `network_error`, `server_error`, `decode_error`, or `internal_error`.
- Response-size limits are enforced after the upstream response is materialized in memory.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LABBY_UPSTREAM_MAX_RESPONSE_BYTES` | 10485760 | Maximum response size from upstream servers. |
| (per `bearer_token_env`) | — | Bearer token for each upstream, named in config. |

## Observability

Discovery events are logged at `INFO` (success) and `WARN` (failure/timeout).

Circuit breaker state changes are logged:

- `WARN` when the breaker opens (3+ failures).
- `INFO` when the breaker resets (successful call after failure).

Tool collision warnings are logged at `WARN`.

## Related Docs

- [CONFIG.md](../runtime/CONFIG.md) — `[[upstream]]` config section
- [MCP.md](../surfaces/MCP.md) — upstream tool merging in MCP surface
- [ERRORS.md](../dev/ERRORS.md) — `upstream_error` kind
- [TRANSPORT.md](../surfaces/TRANSPORT.md) — HTTP transport setup
