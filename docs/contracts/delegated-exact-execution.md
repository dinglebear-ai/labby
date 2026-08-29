# Delegated Exact Execution Contract

Labby owns authorization and execution for durable agent tool calls. Axon may
orchestrate a turn, but it does not forward an end-user bearer token and it does
not authorize or execute an upstream MCP call itself. This contract invokes the
same live exact-call kernel as Palette and never invokes an LLM.

## HTTP flow

All routes are under `/v1/palette/agent` and require normal Labby API
authentication.

1. The actor calls `POST /delegations` with an `audience` naming the service
   that may act for it. Labby captures the actor's current scopes and returns a
   short-lived, opaque, single-use delegation token.
2. The audience service calls `POST /contexts` with that token, an active
   immutable ExecutionLoadout ID and revision, and an absolute expiry. Labby
   consumes the delegation and returns an opaque execution-context ID.
3. For a destructive exact call, the bound actor calls `POST /approvals`. The
   returned short-lived challenge is bound to actor, service audience, context,
   loadout revision, exact tool ID, canonical argument hash, and contract hash.
4. The service calls `POST /executions` with the context, server-scoped
   idempotency key, exact tool request, absolute deadline, and approval token
   when required.
5. `GET /executions/{id}` recovers the durable receipt. `POST
   /executions/{id}/cancel` requests cancellation through the active exact-call
   future and records a terminal state.

The execution statuses are `running`, `succeeded`, `failed`, `cancelled`,
`timed_out`, and `interrupted`. A process restart converts a previously running
receipt to `interrupted`; callers recover it by status rather than guessing
whether an unrecorded retry is safe.

## Binding and replay rules

- Delegations are audience-bound, expiring, and single-use. Forged, stale,
  reused, or wrong-audience values fail closed.
- Contexts bind the actor, delegated service, captured scopes, active immutable
  loadout revision, and expiry. Every approval and execution revalidates that
  the revision is still effective and contains the exact provider-qualified
  tool and contract hash.
- The first idempotency key use atomically reserves the execution before any
  upstream side effect. Concurrent duplicates observe the same `running`
  receipt and do not dispatch. A terminal exact replay returns the original
  durable receipt and result. Reuse with different actor, service, context,
  loadout, tool, arguments, or contract fails closed.
- Destructive execution consumes its matching approval in the same SQLite
  transaction that reserves the request. Challenges are server-issued,
  expiring, and single-use.
- One absolute caller deadline bounds queueing and execution. Cancellation and
  timeout drop the exact-call future, allowing the existing upstream
  cancellation guards to propagate termination.

## Persistence, audit, and compatibility

`agent-executions.sqlite3` is a WAL database beside Labby's gateway config.
Opaque delegation, context, and approval values are persisted only as SHA-256
digests. Request rows correlate request, receipt, audit, actor, service,
loadout, revision, tool, contract, argument hash, status, and timestamps.
Arguments, authorization values, opaque tokens, credentials, and upstream
results are excluded from audit fields. Results are stored only in the durable
receipt needed for exact replay and status recovery.

The existing `/v1/palette/execute` request and response are unchanged. Both
paths report `executionMode: "exact"` and `llmInvocations: 0`.
---
title: Delegated exact execution
created: 2026-08-29
updated: 2026-08-29
---
