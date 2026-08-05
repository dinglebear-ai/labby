# Stdio MCP Proxy Implementation Plan

> **For implementers:** Execute this plan in order. Every task has a test-first gate and an evidence requirement. Do not collapse the direct proxy into the aggregate gateway.

**Goal:** After one-time configuration, `labby proxy /path/to/dist.js` launches a stdio MCP server, exposes a faithful Streamable HTTP endpoint on a random high port through Tailscale Serve, applies the configured tailnet, bearer, or OAuth policy, and owns complete cleanup on exit.

**Research report:** `docs/reports/2026-07-31-stdio-mcp-proxy-research.md`

**Implemented operator guide:** `docs/guides/STDIO_MCP_PROXY.md`

**Stable spec and contract:** `docs/specs/stdio-mcp-proxy.md` and
`docs/contracts/stdio-mcp-proxy.md`

**Base commit used for this plan:** `eff39c79d97c907f6c9956f4711ecab5cd8df62f`

## Release contract

No software test can prove that every possible defect is absent. This plan replaces vague confidence with a falsifiable release contract:

- every protocol behavior has a named automated test;
- every external integration has a deterministic fake and a live proof;
- every cleanup path is fault-injected;
- every dependency and external fixture is pinned by version and commit;
- every release candidate produces a machine-readable evidence bundle;
- the full proof suite must pass twice from a clean checkout at the exact commit being released;
- no expected failure may be added without a linked rationale and owner;
- no deliverable is accepted from configuration, compilation, or logs alone when runtime verification is possible.

A release is valid only when the evidence manifest reports every mandatory gate as passed and its recorded git tree is clean.

## User-facing contract

### Normal use

```console
labby proxy /path/to/dist.js
```

Expected output:

```text
MCP proxy ready

  Server   node /path/to/dist.js
  URL      https://node.tailnet.ts.net:53147/mcp
  Exposure Tailscale Serve
  Auth     OAuth

Press Ctrl+C to stop.
```

### Child arguments

```console
labby proxy /path/to/dist.js --workspace /srv/data --read-only
```

Once the first child-command token is consumed, subsequent tokens belong to the child, including tokens beginning with `-`.

### One-run overrides

Labby options appear before the child target:

```console
labby proxy --port 52177 /path/to/dist.js
labby proxy --auth oauth /path/to/dist.js
labby proxy --bearer-token "$TOKEN" /path/to/dist.js
labby proxy --local --auth none /path/to/dist.js
```

An explicit separator remains supported for unusual command lines:

```console
labby proxy -- npx -y @modelcontextprotocol/server-filesystem /srv/data
```

### Setup

```console
labby setup proxy
```

The setup flow writes non-secrets to `~/.labby/config.toml` and secrets to `~/.labby/.env`. After setup, the normal proxy command requires no flags.

## Product decisions

1. The proxy is foreground-only in the first release.
2. The default exposure is Tailscale Serve.
3. The built-in auth default is `tailnet`, meaning Tailscale reachability and grants without an additional application token.
4. Bearer and OAuth are configuration defaults selected once through setup.
5. There is no silent fallback between exposure or auth modes.
6. The local HTTP listener binds to `127.0.0.1:0`.
7. The external random port range defaults to 49152 through 65535.
8. Tailscale Funnel is out of scope.
9. The proxy exposes only the child server. It does not add Labby tools, Code Mode, catalog prefixes, filters, or normalization.
10. Unknown MCP extension methods and payloads must round-trip unchanged.
11. Modern 2026 requests may run concurrently.
12. Legacy initialized requests use a serialization gate when independent server-to-client request association would be ambiguous.
13. The proxy bearer token is separate from the Labby administrator token.
14. OAuth uses a stable Labby authorization server and an ephemeral resource lease for the exact random-port URL.
15. A Tailscale or OAuth failure stops startup. It never produces a partially exposed or silently weakened endpoint.

## Non-goals for the first release

- detached or persistent proxies;
- `proxy list` and `proxy stop`;
- automatic child restart;
- public internet exposure;
- Tailscale Service virtual IPs;
- multiple stdio children behind one endpoint;
- persisted proxy definitions;
- editing tailnet ACLs or grants;
- remote deployment of the child executable;
- changing aggregate gateway naming or catalog behavior.

## Configuration contract

Add a top-level `[proxy]` table:

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

A fixed external port is represented by an integer:

```toml
[proxy]
port = 52177
```

