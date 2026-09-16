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
and catalog generations. LLM-backed definitions materialize the model and Agent
instructions into Labby's content-addressed payload store and pin that payload
with the existing `content_digest`. Agent payloads, Task inputs, and outputs use
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
its digest.

Agent Tasks capture an exact Agent revision, normalized input digest, catalog
generation, owner, creator, and authority fingerprint. Callers may supply raw
UTF-8 `input`; Labby materializes it in the Task-input CAS namespace and stores
the resulting `input_digest`. The legacy digest-only form remains accepted for
compatibility; execution requires that digest to already be materialized in the
same Task-input namespace. Task idempotency keys are scoped to the owner and
bind the full immutable intent.
Queue, cancellation, execution, and settlement use fenced state transitions;
terminal settlement is exactly once. A freshly authorized `tasks.queue` may
resume a durable `queued` Task that has no live in-process owner, closing the
crash window between queue commit and scheduler handoff without double-starting
a live attempt. The durable attempt fence is bounded by the configured maximum runtime and is
rechecked against the current clock at acquire and settlement. Provider
or payload failures happen inside the owned queued attempt and settle through
the normal runtime as `failed` instead of bypassing durable queue admission. Terminal `tasks.result` responses
include materialized output text when available as well as the immutable output
digest. Current authority is required for every list, get, cancellation, and
result operation.

Authenticated HTTP exposes `POST /v1/agents` and `POST /v1/tasks` with the same
`action` plus `params` envelope used by MCP. MCP exposes the `agents` and
`tasks` caller-bound services only when the transport supplies a verified
identity. Context-free invocation fails closed. Local CLI invocation is not
offered because it has no equivalent authenticated identity binding.

How a caller selects the `owner_kind` and `owner_id` context on each surface is
listed in [Selecting the authority context](../access-control/MULTI_USER_AUTHORITY.md#selecting-the-authority-context).
