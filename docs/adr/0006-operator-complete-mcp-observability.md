---
title: "ADR 0006: Adopt Operator-Complete MCP Observability and Network Attribution"
created: "2026-09-24"
updated: "2026-09-24"
---

# ADR 0006: Adopt Operator-Complete MCP Observability and Network Attribution

Date: 2026-09-24

Status: Accepted

## Context

Labby is an MCP gateway and operator control plane. It terminates inbound MCP
traffic, authenticates callers, dispatches Labby-owned operations, proxies
upstream MCP capabilities, manages upstream processes and connections, and
already records structured traces plus durable upstream usage telemetry.

The existing observability model has useful but disconnected pieces:

- 'gateway.clients.list' stores bounded observations from MCP discovery:
  redacted actor tag, self-declared client name/version, transport, and
  observation time.
- 'UsageAttribution' can associate proxied upstream calls with an inbound
  actor, surface, client name/version, or trusted Labby agent/task/harness.
- 'UsageStore' durably records upstream capability/operation, outcome,
  latency, response size, actor attribution, and related dimensions.
- structured traces correlate dispatch and nested upstream work.
- 'UpstreamPool' owns real shared, OAuth-subject-scoped, and relay MCP
  connections, but does not expose a complete physical connection inventory.
- the HTTP listener already carries Axum 'ConnectInfo<SocketAddr>', and
  Labby's authorization paths already use the direct socket peer for rate
  limiting.
- Labby-issued access tokens contain an 'azp' authorized-party claim set
  from the OAuth client ID, but the current 'AuthContext' discards it after
  token validation.

Those pieces do not currently answer the operator's basic questions with
sufficient precision:

- Which clients are present now?
- Which observations are genuinely live connections and which are recent
  stateless HTTP activity?
- Which authenticated principal and OAuth client application produced a
  request?
- What network peer and proxy path did it arrive through?
- What is the client doing right now?
- Which upstreams, capabilities, prompts, resources, tasks, or tools has it
  touched?
- Which physical upstream connections exist, and which logical callers are
  using them?
- What exact plaintext request/response data passed through Labby when deep
  forensics is required?

The current UI also calls the append-only discovery observation list
"Connected clients" and "live sessions", even though the backend explicitly
documents that it is not a strict liveness view.

MCP 2026-07-28 makes this distinction necessary rather than cosmetic. The
protocol is stateless: requests carry their own negotiated protocol metadata,
HTTP messages are independent requests, and an open stdio process or transport
connection is not a task, thread, conversation, or application session.
Streamable HTTP no longer defines 'Mcp-Session-Id'. The protocol also has no
general heartbeat or 'ping' liveness primitive in 2026-07-28.
'subscriptions/listen' is a long-lived request whose stream lifetime can be
observed, but that stream is not a universal client session.

MCP's 'clientInfo' is useful for telemetry and debugging but is self-reported
and optional on modern requests. It must not be treated as authenticated
identity. Client capabilities and protocol version are request-scoped protocol
facts, not proof of a person or device.

Labby previously chose not to retain raw source IP as a Usage or Traces
dimension. This ADR supersedes that policy. For a privately operated gateway,
network provenance and inspectable plaintext are operational evidence that the
operator has explicitly chosen to retain.

## Decision

Labby will adopt an operator-complete observability model built around three
separate concepts: **Presence**, **Activity**, and **Connections**.

Labby will also retain network provenance, including IP addresses, and will
provide a protected forensic capture plane for plaintext traffic that Labby can
inspect.

The observability system must preserve the provenance and confidence of each
field. Verified identity, direct transport observations, self-reported MCP
metadata, and heuristic correlation must never be silently collapsed into one
undifferentiated "client identity".

### Presence: who is here

Presence is Labby's logical view of a client or caller, not a synonym for an MCP
transport connection.

A presence record should include, when observable:

- server-generated 'presence_id'
- first-seen and last-seen timestamps
- current state and state reason
- active request count
- transport kind
- route / virtual-server scope
- authenticated issuer and principal identity
- stable Labby actor key
- verified OAuth client application / authorized-party identity when available
- full bounded MCP 'clientInfo' implementation metadata
- protocol version
- client capability summary or canonical digest
- direct socket peer address
- effective client address and provenance
- trusted forwarding-chain evidence
- bounded User-Agent / Origin / Host metadata where applicable
- last observed MCP method/capability/operation
- recent upstream set and current in-flight upstream set
- correlation/fingerprint fields described below

