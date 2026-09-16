---
title: "Agent Tasks and Schedules"
created: "2026-09-13"
updated: "2026-09-16"
---

# Agent Tasks and Schedules

The `tasks` service owns durable Agent Task intents, fenced attempts, results,
and recurring schedule definitions. Tasks are distinct from Depot ingestion
jobs. Every Task and schedule belongs to the same `team`, `project`, or
`personal` owner as its Agent; `installation` is not a valid owner.

Task execution uses the shared Assistant LLM provider described in
[Agent Tasks](./AGENT_TASKS.md). A Task or schedule record is not evidence that
a provider is reachable. A queued attempt whose provider is unavailable settles
`failed` through the normal runtime. A due schedule records an admission
failure and does not pretend that a run occurred.

Use the runtime `help` and `schema` actions, or the generated
[action catalog](../generated/action-catalog.md), for the complete action
schemas and required capabilities.

## Durable Task lifecycle

`tasks.create` accepts a caller-provided Task ID and idempotency key, an owner,
an active Agent ID, and the exact raw UTF-8 input. Input must be nonempty and no
larger than 1 MiB; Labby materializes it in the Task-input CAS namespace and
stores the resulting `input_digest`. A supplied `input_digest` is verification
only and must match the supplied bytes.
Labby captures the Agent's current immutable version, content digest, and catalog
generation in the Task intent.

Idempotency is scoped to the owner and binds the complete intent. Replaying the
same Task ID, owner, idempotency key, Agent pin, and input returns the existing
Task. Reusing the identifier for different intent returns the same denial used
for an unauthorized identifier.

`tasks.queue` rechecks the pinned revision and current authority before moving a
Task from `created`, `failed`, `cancelled`, or `expired` to `queued`. A successful
Task cannot be queued again. Each attempt receives a fresh fence and progresses
through `queued` and `running` to one terminal state:

- `succeeded` with an output digest;
- `failed` with a stable error code;
- `cancelled` after an authorized cancellation;
- `expired` when an abandoned lease is recovered.

At most four Tasks execute concurrently for one owner in the current runtime.
Each uses the same five-minute runtime and 16 MiB combined-output bounds as a
direct Agent session. Tasks retain the input needed for execution and their
settlement metadata; they do not expose a session transcript.

`tasks.cancel` moves an admitted Task through `cancelling` and signals its live
process when the attempt is owned by this runtime. `tasks.list` and `tasks.get`
return summaries without output or error details. `tasks.result` returns those
details only after settlement and only to the principal that created the Task;
team administration alone does not grant access to another creator's result.

Agent update, suspension, deletion, membership change, or other authority drift
can block a Task before execution or before result commit. Settlement is fenced
and terminal, so a stale worker cannot overwrite the accepted outcome.

## Create a schedule

`tasks.schedule_create` stores a durable delegation consisting of:

- a unique schedule ID and display name;
- owner and creator identity references;
- an active Agent ID and retained input;
- a cadence and optional initial `armed` state;
- an explicit retry policy.

The schedule stores the Agent ID rather than freezing its revision at schedule
creation. Each occurrence creates a new immutable Task that captures the Agent's
then-current active revision. The schedule service computes the input digest
from the retained input; schedule callers do not supply that digest.

Creation requires both create and operate authority. It captures a nonsecret
reference to the current host-established identity, not its bearer token or
browser session. Each occurrence restores that identity and rechecks the active
principal link, owner membership, Agent definition, and the schedule's fixed
authority ceiling of scoped read, create, and operate.

Schedules are creator-managed. Authorized lists omit input and identity
references. `tasks.schedule_get` returns the retained task template only to the
creator, and edit, arm, pause, run-now, and delete require that same principal in
addition to their action capability.

New schedules are paused unless `armed` is true. Arming computes the next future
occurrence. Pausing prevents future cadence admissions and invalidates pending
claims. `tasks.schedule_run_now` accepts an idempotency key and creates one
pending occurrence even while the cadence is paused. Deleting a schedule removes
its cadence and schedule history; Tasks already admitted keep their independent
lifecycle.

An owner may retain at most 100 schedules. A schedule may have at most 100
pending occurrences, and cleanup retains its 100 most recent completed
occurrence records. Durable Tasks and their results follow the separate Task
ledger lifecycle.

## Supported cadence objects

All timestamps are milliseconds since the Unix epoch.

### Once

```json
{ "kind": "once", "at": 1790000000000 }
```

`at` must be a future nonnegative UTC instant when the schedule is created,
edited, or armed. After the occurrence is enqueued, the cadence disarms.

### Interval

```json
{ "kind": "interval", "every_ms": 3600000 }
```

`every_ms` may range from 60,000 milliseconds through 366 days. The next
occurrence is computed from the scheduler's current time, so downtime does not
produce a replay burst.

### Daily

```json
{ "kind": "daily", "hour": 2, "minute": 0, "timezone": "America/New_York" }
```

`hour` is `0..23`, `minute` is `0..59`, and `timezone` must be an IANA timezone
known to the runtime.

### Weekly

```json
{
  "kind": "weekly",
  "weekdays": [1, 4],
  "hour": 7,
  "minute": 0,
  "timezone": "America/New_York"
}
```

At least one weekday is required. Sunday is `0` and Saturday is `6`.

### Cron

```json
{
  "kind": "cron",
  "expression": "0 3 * * 1-5",
  "timezone": "America/New_York"
}
```

The expression has exactly five numeric fields: minute, hour, day of month,
month, and weekday. Fields support `*`, comma-separated lists, inclusive ranges,
and `/` steps. Names, seconds, `@` macros, and event triggers are not supported.
When both day-of-month and weekday are restricted, matching either field is
sufficient, following cron's OR rule.

Daily, weekly, and cron calculations use IANA timezone rules. Nonexistent local
minutes during a daylight-saving transition are skipped. Repeated local minutes
are distinct UTC occurrences. The search for a next cron occurrence is bounded
to four leap-year cycles; an expression with no match in that window is invalid.

## Missed runs and scheduler recovery

The scheduler checks durable state about every five seconds and claims up to four
ready occurrences per tick. Those values bound polling work; they are not an
exact start-time guarantee.

Missed cadence runs coalesce into one occurrence. The schedule advances directly
to its next future time and does not replay every missed interval. Occurrence and
Task identifiers are deterministic, so duplicate ticks and restart recovery
reuse the same immutable identity. Claims expire after one minute and can then
be recovered by another tick.

Editing or pausing a schedule increments its revision and marks old pending
occurrences and retries as skipped. A claimed occurrence must still match the
current schedule revision and authorization when it enters the Task queue.

## Retry policy

Schedules accept:

```json
{ "max_retries": 2, "backoff_ms": 300000 }
```

`max_retries` ranges from 0 through 10. `backoff_ms` ranges from 60,000 through
86,400,000 milliseconds. The default is no retries; the stored default delay is
five minutes and has no effect while `max_retries` is zero.

Only a terminal `failed` Task with error code `execution_failed` is eligible.
Cancellation, expiration, authority failure, invalid input or definitions,
resource limits, and an unconfigured provider require operator intervention. Each retry
has a distinct immutable Task ID and attempt record, waits the fixed delay after
the scheduler observes failure, and revalidates the original pinned Agent
revision and current authority. Editing, pausing, or deleting the schedule
invalidates delayed retries. Restart recovery preserves their IDs, due times,
and retry limit.

The HTTP and MCP adapters require host-established identity. Browser mutations
also require the session CSRF contract. The local CLI does not expose these
caller-bound operations because it cannot provide the equivalent authenticated
identity binding.
