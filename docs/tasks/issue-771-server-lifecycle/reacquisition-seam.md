---
title: "Issue 771 Lane C: Capture and Reacquisition Handoff"
created: "2026-09-30"
updated: "2026-09-30"
---

# Capture and reacquisition handoff

**Proposal only.** This is not a new production API or a task/session subsystem.
It conforms to the read-only Gate 0 commit recorded in [the audit](README.md),
subject to the explicit clarifications and ownership decisions below.

## C1 / A: preserve discovery at its source

The pinned SDK consumes DiscoverResult into ServerPeerInfo and drops the full
version inventory and cache hints. Capture the SDK-native successful discovery
result at its existing lifecycle boundary, together with the effective version.
Do not invent supported versions from the negotiated version or send a second
unbounded discovery call solely to reconstruct metadata.

A owns the SDK/config/lifecycle integration needed to expose that observation.
C's candidate adapter accepts the full discovery response or an explicitly
identified legacy InitializeResult. Legacy has no discovery cache contract;
absence must not be replaced with a fabricated TTL or supported-version list.
Unknown effective eras fail closed until A qualifies them.

After acceptance, the candidate can move to an owned upstream snapshot module
registered from the existing upstream module root. Attach it to the existing
scoped catalog/publication model under the current publication and independent
catalog-family generation guards. Do not create another server inventory map.
E consumes that projection; D consumes current authorized notification state.
Server-reported names, versions, instructions, extensions, and public cache scope
remain observations, never connection selection or authorization authority.

## B / C2: record and lease handoff

B owns TaskRouteStore, schema/migrations, quota/retention rules, acknowledgement,
and durable route revisions/invalidation. C owns acquisition and process-local
fencing. The current metadata record lacks a durable revision/revocation field;
C must not append one to B's schema or silently reinterpret its current version.

Semantic handoff, not prescribed Rust names:

~~~text
B: resolve_authorized(public_id, trusted_context, now)
   -> immutable AuthorizedRoute(record, durable_revision)
C: acquire_current_peer(AuthorizedRoute, current_definition, trusted_context,
                        original_deadline, cancellation)
   -> existing-pool-owned ephemeral peer/lease plus current observation fences
B+C: submit_if_current(route_revision, binding, lease/fences, native_task_rpc)
B+C: publish_or_observe_if_current(route_revision, binding, lease/fences, result)
~~~

Do not serialize the peer, RelayCacheKey, client/session ID, process handle,
OAuth epoch, bearer, or raw snapshot metadata. Preserve the public/native task
ID boundary and let RMCP own method/name headers on HTTP carriers.

## Dispatch and invalidation linearization

1. Resolve authorization/exposure and expiry before route-specific I/O. Unknown,
   unauthorized, stale, revoked and expired routes use the existing
   indistinguishable task-not-found mapping. Reuse caller/route/credential-owner
   identity projection; do not derive authority from discovery metadata.
2. Obtain an immutable authorized record/revision; end the DB transaction before
   network work. Compare current config/auth reference identity and take the
   existing process-local OAuth/pool/incarnation observation before acquisition.
3. Reacquire through the normal subject-aware pool with its single-flight,
   cancellation, bulkhead, deadline, SSRF/spawn and publication controls. A
   connection failure preserves an otherwise valid durable route and its TTL.
4. After every acquisition/rebinding await, revalidate under the owning current
   guards. A check followed by another unchecked await is not sufficient.
5. The guarded dispatch boundary must include actual local request submission,
   not merely construction of an unpolled future. Release the short guard once
   enqueue/handoff is established, before waiting for the remote response.
   A must identify the exact pinned RMCP submission primitive; C/B must agree
   its ordering with durable invalidation. This is an unresolved shared seam,
   not a claim the current task methods already do it.
6. After the response, revalidate the current binding, incarnation/publication
   generation and durable revision before updating retention hints, publishing
   snapshots, rewriting/forwarding notifications, or exposing success. B's
   compare-and-swap must refuse observations after invalidation.
7. Revocation is durable before the invalidation operation reports success.
   Process-local epoch checks alone cannot prevent resurrection after restart.
   A request already dispatched cannot be undone: preserve side-effect
   uncertainty and do not replay mutating calls blindly. Read-only polling may
   reacquire only within its original retry/deadline budget.

Use the existing invocation, OAuth, connection/catalog and publication guard
order, not a second lock hierarchy. The final dispatch guard/DB transition
ordering needs owner review and a deterministic race fixture before integration.
Do not hold a DB transaction or a global lifecycle writer over remote response
I/O. Cross-process/multi-node dispatch linearization is not established by these
local barriers or by sharing a SQLite filename.

## Required event outcomes

| Event | Route/authority decision | Connection/observation decision |
| --- | --- | --- |
| Transport death | Preserve valid metadata and protocol retention; qualify backend continuity | Drop dead lease; reacquire through current pool; stale peer cannot publish |
| Pool reload, same binding | Do not delete valid routes merely because the pool object changes | New pool/incarnation; reuse existing coordination and reject old publication |
| Same-owner credential refresh | Preserve task ownership only after current credentials and durable binding reauthorize | Old epoch/lease cannot publish; fresh authenticated acquisition required |
| Credential revocation or owner change | B-owned durable invalidation; indistinguishable not-found | Reject stale submission and late publication; shutdown is cleanup, not authority |
| Semantically changed or removed config | Current safe fingerprint/auth-reference mismatch fails closed | No reconnect under the old definition, even if display metadata is identical |
| Observation/cache expiry | Does not expire the task or grant fresh authority | Mark stale/reobserve through bounded discovery; never fabricate missing hints |
| Upstream/child restart | No default continuation promise | Apply the transport matrix; require proven persistent task identity/store |

## Explicitly blocked edits and proof gate

| Owner(s) | Decision required before integration |
| --- | --- |
| Gate 0 | Accept the D1-D6 contract and C1 absence semantics; retain observation-only identity |
| A + C1 | Expose complete SDK discovery metadata without duplicate discovery or a parallel lifecycle implementation |
| B + C2 | Versioned durable revision/revocation semantics and migration/integrity fixtures |
| B + C3 | Distinguish same-owner refresh from revocation; agree durable invalidation and guarded submission order |
| C + existing pool owners | Wire the snapshot/current acquisition into pool.rs, catalog state, and task RPCs without disrupting concurrent lanes |
| B4 + C4 | Admission/retention promises must match qualified backend continuity; no silent idle-time shortening |

No edits to pool.rs, pool/tasks.rs, B's record/store/schema, SDK pin, or the Gate 0
ADR are included. The existing in-memory task companion is not removed here.

Before calling C2 complete, prove a task created through a real modern server
survives creator destruction and Labby restart, then test wrong owner/route,
config change, revocation during acquire/submit/response, same-owner refresh,
expiry, and stale notification publication. Include exact HTTP routing headers
for HTTP and Unix HTTP, backend retention qualification for each supported
transport, and deterministic race ordering without sleeps. New unit/model tests
and a reopened route DB alone cannot satisfy that gate.