Presence states must communicate what Labby actually knows:

- 'connected': Labby owns a live transport/request-stream lifecycle whose
  close is observable.
- 'active': one or more requests are currently executing.
- 'active_recently': stateless request activity was observed within the
  configured presence window, but no durable transport lifetime is proven.
- 'idle': a proven-live connection remains open with no active request.
- 'expired': a stateless observation exceeded its idle window.
- 'disconnected': an observable connection or subscription closed.

A stateless HTTP request must not be represented as a durable MCP session.
Each accepted request updates 'last_seen'. Recent stateless observations are
removed or transitioned to 'expired' after a bounded idle TTL.

For modern 'subscriptions/listen', Labby will attach presence to the
existing server-owned peer 'registration_id'. The existing handler already
has exact registration, cancellation, removal, 'peer.connect', and
'peer.disconnect' lifecycle points; presence must reuse that lifecycle
rather than maintain a second index-paired peer vector.

Stdio and other durable transports may provide a physical connection lifetime,
but that lifetime must not be labeled a conversation, thread, task, or logical
session unless a higher-level trusted contract provides such an identity.

### Heartbeat and liveness

Labby will not invent an MCP heartbeat message that the 2026-07-28 protocol
does not define.

Instead, liveness is derived from evidence:

- request arrival and completion update activity and 'last_seen'
- in-flight request guards prove temporary activity
- an open 'subscriptions/listen' stream proves that subscription request is
  still live
- transport close/cancellation proves that observable lifetime ended
- stdio child/process lifecycle contributes physical-connection evidence
- HTTP/SSE transport keepalive traffic may preserve a network stream, but does
  not by itself prove meaningful client activity
- stateless HTTP presence expires after an idle window and is never promoted to
  'connected' merely because requests share metadata

The UI must show the evidence class and age rather than implying stronger
liveness than Labby can prove.

### Activity: what clients are doing

Every accepted inbound request will have a server-generated observation /
activity identifier that can be joined to existing correlation identifiers
such as JSON-RPC request ID, 'request_id', 'trace_id',
'execution_id', task IDs, and W3C trace context when present.

A bounded in-memory active-operation registry will track currently executing
work. RAII-style guards should ensure completion, error, timeout, and
cancellation remove or finalize the active row even when execution exits early.

The active record should include, when applicable:

- presence / client fingerprint reference
- inbound surface and route
- MCP method and target name
- Labby service/action
- upstream name
- capability and operation
- published route and native upstream identity
- JSON-RPC request ID
- trace/request/execution IDs
- task / agent / harness IDs when trusted Labby execution supplies them
- start time and elapsed time
- queued/in-flight state
- cancellation state
- request/response byte counts and hashes
- final outcome and error classification

Durable history continues to use the existing UsageStore and retained trace
pipeline rather than introducing a second general analytics database. Presence
and active-operation state are process-local live state; durable call history is
stored by the existing telemetry authority.

### Connections: what physical links exist

Physical connections are distinct from logical client presence and from
routability.

Inbound connection views will report the transport facts Labby can actually
observe.

Outbound views will project safe snapshots from the existing upstream pool,
including:

- ordinary shared upstream connections
- OAuth subject-scoped connections
- request-bound relay connections
- transport/runtime kind
- process ID where applicable
- creation / last-used time when known
- in-flight request counts
- reconnect/recovery state
- subscription/listener state
- task-route ownership where safe
- pool revision / incarnation information useful for diagnostics

OAuth credentials, bearer tokens, and raw provider subjects must not be used as
ordinary connection labels. Stable internal/redacted identifiers may be used
for correlation.

The existing gateway 'connected' field, where it means routable/healthy
rather than "a physical connection exists", must be renamed or augmented so the
operator can distinguish 'routable' from physical connection state.

### Identity and evidence classes

Every client-related field must retain an evidence class.

#### Protocol-guaranteed or request-scoped facts