Recommended model:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyExposure {
    Tailscale,
    Local,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyAuthMode {
    Tailnet,
    Bearer,
    Oauth,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProxyPortPreference {
    Fixed(u16),
    Named(ProxyPortMode),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyPortMode {
    Random,
}
```

### Validation

Reject configuration when:

- the path is empty, lacks a leading slash, is `/`, or contains a query or fragment;
- the range start is greater than the end;
- either range endpoint is below 1024 unless an explicit advanced override is present;
- the fixed port is zero;
- `auth = "tailnet"` with `exposure = "local"`;
- `auth = "bearer"` has no token from the CLI, process environment, or configured env-file key;
- `auth = "oauth"` has no stable public issuer, no reachable live daemon, or no scopes;
- an inherited environment variable name is invalid;
- shutdown grace exceeds a documented upper bound.

### Precedence

1. one-run CLI override;
2. process environment and `~/.labby/.env`;
3. `[proxy]` configuration;
4. built-in defaults.

Never persist a one-run literal bearer token.

## CLI and command resolution

### Parser

Add `Proxy(ProxyArgs)` to the top-level command enum and `crates/labby/src/cli/proxy.rs`.

Use a trailing positional vector of `OsString`, not `String`. The parser must preserve non-UTF-8 arguments on Unix and must never pass the command through a shell.

The exact Clap shape must be proven by tests before runtime code is added. The intended behavior is:

- Labby flags are parsed only before the first child token;
- all later tokens are child arguments;
- `--` is accepted but not required;
- missing child command is a usage error;
- `--bearer-token` implies `auth = bearer`;
- `--local` implies `exposure = local` but does not silently rewrite auth.

### Resolver precedence

For the first command token:

1. If it names an existing executable file, execute it directly.
2. If it names a file with a valid shebang, use the operating system's direct executable path when possible. If not executable on Unix, parse only a standards-compliant shebang into an interpreter and optional single argument.
3. If it ends in `.js`, `.mjs`, or `.cjs`, resolve `node` through `PATH` and prepend the file.
4. If it ends in `.py`, resolve `python3` through `PATH` and prepend the file.
5. If it is a bare command, resolve it through `PATH`.
6. If it is an unknown non-executable file, fail with explicit suggested invocations.

Do not guess a TypeScript runtime. A `.ts` file requires a shebang, explicit command, or future configured launcher map.

### Child process policy

Introduce:

```rust
pub enum StdioSpawnAuthority {
    PersistedConfiguration,
    ExplicitLocalCli,
}
```

`ExplicitLocalCli` bypasses the persisted-config command-name allowlist because the operator typed the command directly. It does not bypass:

- shell avoidance;
- environment clearing;
- runtime environment allowlist;
- explicit environment inheritance;
- stderr draining;
- process group or Job Object ownership;
- shutdown escalation;
- secret redaction.

The default child working directory is the caller's current directory. Add a `--cwd` override. Do not add cwd to the shared persisted upstream schema solely for this feature unless another consumer needs it.

## Architecture

```text
Remote MCP client
        |
        | HTTPS over tailnet
        v
Tailscale Serve child process
        |
        | HTTP to 127.0.0.1:<ephemeral>
        v
Proxy HTTP router
  - Host and Origin validation
  - tailnet / bearer / OAuth policy
  - protected-resource metadata
        |
        v
RMCP StreamableHttpService
        |
        v
DirectProxyService<RoleServer>
  - request metadata forwarding
  - ID and token correlation
  - cancellation
  - subscriptions
  - legacy interaction gate
        |
        v
DirectUpstream
  - RMCP 3.1 client lifecycle Auto
  - stdio transport
  - child process guard
        |
        v
stdio MCP child
```

### Module layout

Create or extract:

```text
crates/labby/src/cli/proxy.rs
crates/labby/src/proxy/mod.rs
crates/labby/src/proxy/config.rs
crates/labby/src/proxy/http.rs
crates/labby/src/proxy/auth.rs
crates/labby/src/proxy/supervisor.rs
crates/labby/src/proxy/tailscale.rs
crates/labby/src/proxy/verify.rs

crates/labby-gateway/src/direct_proxy/mod.rs
crates/labby-gateway/src/direct_proxy/service.rs
crates/labby-gateway/src/direct_proxy/upstream.rs
crates/labby-gateway/src/direct_proxy/correlation.rs
crates/labby-gateway/src/direct_proxy/subscriptions.rs
crates/labby-gateway/src/direct_proxy/legacy.rs
crates/labby-gateway/src/upstream/stdio_process.rs

crates/labby-auth/src/resource_leases.rs
```

Names may be adjusted to existing module conventions, but responsibilities must remain separated and testable.

## Direct MCP forwarding design

### Upstream startup

1. Resolve the command vector.
2. Spawn the child with the extracted stdio process guard.
3. Construct a direct RMCP client service using `ClientLifecycleMode::Auto`.
4. Preferred versions begin with `2026-07-28`; legacy fallback uses the latest supported initialized version.
5. Record whether the child negotiated modern discovery or legacy initialize.
6. Probe the child before publishing any external URL.
7. Capture the child's implementation, versions, capabilities, and extension map without changing names or payloads.

### Requests

Implement low-level `Service<RoleServer>` so complete request enums, including `CustomRequest`, remain available.

For each downstream request:

1. Clone the request and its metadata.
2. Preserve protocol version, client info, capabilities, log level, trace context, extension keys, and unknown fields.
3. Replace the downstream progress token with a unique upstream token and register a reverse mapping.
4. Send the complete request using a cancellable RMCP request handle and explicit request metadata.
5. Race the upstream result against the downstream cancellation token.
6. On downstream cancellation, send `notifications/cancelled` for the upstream request.
7. Return the complete `ServerResult` without reconstructing known result types.
8. Remove correlation entries in a drop-safe finalizer.

Forward `server/discover` to the child rather than synthesizing a catalog result. This permits child capability responses to reflect the actual downstream request metadata.

### Notifications

Forward complete client notifications. Special cases:

- cancellation uses mapped upstream request IDs;
- initialized is valid only for a legacy downstream lifecycle and must never leak into a modern child;
- custom notifications preserve method, params, and metadata;
- invalid or unknown correlation targets are logged with redacted identifiers and ignored or rejected according to the protocol.

### Progress

Maintain a correlation record equivalent to:

```text
upstream_progress_token -> {
    downstream_peer,
    downstream_progress_token,
    downstream_request_id
}
```

Progress from the child is delivered only to the originating HTTP response stream. Two concurrent downstream requests using the same progress token must remain isolated.

### MRTR and tasks

For 2026 children:

- return `InputRequiredResult`, task creation, task status, and task acknowledgments unchanged;
- preserve `resultType`, task metadata, input request IDs, and input responses;
- do not automatically answer sampling, roots, or elicitation;
- support custom extension task methods through RMCP custom variants when not represented by a typed variant.

For older children, use the legacy interaction adapter below.

### Modern subscriptions

For a modern child:

1. Receive downstream `subscriptions/listen` and its requested filter.
2. Open upstream `peer.listen` with the same filter and explicit downstream metadata.
3. Wait for the upstream acknowledgment.
4. Acknowledge only the upstream-accepted subset to the downstream stream.
5. Rewrite every upstream subscription ID to the downstream request ID.
6. Preserve all notification bodies and unknown metadata.
7. Cancel the upstream listen request when the downstream SSE stream closes.
8. Deliver the final graceful teardown result when the child supplies one.

### Legacy subscription adapter

For an initialized child:

- tools, prompts, and resources list-change notifications are registered as proxy listeners only if the child advertises the corresponding legacy list-changed capability;
- requested resource URIs are translated to legacy `resources/subscribe` calls;
- URI subscriptions are reference counted across downstream listen streams;
- `resources/unsubscribe` is sent only when the last downstream subscriber leaves;
- global legacy notifications are fanned out only to streams whose acknowledged filter includes them;
- each emitted notification receives the downstream subscription ID;
- the acknowledgment omits every unsupported filter field.

### Legacy server-to-client requests

Legacy stdio has no request-stream association marker. To avoid routing a sampling, roots, or elicitation request to the wrong downstream caller:

- hold a fair `LegacyInteractionGate` around ordinary legacy requests that can produce independent server-to-client requests;
- expose the currently active downstream peer and request context to the legacy client handler;
- route create-message, list-roots, elicitation, and custom server requests through that context;
- clear the context before releasing the gate;
- never hold the gate for the lifetime of a modern subscription stream;
- add starvation and cancellation tests.

This is a correctness tradeoff for old servers. Modern 2026 requests remain concurrent.

## HTTP runtime

### Listener

- bind `127.0.0.1:0`;
- record the selected local port;
- expose `POST /mcp` using RMCP 3.1 Streamable HTTP;
- expose the RFC 9728 protected-resource metadata path when OAuth is enabled;
- expose local `/health` and `/ready` endpoints with no server catalog or secret data;
- use SSE-capable response configuration, not forced JSON responses;
- include `X-Accel-Buffering: no` on SSE responses;
- disable legacy protocol sessions for modern requests while retaining RMCP's negotiated legacy compatibility where required.

### Host and Origin

Allowed hosts are constructed only after the final external URL is known:

- `127.0.0.1:<local-port>`;
- `localhost:<local-port>`;
- exact Tailscale DNS name plus external port;
- explicit local override host, when applicable.

Allowed browser origins include the exact HTTPS Tailscale origin. Invalid present Origin headers return 403. Do not disable RMCP's metadata-header validation.

### Readiness

The endpoint becomes ready only after:

- child discovery succeeds;
- HTTP listener is accepting;
- auth policy is constructed;
- OAuth resource lease is active, when selected;
- Tailscale status shows the exact mapping, when selected.

Do not print the final URL before every readiness condition passes.

## Authentication

### Tailnet

`auth = "tailnet"` adds no application token. It is valid only with Tailscale exposure. The output must clearly say that access is controlled by tailnet grants and ACLs.

### Static bearer

Sources, in precedence order:

1. `--bearer-token`;
2. `--bearer-token-stdin`;
3. process environment variable named by `bearer_token_env`;
4. the same key in `~/.labby/.env`.

Requirements:

- separate key, default `LABBY_PROXY_BEARER_TOKEN`;
- constant-time comparison;
- `WWW-Authenticate: Bearer` on failure;
- protection applies to MCP POST and its SSE response lifecycle;
- token never appears in human output, JSON output, traces, panic messages, process titles, or evidence artifacts;
- literal CLI token is never persisted;
- setup can generate at least 256 bits of cryptographic randomness;
- `labby setup proxy --bearer-token-stdin` is the documented automation path.

### OAuth

The exact resource and audience is:

```text
https://<tailscale-dns>:<external-port>/mcp
```

The stable authorization server is the configured Labby public URL.

#### Resource lease registry

Refactor `AuthState` so configured protected routes and ephemeral leases are independent inputs to one effective resource map.

Recommended API:

```rust
replace_configured_resource_scopes(...)
create_resource_lease(resource, scopes, ttl, owner) -> LeaseId
renew_resource_lease(lease_id, ttl)
release_resource_lease(lease_id)
prune_expired_resource_leases(now)
effective_resource_scopes(resource)
```

Do not let request-time route refresh erase leases.

Expose administrator-only live-daemon actions:

```text
gateway.oauth.resource_lease.create
gateway.oauth.resource_lease.renew
gateway.oauth.resource_lease.release
```

The create action returns a random lease ID and expiration. The proxy renews at one third of the TTL with jitter. The daemon prunes expired leases independently. Owner metadata is a non-secret process fingerprint used only for diagnostics.

#### Token validation

The proxy validates:

- signature from the stable Labby issuer;
- exact issuer;
- exact resource audience including port and path;
- expiration and not-before;
- required scopes;
- token type and supported algorithm.

Reuse `labby-auth` signing and JWKS validation rather than implementing JWT parsing in the CLI. A same-host issuer may load shared keys; a remote configured issuer must use metadata plus JWKS with bounded caching and refresh-on-key-miss.

Serve Protected Resource Metadata that advertises:

- the exact proxy resource;
- the stable authorization server;
- configured proxy scopes;
- header bearer method.

On startup, OAuth mode must:

1. detect the live Labby daemon;
2. verify its stable public issuer;
3. create the resource lease;
4. construct the validator;
5. verify metadata is reachable;
6. publish through Tailscale.

If any step fails, release the lease if created and stop the child. Never downgrade auth.

## Tailscale Serve controller

### Discovery

Run and parse:

- `tailscale version`;
- `tailscale status --json`;
- `tailscale serve status --json`.

Verify:

- backend state is running;
- the local node is online;
- a DNS name exists;
- HTTPS Serve can be used;
- the configured fixed port is not already owned by another mapping.

Trim the trailing dot from the DNS name only for URL construction.

### Port allocation

For random ports:

1. use a CSPRNG to shuffle or sample candidates from the configured range;
2. skip candidates present in both current TCP and Web maps;
3. attempt the real Serve command;
4. retry only recognized collisions or concurrent configuration conflicts;
5. cap attempts and return a diagnostic with the range and last error.

Never use a listener pre-bind as proof that a Tailscale virtual port is free.

### Foreground process ownership

Spawn:

```console
tailscale serve --yes --https=<external-port> http://127.0.0.1:<local-port>
```

The controller records:

- executable identity and version;
- child process guard;
- external port;
- expected DNS authority;
- exact local backend URL;
- a normalized status fingerprint of the mapping.

Readiness is status-based, not stdout-text-based.

### Cleanup

1. signal the foreground Serve process;
2. wait for the mapping to disappear;
3. if it remains, re-read status;
4. use exact-port `off` only when the mapping still points to the recorded backend;
5. if ownership changed, refuse removal and report the conflict;
6. verify unrelated mappings match the pre-start snapshot;
7. never call `reset`.

The exact supported command shape for fallback cleanup must be established by the Tailscale spike and covered by versioned tests.

## Supervisor

Create one cancellation tree for:

- operator Ctrl+C or SIGTERM;
- child process exit;
- HTTP server exit;
- Tailscale Serve exit;
- OAuth lease renewal failure;
- unrecoverable mapping drift.

Whichever terminal condition occurs first cancels the complete runtime.

Shutdown order:

1. mark readiness false;
2. stop accepting new HTTP requests;
3. cancel active MCP requests and subscriptions;
4. close child stdin and wait;
5. escalate child process termination after grace;
6. stop and verify Tailscale mapping cleanup;
7. release OAuth lease;
8. flush bounded logs and print final status.

Cleanup must be idempotent and safe when startup stops halfway through.

## Observability and output

### Human output

Print only:

- resolved child command;
- child server identity;
- final endpoint;
- exposure mode;
- auth mode;
- selected port;
- stop instruction.

Do not print secrets, full environment, JWT claims, or raw auth headers.

### JSON output

`--json` returns one startup record:

```json
{
  "url": "https://node.tailnet.ts.net:53147/mcp",
  "exposure": "tailscale",
  "auth": "oauth",
  "externalPort": 53147,
  "localPort": 38417,
  "command": ["node", "/path/to/dist.js"],
  "protocol": "2026-07-28",
  "server": {"name": "example", "version": "1.0.0"}
}
```

No token, lease ID, unredacted subject, or environment value is included.

### Logs

Use stable event names and fields for:

- command resolution;
- child spawn and exit;
- protocol lifecycle selected;
- local listener readiness;
- auth lease create, renew, and release;
- Tailscale candidate, claim, drift, and cleanup;
- request cancellation;
- subscription open and close;
- supervisor terminal reason.

Hash or redact user, subject, token, request ID, progress token, and lease identifiers.

## Implementation tasks

### Task 0: Land RMCP 3.1.0 as an isolated prerequisite

**Files:** workspace Cargo files, `scripts/ci/mcp-conformance.sh`, drift baselines, migration call sites.

**Steps:**

1. Finish the existing `chore/pin-rmcp-3.1.0-20260731` worktree rather than duplicating it.
2. Verify the MCP `2026-07-28-RC` source tag still resolves to `9d700ed62dcf86cb77475c9b81930611a9182f46`, record any live-draft drift, and update this plan before coding if the target changes.
3. Update the exact workspace pin to `=3.1.0`.
4. Update the conformance script to tag `rmcp-v3.1.0` and source commit `1f9358eddca42d3a510c70ae6446dd6548c7c856`.
5. Resolve API changes without adding compatibility wrappers that hide metadata failures.
6. Run workspace build, all-features tests, auth tests, conformance, and docs checks.

**Acceptance:** clean commit; all mandatory existing gates pass; no new unexplained expected failure.

### Task 1: Add proxy configuration and validation

**Files:** `crates/labby/src/config.rs`, a focused config module if extracted, runtime docs, config tests.

**Tests first:** parse defaults, parse fixed and random ports, reject invalid combinations, preserve comments during mutation, keep secrets absent from serialized TOML.

**Acceptance:** `LabConfig::default()` yields Tailscale, tailnet, and a random external port; all precedence paths are deterministic.

### Task 2: Freeze CLI grammar and command resolver

**Files:** `crates/labby/src/cli.rs`, `crates/labby/src/cli/proxy.rs`, resolver module.

**Tests first:** all parser and resolver cases in the command-resolution section, including non-UTF-8 Unix arguments and Windows path forms.

**Acceptance:** `labby proxy /path/to/dist.js` resolves to Node with no required flag; child flags are unmodified; no shell is used.

### Task 3: Extract reusable stdio process ownership

**Files:** extract from `connect_stdio.rs` into a process module used by both the pool and direct proxy.

**Tests first:** environment scrub, explicit inheritance, stderr drain, clean EOF shutdown, timeout escalation, Unix grandchild reap, Windows Job Object reap.

**Acceptance:** existing pool behavior remains unchanged and direct proxy can own a child without constructing an aggregate upstream configuration.

### Task 4: Spike and implement direct modern forwarding

**Files:** `labby-gateway/direct_proxy` modules and a deterministic fixture.

**Tests first:** complete request and result round-trip for tools, prompts, resources, templates, completion, tasks, MRTR, custom request, custom notification, custom result, metadata, and errors.

Add concurrency tests with duplicate downstream JSON-RPC IDs and progress tokens.

**Acceptance:** the 2026 fixture supports concurrent requests; no metadata or extension field is dropped; cancellation reaches the correct upstream request.

### Task 5: Implement subscription forwarding

**Tests first:** acknowledgment first, accepted subset, two concurrent modern listens, ID rewriting, stream cancellation, child teardown, ref-counted legacy resources, and global list-change filtering.

**Acceptance:** subscription conformance scenarios pass and no notification crosses streams.

### Task 6: Implement legacy interaction compatibility

**Tests first:** Auto fallback only on method-not-found, initialized lifecycle, serialized sampling, roots, and elicitation routing, cancellation while queued, no starvation, old resource subscribe translation.

**Acceptance:** a legacy fixture remains usable through the modern HTTP endpoint without ambiguous server-to-client routing.

### Task 7: Extract and build the proxy HTTP router

**Tests first:** loopback bind, Host allowlist, Origin rejection, required headers, JSON response, SSE progress, SSE cancellation, readiness transitions.

**Acceptance:** the direct service is available at local `/mcp`; aggregate Labby tools are absent; progress and subscriptions stream.

### Task 8: Add bearer policy

**Tests first:** correct token, wrong token, missing token, constant-time helper, challenge header, SSE path, token redaction, and process-output scanning.

**Acceptance:** every protected request requires the dedicated proxy token; no secret appears in captured output or logs.

### Task 9: Add OAuth resource leases and exact-audience auth

**Files:** `labby-auth` lease registry, daemon actions, LiveGateway methods, proxy OAuth policy, metadata routes.

**Tests first:** configured resources survive lease updates; leases survive configured-route refresh; expiry; renewal; release; daemon restart and re-registration; exact port and path audience; wrong issuer, audience, or scope; Protected Resource Metadata challenge.

**Acceptance:** an MCP OAuth client can discover the stable issuer, obtain a token for the random-port resource, connect, and loses authorization when the lease expires.

### Task 10: Implement Tailscale Serve ownership

**Tests first:** fake CLI version and status parsing, random selection, collision retry, ready mapping, process exit, stale mapping cleanup, ownership drift refusal, unrelated mapping preservation, and proof that `reset` is never invoked.

Then run the live versioned spike on devhost.

**Acceptance:** the endpoint is reachable through tailnet HTTPS; Ctrl+C removes only its mapping; forced Labby termination leaves no mapping after the recovery path.

### Task 11: Build supervisor and failure rollback

**Tests first:** failure after each startup stage and each shutdown stage. Use a table-driven failpoint harness.

Mandatory failpoints:

- command resolution;
- child spawn;
- child discovery;
- listener bind;
- bearer resolution;
- OAuth lease create;
- OAuth validator create;
- Tailscale claim;
- Tailscale readiness;
- child runtime exit;
- Serve runtime exit;
- lease renewal failure;
- Ctrl+C with an active request;
- Ctrl+C with an active subscription.

**Acceptance:** every failpoint leaves zero owned child processes, zero test Serve mappings, and zero active OAuth leases.

### Task 12: Add setup and doctor integration

Add `labby setup proxy` with interactive and noninteractive paths.

Checks:

- Tailscale installed, connected, DNS name present, HTTPS capability available;
- selected port or range valid;
- bearer secret present or generated;
- stable OAuth issuer and live daemon available;
- resource lease action supported;
- child runtime launchers present.

**Acceptance:** setup is idempotent; a second run makes no change; generated secret permissions are restrictive.

### Task 13: Documentation and generated CLI inventory

**Completed 2026-08-01.** The operator guide, README quickstart, runtime config,
OAuth, transport, architecture, spec, contract, research outcome, release
notes, and code-owned generated inventories now describe the implemented
surface. Generator drift tests pin top-level `labby proxy`, `setup proxy`,
zero-route and routed `doctor proxy`, all `[proxy]` keys and environment
controls, the CLI-only proxy service entry, generic gateway API/OpenAPI, and
all three OAuth lease actions.

Update:

- CLI help inventory;
- runtime config and environment docs;
- OAuth docs;
- gateway and direct-proxy architecture docs;
- troubleshooting and examples;
- changelog and release notes.

Explicitly document that random-port OAuth creates a distinct resource URL per run and that a fixed port is preferable for long-lived connector configuration.

**Acceptance:** `just docs-check` passes from a clean tree.

### Task 14: Verification harness and proof pack

Add:

```console
cargo run -p xtask -- proxy-verify --binary target/debug/labby
```

Optional live gate:

```console
cargo run -p xtask -- proxy-verify --binary target/debug/labby --live-tailscale --live-oauth
```

The harness produces:

```text
target/proxy-verification/<run-id>/
  manifest.json
  commands.jsonl
  unit-tests.json
  integration-tests.json
  conformance/
  fault-injection.json
  tailscale-before.json
  tailscale-during.json
  tailscale-after.json
  oauth-metadata.json
  oauth-negative-tests.json
  process-tree-before.json
  process-tree-after.json
  redaction-scan.json
  summary.md
```

The manifest records:

- git commit and tree-clean status;
- Rust, Cargo, RMCP, Tailscale, operating system, and conformance versions;
- fixture commits and hashes;
- every command, exit status, duration, and artifact hash;
- mandatory gate results;
- final verdict.

The manifest must never include secrets.

## Test matrix

| Area | Mandatory proof |
| --- | --- |
| CLI | no-flag JS path, child flags, explicit separator, non-UTF-8 arguments |
| Resolver | executable, shebang, JS, Python, PATH command, unknown extension |
| Process | environment scrub, stderr, EOF, escalation, Unix group, Windows Job Object |
| Modern protocol | discover, every core primitive, tasks, MRTR, custom extensions |
| Metadata | version, client info, capabilities, trace keys, log level, unknown keys |
| Correlation | duplicate IDs, duplicate progress tokens, cancellation isolation |
| Subscriptions | acknowledgment, filters, IDs, multiple streams, cancellation, graceful result |
| Legacy | Auto fallback, interaction routing, serialization, resource adaptation |
| HTTP | Host, Origin, required headers, JSON, SSE, disconnect cancellation |
| Bearer | positive, negative, challenge, redaction, SSE |
| OAuth | metadata, issuer, exact audience, scopes, lease create, renew, expire, release |
| Tailscale fake | status parse, port collision, drift, cleanup, unrelated routes |
| Tailscale live | HTTPS reachability, certificate, Ctrl+C cleanup, crash recovery |
| Supervisor | every startup, runtime, and shutdown failpoint |
| Compatibility | Linux and Windows mandatory; macOS compile and test where available |
| Documentation | generated help and config/environment inventory current |
| Security | no LAN bind, no auth downgrade, no shell, no secret leak, no reset |

## Conformance strategy

1. Update the existing pinned RMCP and MCP conformance gate to RMCP 3.1.0.
2. Add a direct-proxy scenario that launches an all-capability stdio fixture behind `labby proxy --local --auth none`.
3. Run the dated server suite against the proxy HTTP endpoint.
4. Run extension task scenarios.
5. Run custom Labby scenarios for metadata preservation, unknown extensions, progress, cancellation, and subscriptions not covered upstream.
6. Keep expected failures empty for the direct proxy unless the upstream suite itself marks a scenario inapplicable.
7. Store raw conformance output in the proof pack.

## Live end-to-end proof

The controlled live test must:

1. snapshot Tailscale Serve status;
2. start a deterministic stdio fixture through the zero-flag configured path;
3. capture the printed URL without exposing a token;
4. verify HTTPS and certificate validity from a second tailnet client when available;
5. run discovery and each advertised primitive;
6. open two subscriptions and two concurrent progress-producing calls;
7. verify bearer or OAuth negative cases;
8. in OAuth mode, obtain a token for the exact resource and reject a token for the same host with the wrong port;
9. terminate with Ctrl+C;
10. verify the child and descendants are gone;
11. verify the exact Serve mapping is gone;
12. verify all pre-existing mappings are byte-for-byte equivalent after normalization;
13. verify the resource lease is released or expired;
14. rerun the same test with forced termination and the recovery cleanup path.

## Required release commands

At minimum, from a clean checkout of the candidate commit:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-features --locked -- -D warnings
cargo build --workspace --all-features --locked
cargo nextest run --workspace --all-features --locked --profile ci
cargo nextest run -p labby --no-default-features --features gateway --locked
cargo test -p labby-auth --all-features --locked
just docs-check
scripts/ci/mcp-conformance.sh
cargo run -p xtask -- proxy-verify --binary target/debug/labby
cargo run -p xtask -- proxy-verify --binary target/debug/labby --live-tailscale --live-oauth
```

Run the complete mandatory sequence twice from clean target directories. Hash both manifests. The second run must produce the same verdict and equivalent normalized protocol results.

## Deliverables

### D0: RMCP 3.1.0 migration

- exact dependency pin and lockfile;
- updated conformance pins;
- migration fixes;
- green existing gates.

### D1: Stable zero-flag CLI

- `labby proxy /path/to/dist.js`;
- command resolver;
- child argument passthrough;
- human and JSON output.

### D2: Persisted proxy defaults

- `[proxy]` schema;
- config validation and precedence;
- setup and doctor flows;
- dedicated bearer secret management.

### D3: Faithful direct MCP bridge

- all core primitives;
- tasks and MRTR;
- custom extensions;
- per-request metadata;
- cancellation and progress;
- modern and legacy lifecycle compatibility.

### D4: Subscription bridge

- modern listen forwarding;
- legacy adaptation;
- ID translation;
- multiple concurrent streams;
- cancellation and cleanup.

### D5: Auth policies

- tailnet;
- static bearer;
- OAuth Protected Resource Metadata and exact audience validation;
- ephemeral daemon resource leases.

### D6: Tailscale Serve ownership

- random and fixed external ports;
- readiness verification;
- collision handling;
- exact cleanup without disturbing other mappings.

### D7: Unified supervisor

- complete startup rollback;
- runtime failure propagation;
- cross-platform process-tree cleanup;
- idempotent shutdown.

### D8: Security and observability

- Host and Origin enforcement;
- no shell execution;
- environment scrub;
- secret redaction;
- structured lifecycle events.

### D9: Documentation

- generated CLI reference;
- config, environment, and OAuth documentation;
- examples and troubleshooting;
- architecture and release notes.

### D10: Proof pack

- deterministic xtask verifier;
- updated MCP conformance gate;
- fault injection;
- live Tailscale and OAuth evidence;
- machine-readable manifest with hashes and verdict.

## Suggested commit sequence

1. `chore(deps): update rmcp to 3.1.0`
2. `feat(proxy): add proxy preferences and CLI grammar`
3. `refactor(gateway): extract reusable stdio process ownership`
4. `feat(proxy): add direct modern MCP bridge`
5. `feat(proxy): bridge subscriptions and legacy interactions`
6. `feat(proxy): serve direct bridge over loopback HTTP`
7. `feat(proxy): add bearer authentication`
8. `feat(auth): add ephemeral OAuth resource leases`
9. `feat(proxy): publish with Tailscale Serve`
10. `feat(proxy): supervise lifecycle and rollback`
11. `feat(setup): configure proxy defaults`
12. `test(proxy): add conformance and proof-pack verification`
13. `docs(proxy): document stdio proxy workflows`

Each commit should be independently reviewable and should leave the relevant focused tests green.

## Merge coordination

- Do not write into the active RMCP or MCP capability worktrees.
- Land or rebase onto the completed RMCP 3.1.0 commit before Task 1.
- Rebase after the capability branch lands if it changes shared relay or handler types.
- Keep new direct-proxy modules separate so aggregate-gateway conflicts are mechanical rather than architectural.
- Run the full proof pack after the final rebase, not only before it.

## Rollback plan

The feature is additive. If release verification fails after merge:

1. hide the command behind the proxy feature gate if one is introduced;
2. preserve the RMCP 3.1.0 upgrade if its independent gates remain green;
3. revert proxy CLI, config, runtime modules, and lease actions as one feature series;
4. remove generated docs entries in the same revert;
5. verify existing `labby serve`, stdio bridge, gateway, auth, and conformance behavior.

No persistent migration is required for ordinary proxy settings. OAuth leases are ephemeral and expire automatically.

## Definition of done

The implementation is complete only when all of the following are true:

- the exact zero-flag command works with configured defaults;
- the child catalog and wire payloads are not altered by aggregate gateway behavior;
- modern requests preserve full per-request metadata;
- all core primitives, tasks, MRTR, custom extensions, progress, cancellation, and subscriptions have positive and negative tests;
- bearer and OAuth protect the endpoint without leaking credentials;
- OAuth tokens are bound to the exact random-port resource;
- Tailscale mapping ownership and cleanup are proven without touching unrelated mappings;
- child descendants are reaped on Linux and Windows;
- every startup and shutdown failpoint leaves no owned residue;
- the pinned conformance suite passes;
- the proof pack passes twice from a clean checkout;
- docs and generated CLI inventories are current;
- the release commit and evidence manifests are recorded in the pull request.
