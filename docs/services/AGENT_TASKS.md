---
title: "Agents and Agent Tasks"
created: "2026-09-07"
updated: "2026-09-13"
---

# Agents and Agent Tasks

Labby owns durable Agent definitions and Agent Task lifecycle. These resources
are separate from Depot artifacts and ingestion jobs.

Every definition and task has exactly one `team`, `project`, or `personal`
owner. The authenticated principal is resolved by the host; owner
and principal parameters never establish authority. List operations omit
resources the caller cannot read, while direct reads and mutations return the
same non-enumerating denial for absent and unauthorized identifiers.

Agent revisions pin content, repository, image, harness, loadout, credentials,
and catalog generations. Updating an Agent creates the next immutable revision.
Suspension or deletion blocks future runs. Runtime leases are checked at safe
boundaries so membership or policy revocation fences retained execution.

Agent execution uses an operator-approved harness from `agents.harnesses` in
`config.toml`. Each harness fixes an absolute executable, literal arguments,
an optional absolute working directory, and the names of environment variables
that may be inherited. An Agent revision pins the digest returned by
`agents.harnesses`; the descriptor also binds the exact content, repository,
image, loadout, and catalog pins provisioned by the operator. Labby compares
every pin before spawning and does not accept a command from an Agent definition
or run request. With no exact configured match, admission fails explicitly.

`agents.run` accepts the initial input and returns an admitted session. The
session actions list retained runs, read status and bounded stdout transcripts,
stop a live process tree, and resume a terminal session by starting a new,
reauthorized run from the retained input. Input and transcript evidence are
limited to 1 MiB. Session metadata exposes content digests and stable error
codes without returning retained input in list or metadata responses. The
owner-authorized transcript action returns the initial input with the harness
stdout so the session drawer can reconstruct the exchange.

Agent Tasks capture an exact Agent revision, normalized input digest and input,
catalog generation, owner, creator, and authority fingerprint. Task idempotency keys are
scoped to the owner and bind the full immutable intent. Queue, cancellation,
execution, and settlement use fenced state transitions; terminal settlement is
exactly once. Current authority is required for every list, get, cancellation,
and result operation.

Authenticated HTTP exposes `POST /v1/agents` and `POST /v1/tasks` with the same
`action` plus `params` envelope used by MCP. MCP exposes the `agents` and
`tasks` caller-bound services only when the transport supplies a verified
identity. Context-free invocation fails closed. Local CLI invocation is not
offered because it has no equivalent authenticated identity binding.

How a caller selects the `owner_kind` and `owner_id` context on each surface is
listed in [Selecting the authority context](../access-control/MULTI_USER_AUTHORITY.md#selecting-the-authority-context).

## Durable schedules

The `tasks.schedule_*` actions create, list, inspect, edit, arm, pause, delete,
and immediately run a schedule. A schedule names an existing active Agent and
retains its input (at most 1 MiB); each occurrence captures the Agent's current
immutable revision when creating its Task. Schedule inspection and mutation
are creator-only; list summaries omit input and identity references.

Supported `schedule` objects are:

- `{"kind":"once","at":1790000000000}`: one UTC epoch-millisecond instant.
- `{"kind":"interval","every_ms":3600000}`: intervals from one minute to 366 days.
- `{"kind":"daily","hour":2,"minute":0,"timezone":"America/New_York"}`.
- `{"kind":"weekly","weekdays":[1,4],"hour":7,"minute":0,"timezone":"America/New_York"}`:
  weekdays use Sunday `0` through Saturday `6`.
- `{"kind":"cron","expression":"0 3 * * *","timezone":"America/New_York"}`:
  five numeric fields support wildcards, lists, ranges, and steps. Weekday is
  `0..6`; names, seconds, macros, and event triggers such as “on push” are not
  cron schedules. Restricted day-of-month and weekday fields use cron's OR rule.

Clock schedules use IANA timezone rules. Nonexistent DST wall times are skipped;
repeated wall minutes are distinct UTC occurrences. The next matching time must
fall within four years. Missed runs coalesce into one pending occurrence and
advance directly to the next future time; the scheduler never replays an entire
missed backlog. Arming starts with the next future occurrence. “Run now” requires
an idempotency key and also works while the cadence is paused.

Creating a schedule explicitly delegates future task creation and operation.
This delegation outlives the creating bearer or browser session; it stores only
nonsecret principal-link facts, never bearer credentials. Every admission
rechecks the active principal, identity link, organization, current membership,
and Agent definition. Its authority ceiling permits only scoped read, create,
and operate. Revocation or pausing prevents new admissions; an already queued
Task retains its independent cancellation and authority-check lifecycle.

The access database atomically records a deterministic occurrence/task identity,
advances cadence, and leases pending work. Restart recovery uses the same Task
identity; task queue admission validates the schedule claim in the same
transaction as its fenced transition. Per owner there are at most 100 schedules;
per schedule there are at most 100 pending occurrences and 100 retained recent
completed schedule records. Durable Tasks and their results have their own
retention lifecycle. Deleting a schedule removes its schedule records, not Tasks
already admitted.

### Scheduled execution retries

Schedules accept an explicit `retry_policy` with `max_retries` from 0 to 10 and
`backoff_ms` from 60000 to 86400000. Retries are disabled by default. The Tasks
form offers two retries with a five-minute delay only after the operator enables
retries. Each retry has a distinct immutable Task ID and durable attempt record.
The fixed delay starts when the scheduler observes the terminal execution failure.
Only `execution_failed` is eligible; cancellation, expiration, authority failures,
missing executors, invalid input or definitions, and resource limits require
intervention. Each attempt validates the original pinned definition, current
delegation, and a fresh authority lease. Pausing, editing, or deleting a schedule
invalidates pending claims, including delayed retries. Restart recovery preserves
attempt identity and the retry limit; it never requeues a settled Task.