Examples include:

- negotiated / supplied MCP protocol version
- client capabilities
- JSON-RPC request ID
- method and target name
- MCP request metadata
- subscription stream lifetime
- W3C trace context when supplied

These describe a request and its advertised capabilities, not authenticated
human/device identity.

#### Self-reported client metadata

The full bounded MCP 'clientInfo' 'Implementation' object should be
captured when supplied, not only name/version. Depending on the SDK/schema this
can include name, version, title, description, website URL, and icons.

This is useful forensic evidence but remains self-reported and unverified.

#### Verified Labby identity

When authentication supplies them, Labby should retain suitable operator
observability fields from verified auth state:

- token/session issuer
- authenticated principal / subject
- stable actor key
- effective scopes
- authentication mechanism / session-vs-bearer provenance
- verified OAuth authorized party / client application identity when the token
  format provides it
- safe token/grant diagnostic identifiers that do not require exposing the
  bearer secret itself

For Labby-issued OAuth access tokens, the already-validated 'azp' claim is a
verified client-application signal and should no longer be discarded before
observability attribution.

Raw bearer values are forensic payload data, not semantic identity fields.

#### Trusted Labby execution identity

'agent_id', 'task_id', and 'harness_id' are trustworthy only when
they originate from Labby-owned execution state, such as the task dispatcher.
They are not core MCP client identity fields and must not be fabricated for an
external ChatGPT, Claude, Codex, IDE, or other connector that did not provide a
trusted Labby execution identity.

#### Transport and network observations

Labby will retain:

- direct socket peer IP and port
- transport type
- listener / route
- effective client IP when resolvable through an explicitly trusted proxy
- forwarding chain and the header/protocol source used to derive it
- relevant bounded HTTP connection metadata such as User-Agent, Origin, Host,
  and protocol routing headers

A direct socket peer is observed fact. A forwarded address is asserted by a
proxy and becomes trusted only when the immediate peer is within Labby's
explicit trusted-proxy policy and that proxy is configured to overwrite
client-supplied forwarding headers.

### Network attribution and IP logging

The previous policy that raw source IP is not a Usage or Traces metric is
superseded.

For inbound HTTP traffic Labby will record the direct socket peer address.
Where deployments use SWAG/nginx, Caddy, Tailscale, Cloudflare, or another
proxy, Labby will introduce an explicit trusted-proxy policy before promoting
forwarded values to effective client identity.

The model must preserve at least:

- 'socket_peer_ip'
- 'socket_peer_port' when useful
- 'forwarded_for' or equivalent bounded forwarding evidence
- 'effective_client_ip'
- 'client_ip_source'
- trusted-proxy hop / policy information sufficient to explain the derivation

Existing '[api].trust_forwarded_headers', which currently governs forwarded
host authority, must not silently grow unrelated client-IP trust semantics.
Client-address trust requires an explicit configuration contract.

Untrusted forwarding headers may be retained as untrusted forensic evidence,
but must never override the observed socket peer or be treated as verified
client identity.

### Client correlation and fingerprinting

MCP 2026-07-28 does not supply a universal durable client, device, conversation,
or session identifier. Labby therefore will not claim that one exists.

Labby may derive a versioned 'client_fingerprint_v1' for correlation. It is
a server-keyed HMAC over a canonical evidence tuple chosen from available
signals, for example:

- verified issuer + principal
- verified OAuth client application / 'azp', when available
- normalized full 'clientInfo'
- canonical client capability digest
- transport
- bounded User-Agent
- network provenance, especially for anonymous callers

The raw component evidence remains independently queryable so an operator can
understand why two observations were grouped.

Fingerprint confidence must be explicit, for example:

- 'verified_app_principal'
- 'verified_principal'
- 'network_attributed'
- 'self_reported'
- 'heuristic'

A fingerprint is a correlation aid only. It must never grant authorization,
select credentials, widen scopes, or replace verified identity.

IP address alone is not a durable device identity. NAT, proxies, VPNs, mobile
networks, and address churn can merge or split real clients. Conversely,
multiple independent connectors can truthfully publish identical
'clientInfo'. The operator UI must expose these limitations.

