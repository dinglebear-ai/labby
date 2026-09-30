---
title: "Durable MCP Task Routing and Lifecycle Boundaries"
created: "2026-09-30"
updated: "2026-09-30"
---

# ADR 0007: Durable MCP Task Routing and Lifecycle Boundaries

Status: **Proposed Gate 0 contract; not merged, not an implemented guarantee.**

Issue: [#771](https://github.com/dinglebear-ai/labby/issues/771).
[Baseline and exact evidence](../tasks/issue-771-gate0/README.md).
This change defines lane handoffs, not their implementation. Formal Gate 0 still
requires a clean conformance baseline, ADR acceptance/merge, and resolved ownership.

## Context and authority

Audited product base: b42818f256968466c7d85828aa42c1cd8523efad. Its task routes own
relay connections, use counter IDs, and expire on connection closure or idle time.
The separate, initially dirty B1-B3 worktree proposes opaque IDs, SQLite metadata and
persistence before acknowledgement. It is a prerequisite, not merged behavior or
proof of restart-safe reacquisition. Gate 0 holds the existing RMCP pin; Lane A
owns qualifying official 3.4 and any upgrade.

Protocol authority: [MCP 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28)
and the [Tasks extension](https://tasks.extensions.modelcontextprotocol.io/specification/draft/tasks).
Reuse RMCP models, extension helpers, lifecycle detection and standard routing
headers. Implementation references: [upstream rules](../../crates/labby-gateway/src/upstream/AGENTS.md),
[pool](../../crates/labby-gateway/src/upstream/pool.rs),
[routes](../../crates/labby-gateway/src/upstream/pool/tasks.rs),
[authorization](../../crates/labby-gateway/src/upstream/pool/task_route.rs),
[incarnation guards](../../crates/labby-gateway/src/upstream/pool/incarnation.rs),
[MCP adapters](../../crates/labby/src/mcp/server.rs) and
[conformance](../surfaces/MCP_CONFORMANCE.md).

## D1: Server, connection and legacy session are different objects

| Object | Owns | Does not own |
| --- | --- | --- |
| UpstreamDefinition | Stable configured identity, transport parameters, policy and credential references | Peer handles or self-reported authority |
| ServerSnapshot | Normalized observations scoped by definition and authenticated observation context | Execution permission, credentials or transport keepalive |
| ConnectionLease | Current peer/service/process, health, incarnation and cancellation | Durable task identity or retention |
| SubscriptionLease | Authorized notification stream and bounded delivery state | The task or its durable route |
| LegacySession | Compatibility state for explicitly identified pre-2026 peers | A session model imposed on modern MCP |

### ServerSnapshot shape (C; consumed by A/D/E)

This is a semantic DTO contract, not a new compiled type:

~~~text
ServerSnapshot {
  upstream_name, config_fingerprint,
  observation_scope,                 // opaque trusted auth/route scope
  protocol_era,                      // modern_2026_07_28 | explicit_legacy(version)
  supported_versions, effective_version,
  server_info,                       // optional RMCP display metadata only
  capabilities,                      // RMCP ServerCapabilities, including extensions
  instructions, cache_hints,         // preserve SDK-native metadata when present
  catalog_fingerprint,
  observed_at_unix_ms, freshness_deadline_unix_ms,
  publication_generation             // process-local fencing, not durable authority
}
~~~

Extensions live inside capabilities; UI projections derive them, not a second
mutable copy. Observation expiry means stale cache, not expired task or lost
authorization. Preserve SDK-supported fields at adapter boundaries. Names and
versions are display/debug data, never identity authority. Reuse existing
publication guards and independent catalog-family generations. Include client
capability/protocol context in the observation cache key when discovery varies
with it; that context remains cache metadata, never authorization.

## D2: A TaskRoute is durable metadata

B owns the store, schema/migrations, ID minting, acknowledgement transaction,
retention admission and GC. C obtains a usable peer from the record. Preserve the
prerequisite's UUIDv4 public-ID format; never encode native IDs or use counters.
Random IDs do not replace per-operation authorization.

### Durable schema

| Existing prerequisite v1 fields | Required meaning |
| --- | --- |
| public_task_id | Opaque immutable primary key; collisions never overwrite |
| native_task_id, upstream_name | Immutable native ID and stable configured upstream identity |
| caller_subject, route_key, allowed_upstreams | Trusted creation-owner and route snapshot, not client-provided authority |
| oauth_subject | Credential-owner reference, never a bearer token |
| config_fingerprint | Canonical safe config plus stable auth-reference identity; no secrets or hashes of secret values |
| created_at_unix_ms, updated_at_unix_ms | Checked upstream timestamps; immutable creation identity and nondecreasing observations |
| ttl_ms, poll_interval_ms | Protocol retention/poll hints; null TTL is not an idle-time default |

**B-owned additions requiring coordination:** row_revision for compare-and-swap;
revoked_at_unix_ms for durable invalidation; terminal_observed_at_unix_ms and
gc_after_unix_ms if terminal retention is materialized. Derived expiry need not
be another column. These fields are absent from the inspected v1. Final results
and task input remain upstream; do not create a second task state machine.

Never persist relay keys/peers, process handles, access/refresh tokens, secrets,
synchronous MRTR tokens or self-reported authority. Do not silently append columns
to a versioned v1 store. B supplies a migration and integrity fixtures, or proves
an initial schema was never deployed before changing that initial version.
Unknown/corrupt schemas fail closed without destructive repair. Preserve
owner-only files/sidecars, file-backed transactions and durable commit settings.

### Store API semantics

Names below express the contract, not mandatory renames. The prerequisite's
insert, get_for_caller and update_hints are existing integration points.

~~~text
insert_committed(record, admission) -> committed revision | typed failure
resolve_authorized(public_id, trusted_context, now) -> AuthorizedRoute | NotFound
observe_if_current(route, expected_revision, metadata) -> revision | stale
invalidate_binding(trusted_scope, expected_revision) -> durable invalidation
collect_expired(now, bounded_batch) -> bounded GC outcome
~~~

AuthorizedRoute is an immutable validated record plus revision, not a peer or
permission valid forever. Current route exposure/allowlists and caller,
Team/actor and credential-owner context must still authorize the operation.
Reuse the existing auth identity projection and preserve the prerequisite's
equality-based caller/route snapshot binding. Relaxing that binding needs explicit
B/F security review, not an accidental change during reacquisition. Missing caller subject is not a
wildcard: permit it only for explicitly trusted, stable local/stdio root authority;
remote anonymous contexts must not collapse into one persistent owner.

Creation order: reserve bounded admission; dispatch under trusted binding;
validate returned metadata/retention; commit route; revalidate binding/fences;
make resolution available; only then expose the public ID in response or
notification. Failed persistence prevents successful acknowledgement. The
upstream may already have executed: retain side-effect uncertainty, use only
safe bounded cleanup and never blindly replay creation. A lost downstream
response after commit is not permission to remove a valid durable route.

Expiry is checked creation time plus protocol TTL, not last poll or connection
activity; now >= expiry is expired. Reject already-expired creation and overflow.
Do not evict acknowledged routes to admit new work or because a peer died.
Terminal GC cannot shorten advertised retention. Unlimited TTL requires admission
that can honor it; otherwise refuse new task admission rather than invent an
undisclosed TTL. Admission applies global/per-owner row and byte bounds, including
a maximum serialized record size. B4 owns numerical limits and terminal-retention
policy before rollout.

## D3: Reacquire through existing authority and fence publication

Each get/update/cancel validates the ID, resolves and authorizes the record,
compares current definition/fingerprint, resolves its credential-owner reference,
and acquires a current lease through the normal authenticated pool. Revalidate
after awaits and before dispatch. Invoke the native ID, fence observations and
notifications, and rewrite the public ID. Preserve existing bulkheads,
cancellation, deadlines and typed errors. RMCP emits Mcp-Method/Mcp-Name.

A downstream connection/session ID, stale RelayCacheKey, cached snapshot or old
peer is never reconnection authority. Reload/replacement can replace leases
without deleting valid metadata. Do not hold a DB transaction across network I/O
or retain the creator peer for correctness.

Reuse OAuthLifecycleEpoch, pool revision, connection incarnation and publication
guards for process-local work. Late results cannot republish after invalidation.
Never serialize a process-local epoch and treat it as authority after restart.
Durable revocation is a store transition/revision, so restart cannot resurrect
revoked bindings. Same-owner credential refresh can replace a lease without
changing task ownership. Revocation, owner change or semantically different
configuration invalidates the affected durable binding before success is reported.

Specify the dispatch/invalidation synchronization point: check, await, dispatch
without another guard is insufficient. Reuse the owning guard and check revision
at dispatch and publication/commit. Invalidation cannot undo a remote mutation
already dispatched: report uncertainty, not replay. Mutating retries require
known-safe semantics; read-only polling may reacquire within the original budget.

### Per-transport qualification (C4)

| Transport | Creator-peer loss / Labby restart | Upstream restart |
| --- | --- | --- |
| Independent remote HTTP service | Conditional on the same durable route DB, current authorized credentials, stable configured identity and a connection-independent upstream task store | Requires persistent upstream task state |
| Unix Streamable HTTP to an independent daemon | Same conditions as HTTP; socket reconnection does not prove daemon durability | Requires persistent daemon task state |
| Local stdio or remote stdio launched through SSH | No default guarantee: reconnect may create a different child/task store; qualify persistence and identity first | Requires persistent upstream state and tested restart behavior |
| In-process upstream | Lease replacement requires a surviving task owner; Labby restart requires a durable upstream task implementation | No guarantee from a retained in-process handle |
| Explicit legacy lifecycle | Preserve actual SDK version/Tasks compatibility and test separately | Do not infer modern durability from session recovery |

A persisted mapping cannot make an ephemeral upstream task durable. Do not
acknowledge retention the qualified backend cannot honor. Extension advertisement
is not independent proof of persistence. Reopen tests prove only mapping survival.
Local SQLite solves process restart, not arbitrary multi-node failover. Multiple
instances need the same supported durable routing authority; separate local DBs
are not shared. No network-filesystem SQLite design or sticky creator-process
correctness requirement is introduced.

## D4: Task input is Tasks; synchronous MRTR remains #208

[#208](https://github.com/dinglebear-ai/labby/issues/208) owns generic synchronous
input overlay, requestState storage, trust/schema checks and destructive
confirmation. D can reuse pure validation/policy helpers, not the pending
confirmation map or its continuation lifetime.

After task creation, input_required/inputRequests are upstream task state and
responses use tasks/update. Reauthorize updates, preserve request keys and partial
responses, reject malformed/duplicate keys, and handle unknown/stale keys according
to the pinned Tasks rules. Model-generated input cannot replace trusted user
approval. Do not translate task input to a custom confirm parameter or replay
tools/call to continue an existing task.

SubscriptionLease is disposable. Reconnect notifications/tasks through the
current authorized route/upstream, not the creator relay. Forward complete task
states, fence old-stream events and bound delivery. Polling remains recovery;
there is no exactly-once notification claim. Cancel acknowledgement is cooperative,
not cancelled terminal-state proof; upstream state resolves completion/cancel races.

## D5: Reuse primitives, not a universal task engine

Agent Tasks, MCP Tasks and Code Mode keep separate lifecycles. Reuse proven
storage, owner binding, revisions/fencing, clocks and audit primitives where their
semantics fit. No universal DurableExecution type, new queue/scheduler, duplicate
result store or continuation subsystem is introduced.

## D6: Preserve Code Mode / Microsandbox ownership

[#709](https://github.com/dinglebear-ai/labby/issues/709), observed closed at audit,
retains its Code Mode/Microsandbox scope. Do not reopen/reimplement it here.
Isolation does not replace authentication, approval, quotas, secrets, idempotency,
protocol validation or task ownership. Code Mode's best-effort journal remains
neither a resume store nor a replay store.

## Error and authorization behavior (B/C/D; E presents)

| Internal classification | External contract |
| --- | --- |
| Unknown, malformed, unauthorized, expired, stale/revoked route | Indistinguishable task-not-found path: existing adapter uses invalid_params / -32602, message "task not found", no identifying data |
| Storage unavailable/corrupt or failed creation commit | Sanitized typed infrastructure failure; no success acknowledgement or raw SQL/owner/native-ID leakage |
| Authorized route, temporarily unreachable upstream | Preserve route; truthful upstream/infrastructure error and retry/side-effect semantics, not fabricated expiry |
| Authorized native protocol error | Preserve MCP meaning through normal mapping without private routing context |
| Stale publication/revision or input | No stale commit/dispatch; preserve non-enumeration and pinned input semantics |

Use one authorization/resolution path for get/update/cancel/subscription. Resolve
visibility before route-specific details or upstream I/O. Keep timeout/bulkhead
and cancellation behavior. Bounded internal reason codes may distinguish failures;
metric labels must not include public/native IDs, raw subjects or secrets. Never
turn an RPC failure into a forged upstream task state.

## Cross-lane ownership and outstanding coordination

| Owner | Responsibility and handoff |
| --- | --- |
| A | RMCP pin/config/lifecycle parity and protocol fixtures; qualify fork patches before upgrading |
| B | Schema/store/IDs/ack/TTL/GC; supplies AuthorizedRoute/revision, never ConnectionLease |
| C | ServerSnapshot, authenticated acquisition, reconnect/invalidation/fencing; consumes B records, does not own migrations |
| D | Task input and notifications after B+C; never #208 continuation storage |
| E | Existing operator surfaces from B/C snapshots; no second inventory |
| F | Adversarial fixtures/docs; coordinate shared adapters with implementation owners |
| Gate 0 | ADR and baseline evidence only |

**Report before shared edits:** pool.rs and task registration/dispatch signatures
are B/C integration seams; mcp/server.rs is shared by A/D/E. B owns schema changes,
C owns lifecycle guards. A second lane must coordinate before editing these
files. Gate 0 changes none of them.

Pending: accept this DTO/API contract; assign B's migration/revision and terminal
retention additions; agree C's guard handoff to registration/observation; select
B4 numerical admission policy. These are not resolved by editing the dirty
prerequisite. A's final upgrade/rebase/hold decision follows its compatibility
tests; Gate 0 holds the existing pin. Acceptance/merge is outside this local-only run.

## Non-goals, alternatives and proof

No tasks/list, tasks/result, custom continuation protocol, mandatory Cloudflare
Durable Objects, new MRTR store, universal execution state machine, runtime
simplification, UI implementation, deployment or lane A-F implementation here.
Protected historical documentation is outside scope.

Reject retained-relay correctness, transport-closure GC, names as authority and
undisclosed idle TTL: they violate the boundaries above. A small metadata store
plus the existing authenticated pool is necessary; a second execution framework
is not justified.

Gate 1 must prove peer destruction/reacquisition, Labby restart, owner/route
non-enumeration and stale/expired non-dispatch. Gate 2 adds live input,
notifications, cancel races, TTL/GC, HTTP headers and a secret-free route DB.
Neither this proposal nor the prerequisite establishes those proofs. Missing or
environment-blocked tests remain unverified.
