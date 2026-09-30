---
title: "MCP Task Failure Recovery"
created: "2026-09-30"
updated: "2026-09-30"
---

# MCP Task Failure Recovery

This guide describes the implementation at commit
`138ef902dc1842daf883a6e7710b140fbfaf53c0` (PR #848), not the complete durability
outcome proposed in [issue #771](https://github.com/dinglebear-ai/labby/issues/771).
The [Lane F evidence](../tasks/issue-771-adversarial-docs/README.md) distinguishes
passing gateway integration checks from dependencies that are not implemented.

## The boundary to remember

A durable route is a mapping, not the upstream task itself. Labby commits its
public-to-native task mapping before returning a task acknowledgement. In this
baseline, task RPCs still use the retained upstream relay that created the task.
A replacement downstream client can poll through that relay, but a new pool or
Labby process cannot reconstruct it from the database alone. Do not promise
creator-upstream-peer loss or Labby restart recovery on the strength of a route
row surviving a database reopen.

Modern MCP server observations, live connections, and legacy sessions are
separate concepts. A connection identifier or a server's self-reported name does
not authorize recovery. The existing task lookup checks the trusted owner,
route authorization snapshot, expiry, and configured upstream fingerprint.
Use the original authorized context; never bypass rejection by editing owner,
route, native task ID, or fingerprint columns.

## Responding to failures

The messages below are gateway-runtime classifications. Surface adapters may
wrap them. Keep public unknown/unauthorized responses indistinguishable; internal
diagnostics are not permission to expose a task's existence to another caller.

| Observation | What it establishes | Operator response |
| --- | --- | --- |
| A public task ID was returned | The route commit and live companion installation succeeded at acknowledgement time | Retain the public ID and authorized route context. Poll that ID; do not start the original tool again merely because a notification is missing. |
| `task not found` | No usable route is visible in this context; malformed, unknown, wrong-owner, changed-route, expired, and stale-config paths intentionally share this result | Verify the original caller/route and current approved configuration without probing another caller's tasks. Do not infer nonexistence or modify the DB to force resolution. |
| `upstream task connection unavailable` | In this baseline, a valid visible durable record has no usable retained creator relay | Preserve the record. Treat continuation as unavailable until an authenticated reacquisition implementation is qualified; restarting Labby is not a recovery mechanism for this baseline. |
| `task routing unavailable` | A durable lookup or changed-retention write failed | Inspect storage availability, permissions, capacity, and sanitized errors. A failed poll is not a terminal upstream task state. Do not delete/recreate the DB as an automatic repair. |
| `upstream task registration failed` | Labby withheld task acknowledgement; upstream work may already have started | Do not blindly replay `tools/call`. Reconcile with the upstream operation using its supported administrative or idempotency facilities. Missing acknowledgement is not proof of no side effects. |
| Upstream timeout, protocol error, or temporary unavailability | The task RPC failed, not necessarily the task | Preserve the public ID and route. Use bounded authorized read-only polling when safe, honoring current polling hints. Do not turn a transport error into a fabricated task result. |

If an acknowledgement response was lost after commit, the caller might never
learn the public ID. This slice has no task enumeration or create-request replay
store that recovers that missing receipt. Do not invent `tasks/list` or
`tasks/result`; modern task operations are `tasks/get`, `tasks/update`, and
`tasks/cancel`.

## Notifications are observations, not exactly-once delivery

A task notification contains a complete task-state observation. Duplicate states
can be delivered, and delivery to a disconnected downstream can fail. Neither
case means the original tool should run again. The baseline forwards duplicate
observations rather than offering a durable exactly-once notification ledger.
Do not trigger duplicate downstream mutations or resubmit input merely because
the same observation arrived twice.

The initial queued-notification delivery has a bounded grace period. Once a
route is committed and available, notification delivery failure must not erase
it or convert the committed task into a second create attempt. Polling is the
reconciliation path. A new authorized downstream poll can rebind the retained
relay's notification destination. This is not proof of independent
`subscriptions/listen` reconnection, old-stream fencing, or replay/catch-up after
upstream connection loss. Those are later Lane D dependencies.

Honor updated `pollIntervalMs`. Retention is based on `createdAt + ttlMs`, not
time since the last notification, poll, or connection activity. A null TTL does
not mean that an ephemeral upstream task store becomes persistent. Avoid
assuming an acknowledgement to `tasks/cancel` is terminal-state proof: cancellation
is cooperative, and the upstream's subsequent state resolves a completion race.
Retries of mutating update/cancel operations need their own safe semantics; a
lost response is not permission for blind replay.

## Reload, OAuth, and transport limits

A semantically changed upstream configuration rejects an old route before a
task RPC is dispatched. Current OAuth subject invalidation closes matching live
task companions. It does not yet write the durable revocation/revision contract
proposed by Gate 0. Do not interpret a retained row as permission to resurrect
revoked authority after restart. Same-owner credential refresh, durable revocation,
late in-flight publication, and restart-safe invalidation need B/C qualification.

| Transport situation | Honest baseline guarantee |
| --- | --- |
| Remote HTTP with an independent persistent task service | A live retained upstream relay can serve a new authorized downstream. The route DB alone does not yet enable Labby pool/process restart recovery. |
| Unix HTTP to an independent daemon | Socket reconnection alone does not establish daemon task durability. No cross-restart guarantee is added by this guide. |
| Local or SSH-launched stdio | Reconnect may launch another child with another task store. Require explicit upstream persistence/identity qualification; do not infer durability from transport support. |
| In-process or explicitly legacy upstream | Qualify the actual task owner and lifecycle separately. A retained handle or restored legacy session is not proof of modern durable task routing. |

The Lane F loopback HTTP fixture exercises the real gateway pool API and SQLite
store. It does not launch/restart a production Labby process, exchange OAuth
tokens, qualify other transports, or satisfy issue #771's live Gate 2.

## Diagnostic evidence to retain

Record the software/base commit, transport, whether loss affected the downstream
or upstream connection, the original route's availability, failure classification,
and exact test/reproduction commands. Keep task IDs, raw subjects, native IDs,
bearers, and credentials out of public reports and high-cardinality metric labels.
Store any necessary private diagnostic correlation only in an approved restricted
location. A failing storage or notification check must retain uncertainty rather
than manufacture success.

## Implementation and protocol references

- [Task registration](../../crates/labby-gateway/src/upstream/pool/task_registration.rs): commit ordering, post-await checks, bounded initial notification delivery, and sanitized acknowledgement failure.
- [Task RPC routing](../../crates/labby-gateway/src/upstream/pool/tasks.rs): authorization, expiry/config checks, retained peer, downstream rebinding, and hint-write failure.
- [OAuth invalidation](../../crates/labby-gateway/src/upstream/pool/oauth_invalidation.rs): subject-scoped live connection/task invalidation.
- [Durable route store](../../crates/labby-gateway/src/upstream/pool/task_route_store.rs): storage, admission, and authorized lookup.
- [Relay notification forwarding](../../crates/labby-gateway/src/upstream/pool/relay.rs): complete-state translation and best-effort delivery.
- [MCP Tasks extension](https://tasks.extensions.modelcontextprotocol.io/specification/draft/tasks): task acknowledgement, polling, retention, input, and cooperative cancellation.
- [Upstream service contract](../services/UPSTREAM.md) and [testing policy](../dev/TESTING.md).