### Observe all inspectable plaintext

Labby's operator policy is that protocol data visible to Labby is observable.

If Labby can parse or proxy a plaintext request, response, notification,
elicitation, sampling exchange, task update, resource read, prompt, tool call,
header, or metadata object, the observability architecture must provide a way
for an authorized operator to inspect it.

If payload data remains encrypted end-to-end beyond Labby's visibility, Labby
records only what it can observe, such as ciphertext size, timing, route, and
digest. The system must never pretend it decrypted data it could not inspect.

This policy uses two storage planes.

#### Semantic telemetry

Normal structured logs, Presence, Activity, Connections, Usage, and Traces
remain queryable and efficient. They store strongly typed metadata, network
provenance, identity provenance, method/target names, sizes, hashes,
correlation IDs, outcomes, and bounded values needed for routine operations.

Source IP is allowed and expected here.

Semantic telemetry must still avoid copying bearer tokens, cookies, OAuth
authorization codes, private keys, raw credential material, and unbounded raw
payloads into every ordinary log event. Repeating those values through
high-cardinality logs would make every log sink a credential database without
adding observability value.

#### Protected forensic capture

Labby will provide a first-class protected forensic journal capable of retaining
the raw inspectable request/response envelopes and headers, including
secret-bearing plaintext when the operator enables complete capture.

Because this plane may contain credentials and arbitrary private content, it
must have stronger controls than ordinary logs:

- encryption at rest
- owner/admin-only access
- restrictive filesystem permissions
- bounded record and total-size budgets
- configurable retention and pruning
- explicit audit events for reads/exports where practical
- no accidental inclusion in ordinary support bundles or unprivileged APIs
- provenance linking each capture to its semantic trace/activity identifiers
- clear UI labeling that raw capture may contain credentials and private data

The operator must be able to disable or narrow raw capture, but doing so is an
operator policy choice rather than a protocol limitation. The architecture
must not require a code change to turn complete capture on.

### Retention and queryability

Live Presence and active Activity state are bounded in memory.

Durable semantic telemetry should continue to use the existing bounded storage
and retention mechanisms where possible. New dimensions such as network
provenance and application identity require schema migration rather than a
parallel analytics store.

Raw forensic capture has an independently configurable, bounded retention
policy because its sensitivity and byte volume differ materially from semantic
telemetry.

All operator-facing client, activity, connection, usage, trace, and forensic
surfaces are admin-gated and route-scope aware where a route-scoped operator
view exists.

### Required operator surfaces

The web UI, shared gateway action API, CLI, and MCP admin surface should expose
the same server-truth model rather than independently reconstructing it.

The operator experience should support:

- live Presence list
- live in-flight Activity
- physical inbound/outbound Connections
- client -> Labby -> upstream topology
- drill-down from a client to recent calls
- drill-down from an upstream to current/recent clients
- links from Activity/Usage to retained Traces
- raw forensic inspection when authorized and capture is enabled
- explicit labels for verified, observed, self-reported, and heuristic fields

The existing "Connected clients / live sessions" UI must not keep that wording
until its data actually satisfies the liveness semantics defined here.

## Consequences

- Labby gains a truthful answer to "who is using the gateway, what are they
  doing, and where is the traffic going?"
- Source and effective client IP become supported observability dimensions.
- Authenticated principal identity and OAuth client-application identity can be
  separated when the token format provides both.
- Stateless HTTP observations are not mislabeled as durable sessions.
- 'subscriptions/listen' and other observable lifetimes can provide exact
  connect/disconnect state without a duplicate peer registry.
- Client fingerprinting becomes evidence-based and confidence-labeled rather
  than pretending MCP has a device/session ID.
- Upstream routability and physical connection state become separate operator
  concepts.
- Existing UsageStore and trace infrastructure remain useful and are extended
  instead of replaced.
- Full raw capture materially increases the sensitivity of Labby's retained
  data. Encryption, access control, pruning, and export discipline become part
  of the implementation requirement, not optional hardening.
- Operators deploying behind a reverse proxy need an explicit trusted-proxy
  policy before forwarded addresses can be treated as effective client IP.
- The current observability implementation is incomplete relative to this
  decision and requires staged schema, runtime, and UI work.

