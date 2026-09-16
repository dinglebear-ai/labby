---
title: "Agents and Agent Tasks"
created: "2026-09-07"
updated: "2026-09-15"
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
and catalog generations. `agents.create` requires the Agent `instructions` (and
accepts an optional `model`); Labby materializes them into its
content-addressed payload store, pins that payload with `content_digest`, and
derives every other revision digest from the payload and the configured
provider, so the required parameters alone are sufficient. Supplied digests are
verification only. `agents.update` accepts new `instructions` or a new `model`
and inherits the rest from the prior revision. `agents.delete` is
`destructive: true` in the shared action metadata: a deleted definition is
filtered out of every read and has no restore action, so MCP elicitation, the
web confirmation, and the `agents` tool's `destructiveHint` all derive from
that one flag. `agents.suspend` is a reversible state change and is not
destructive. Agent payloads, Task inputs, and outputs use
separate CAS namespaces under `agent-payloads/`, while digest values remain the
SHA-256 of the exact stored bytes. Reads re-verify the digest, and a digest from
one namespace never resolves through another namespace. The harness digest
identifies the configured OpenAI-compatible provider endpoint, so a revision
cannot silently move to a different execution provider. Updating an Agent creates the next immutable
revision. Suspension or deletion blocks future runs. Runtime leases are checked
at safe boundaries so membership or policy revocation fences retained execution.
The authority lease issued for `agents.run` and `tasks.queue` covers the full
300 s runtime bound; every other Agent and Task action keeps the short
request-scoped lease.

Product Agent execution uses the same OpenAI-compatible provider abstraction as
the Assistant. Configure `LABBY_PHOENIX_OPENAI_BASE_URL` and, when required,
`LABBY_PHOENIX_OPENAI_API_KEY` in Labby's private environment file. The base URL
must be reachable from the Labby process, not merely from the operator's host; a
loopback URL is therefore valid only for a co-located provider. Embedded URL
credentials, query strings, and fragments are rejected, and the API key is sent
separately as bearer authentication. ExGPT-style providers must expose the
session create, cancel, and close extensions in addition to
`/v1/chat/completions`. Session lifecycle calls use a short bounded timeout,
and a hard Agent runtime timeout invokes executor cleanup so an abandoned chat
does not leave its provider session behind. Direct `agents.run` `input` is an
optional string (absent or `null` means no input; any other JSON type is
rejected as `invalid_param`) and is bound
only to that run; the immutable Agent instructions remain pinned to the selected
revision and travel to the provider as the system turn, while the run input is
sent as a separate user turn, so input can never rewrite the pinned revision
inside a shared prompt. Successful text output is stored content-addressed and returned with
its digest; at most 256 KiB is returned inline, and a longer output is cut at a
character boundary with `output_truncated: true` while `output_digest` remains
the key to the full stored bytes. Each owner scope may have at most four live direct runs; a fifth
concurrent `agents.run` is rejected with `queue_saturated` instead of opening
another provider session. `agents.session.cancel` signals a live run so the
executor stops at its next safe boundary, cancels and closes the provider
session, and settles the session as `cancelled`; a session with no live
in-process owner keeps its durable status and reports `cancel_requested: false`.

Agent Tasks capture an exact Agent revision, normalized input digest, catalog
generation, owner, creator, and authority fingerprint. `tasks.create` requires
raw UTF-8 `input`; Labby materializes it in the Task-input CAS namespace and
stores the resulting `input_digest`. A supplied `input_digest` is verification
only and must match the supplied bytes; a digest without its bytes cannot
create a Task. Task idempotency keys are scoped to the owner and
bind the full immutable intent.
Queue, cancellation, execution, and settlement use fenced state transitions;
terminal settlement is exactly once. A `tasks.cancel` that commits after the
attempt's last cancellation check fences the terminal settlement; the live
attempt then settles the Task `cancelled` itself instead of leaving it in
`cancelling` until lease expiry. A freshly authorized `tasks.queue` may
resume a durable `queued` Task that has no live in-process owner, closing the
crash window between queue commit and scheduler handoff without double-starting
a live attempt. The durable attempt fence is bounded by the configured maximum runtime and is
rechecked against the current clock at acquire and settlement. Provider
or payload failures happen inside the owned queued attempt and settle through
the normal runtime as `failed` instead of bypassing durable queue admission. Terminal `tasks.result` responses
include the immutable output digest plus up to 256 KiB of materialized output
text inline, with `output_truncated` marking a longer output. A recorded digest
whose bytes are missing, oversized, or fail verification is reported as an
error (`unavailable`, `protocol_error`, or `internal_error`), never as a digest
with silently absent text. Current authority is required for every list, get, cancellation, and
result operation.

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
