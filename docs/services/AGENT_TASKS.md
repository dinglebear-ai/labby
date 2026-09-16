---
title: "Agents and Agent Tasks"
created: "2026-09-07"
updated: "2026-09-15"
---

# Agents and Agent Tasks

Labby owns durable Agent definitions and Agent Task lifecycle. These resources
are separate from Depot artifacts and ingestion jobs.

Every definition and task has exactly one `installation`, `team`, `project`, or
`personal` owner. The authenticated principal is resolved by the host; owner
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
and inherits the rest from the prior revision. Agent payloads, Task inputs, and outputs use
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
does not leave its provider session behind. Direct `agents.run` input is bound
only to that run; the immutable Agent instructions remain pinned to the selected
revision. Successful text output is stored content-addressed and returned with
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