## Alternatives considered

### Keep the current discovery observation list

Rejected. It cannot answer liveness, disconnect, last-seen, in-flight activity,
or client-to-upstream usage, and the current UI language overstates its
semantics.

### Treat every HTTP request as a unique MCP session

Rejected. MCP 2026-07-28 is stateless and Streamable HTTP does not define such
a session identity.

### Reuse 'relay_session_id' as the client/session identifier

Rejected. Labby intentionally creates a fresh server handler and relay ID per
stateless HTTP POST. It is an internal relay isolation key, not a protocol or
client identity.

### Use 'clientInfo' as the client identity

Rejected. MCP explicitly treats implementation metadata as self-reported and
unsuitable for security identity.

### Use IP address as the client identity

Rejected as an identity primitive, but accepted as network provenance and one
possible heuristic correlation component. NAT, proxies, VPNs, and address churn
make IP alone insufficient.

### Keep source IP out of telemetry

Rejected. The operator has explicitly chosen network provenance as part of the
gateway's observability contract. The design instead distinguishes direct
socket fact, trusted-proxy-derived effective IP, and untrusted forwarded claims.

### Put complete plaintext payloads into ordinary structured logs

Rejected. It duplicates arbitrary payloads and credentials into every log sink,
creates unbounded cardinality/volume, and weakens access control. Complete
payload visibility belongs in a dedicated protected forensic journal linked to
normal semantic traces.

### Invent an MCP heartbeat extension

Rejected. Liveness can be derived from requests, subscription lifetime,
transport lifecycle, and bounded expiry without creating a Labby-private
protocol dependency.

## Authority and implementation status

This ADR is an accepted product/operator decision.

It supersedes the previous statement in 'docs/dev/OBSERVABILITY.md' that raw
source IP should not be a Usage or Traces metric. It does not retroactively make
untrusted forwarding headers authoritative.

As of this ADR's acceptance, the implementation is partial:

- direct HTTP socket peer information already exists in request extensions and
  is used by authentication rate limiting
- modern MCP discovery already captures bounded client name/version
- 'subscriptions/listen' already has explicit peer registration and
  disconnect lifecycle
- UsageAttribution and UsageStore already carry substantial request/upstream
  attribution
- Labby-issued access tokens already contain 'azp', but AuthContext does not
  yet preserve it
- the client registry is still append-only/best-effort
- modern request attribution still uses 'Peer::peer_info()' in at least one
  path and should move to request-scoped client metadata for stateless MCP
- trusted forwarded client-IP resolution is not yet implemented
- the physical upstream connection inventory is not yet a first-class public
  view
- the protected raw forensic journal is not yet implemented
- UI wording still overstates the existing client registry's liveness

Implementation work must preserve this distinction between accepted target
architecture and currently shipped behavior.

## References

- [MCP 2026-07-28 specification](https://modelcontextprotocol.io/specification/2026-07-28)
- [MCP 2026-07-28 introduction](https://modelcontextprotocol.io/docs/2026-07-28/getting-started/intro)
- [MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Rust SDK issue #1172: stateless peer_info reports synthetic client info](https://github.com/modelcontextprotocol/rust-sdk/issues/1172)
- [Rust SDK PR #1173: stateless initialize peer_info handling](https://github.com/modelcontextprotocol/rust-sdk/pull/1173)
- 'crates/labby-runtime/src/client_registry.rs'
- 'crates/labby-runtime/src/usage_actor.rs'
- 'crates/labby/src/mcp/context.rs'
- 'crates/labby/src/mcp/server.rs'
- 'crates/labby/src/mcp/peers.rs'
- 'crates/labby/src/cli/serve.rs'
- 'crates/labby-auth/src/auth_context.rs'
- 'crates/labby-auth/src/jwt.rs'
- 'crates/labby-auth/src/middleware.rs'
- 'crates/labby-gateway/src/upstream/pool.rs'
- 'docs/dev/OBSERVABILITY.md'
- 'docs/runtime/REVERSE_PROXY.md'
- evidence baseline: 'origin/main' at '24b918b65a9fd64cc2c7d9c3c9a7cee7d3ca77bf'
