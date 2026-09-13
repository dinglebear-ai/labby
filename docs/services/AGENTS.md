---
title: "Agent Definitions and Sessions"
created: "2026-09-13"
updated: "2026-09-13"
---

# Agent Definitions and Sessions

The `agents` service owns immutable Agent revisions and one-shot execution
sessions. It is an authenticated, owner-scoped control-plane service. An Agent
belongs to a `team`, `project`, or `personal` owner; `installation` is not a
valid Agent owner.

Agent execution is unavailable until an operator configures a matching local
harness. Creating an Agent record does not install an executable, clone a
repository, pull an image, build a container, or provision a loadout. The
process executor launches only the command that the operator approved in
Labby's configuration.

Use the runtime `help` and `schema` actions, or the generated
[action catalog](../generated/action-catalog.md), for the complete action
schemas and required capabilities.

## Configure an execution harness

Harnesses live in the canonical `$LABBY_HOME/config.toml`; without an explicit
`LABBY_HOME`, that path is `~/.labby/config.toml`. Each entry under
`[[agents.harnesses]]` is an operator-owned launch descriptor:

```toml
[[agents.harnesses]]
id = "review-harness"
content_digest = "sha256:<64 lowercase hex characters>"
repository_digest = "sha256:<64 lowercase hex characters>"
image_digest = "sha256:<64 lowercase hex characters>"
loadout_digest = "sha256:<64 lowercase hex characters>"
catalog_generation = "<exact catalog generation>"
command = "/absolute/path/to/approved-agent-executable"
args = ["<literal argument>"]
cwd = "/absolute/path/to/operator-provisioned-workspace"
inherit_env = ["<APPROVED_SECRET_ENV_NAME>"]
```

Replace every placeholder with an exact value for the runtime the operator has
already provisioned. The five digest fields and `catalog_generation` are
operator assertions. Labby compares them with the Agent revision before spawn;
the current process adapter does not resolve those pins into files, containers,
or other resources.

Configuration validation enforces these limits:

- at most 16 harness descriptors;
- a unique 1–64 character ASCII `id` containing letters, digits, `-`, or `_`;
- an absolute `command` and, when present, an absolute `cwd`;
- at most 64 literal arguments, each at most 4,096 bytes and containing no NUL,
  newline, or carriage return;
- canonical lowercase `sha256:` digests for the content, repository, image, and
  loadout pins, plus a nonempty catalog generation of at most 256 bytes;
- unique inherited environment names of at most 128 characters, using uppercase
  letters, digits, and underscores and beginning with an uppercase letter.

`LABBY_*`, dynamic-loader injection variables, `RUSTC_WRAPPER`, `IFS`, `SHELL`,
and `PWD` cannot be inherited. The child starts from an empty environment. Labby
adds `PATH`, locale, temporary-directory, and certificate-path variables when
they exist in the server environment, followed by the explicitly allowed names.
Secret values stay in the server environment and are not returned by harness
discovery.

Restart Labby after changing this section. Then call `agents.harnesses`. Each
result contains the descriptor's computed `digest`, its public pins, and
`available`. Availability is false when the command is missing or not executable,
or when the configured working directory does not exist. The response does not
expose command arguments, paths, or environment values.

The computed harness digest covers the complete nonsecret descriptor. Changing
its command, arguments, working directory, inherited environment names, or any
pin produces a different digest. Existing Agent revisions continue to name the
old digest and become unavailable until they are updated to a matching approved
descriptor.

## Create a pinned Agent

`agents.create` stores an active revision containing these exact pins:

- content, repository, image, harness, and loadout digests;
- catalog generation;
- optional credential-reference names.

Credential-reference names are retained with the revision, but the current
process adapter does not resolve them. Secrets needed by the command must be
made available through the harness's operator-approved `inherit_env` names.

Use the values returned by `agents.harnesses`; do not invent a harness digest or
accept executable configuration from an Agent payload. The Gateway Admin first
session flow follows this rule: in an empty workspace it lists available
harnesses, copies the selected descriptor's exact public pins into
`agents.create`, and then calls `agents.run`.

For HTTP, send the normal service envelope to `POST /v1/agents`:

```json
{
  "action": "agents.create",
  "params": {
    "agent_id": "review-agent",
    "owner_kind": "personal",
    "owner_id": "<current principal id>",
    "content_digest": "<content_digest from agents.harnesses>",
    "repository_digest": "<repository_digest from agents.harnesses>",
    "image_digest": "<image_digest from agents.harnesses>",
    "harness_digest": "<digest from agents.harnesses>",
    "loadout_digest": "<loadout_digest from agents.harnesses>",
    "catalog_generation": "<catalog_generation from agents.harnesses>"
  }
}
```

The host-established identity and selected owner determine authority. Supplying
an `owner_id` does not grant access. See
[Selecting the authority context](../access-control/MULTI_USER_AUTHORITY.md#selecting-the-authority-context).
Agent identifiers are durable and cannot be used as cross-owner existence
probes: a taken or unauthorized identifier returns the same denial.

`agents.update` creates the next immutable revision. Omitted pins carry forward,
the owner cannot change, and updating a suspended Agent does not reactivate it.
Labby assigns revision and authority epochs; callers cannot set them.

## Run and inspect a session

`agents.run` accepts an active Agent ID, one nonempty input of at most 1 MiB,
and a caller-generated `idempotency_key`. The key must be 1–256 bytes with no
leading or trailing whitespace or control characters. Labby hashes and retains
the input, rechecks current owner authority, resolves an exact configured
harness, records an admitted session, and writes the input to the child process
on standard input. This is a one-shot process invocation; there is no
interactive input channel after admission.

Keep the same idempotency key when retrying a logical request after a timeout,
lost connection, or uncertain response. The durable key scope is the authorized
principal plus the Agent's immutable owner. Within that scope, a retry must also
match the operation, Agent ID and revision, input digest, and, for resume, the
source session. An exact match returns the existing durable session, including
after a Labby restart, and never registers or launches another process. Reusing
the key for different intent returns `conflict` with the existing session ID.
Labby records the key mapping in the same transaction as the session and input
evidence. It remains retained with that session; normal session settlement does
not expire the mapping.

If Labby stops after committing admission but before spawning the process, a
retry still returns that admitted session instead of launching a replacement.
Normal restart recovery marks the orphaned session `interrupted` after its
authority lease expires. Resume it with a new idempotency key after it becomes
terminal.

The execution lease lasts at most five minutes. Labby reserves its final five
seconds for durable settlement, so a newly admitted direct session normally
receives at most 295 seconds of process runtime; delayed admission is trimmed
further from the fresh remaining lease. The output bound is 16 MiB of combined
process output. The process adapter captures standard output for the digest and
transcript, drains standard error without putting it in the transcript, and
terminates the process tree after cancellation, failure, or timeout. A nonzero
exit is `execution_failed`.

Session state progresses from `admitted` to `running`, then to `completed`,
`failed`, `cancelled`, `revoked`, or `interrupted`. Use:

- `agents.sessions.list` for retained session summaries;
- `agents.session.status` or `agents.session.get` for one session's metadata;
- `agents.session.transcript` for its retained input and standard-output text;
- `agents.session.stop` to cancel a live session;
- `agents.session.resume` to start a new session from retained input.

List and metadata responses expose input and output digests, but not the input
text. The transcript action is owner-authorized and returns the input. It retains
at most the first 1 MiB of standard output, decodes invalid UTF-8 lossily, and
reports whether live capture was truncated. For a successful run, the output
digest covers all standard output, even when the displayed transcript is
shorter.

Resume is allowed only from a terminal session whose Agent revision still
matches the current definition. `agents.session.resume` also requires an
idempotency key. It performs fresh authorization and creates a new session
linked through `resumed_from_session_id`; it does not continue the old process.
A runtime restart marks orphaned admitted or running sessions `interrupted`
after their lease expires.

## Suspension, deletion, and revocation

`agents.suspend` blocks future runs while retaining the definition and history.
There is currently no separate reactivate action. `agents.delete` removes the
definition from normal reads and lists and blocks new execution.

Every admission checks the current principal, owner membership, Agent state,
definition pins, and authority epochs. The current revision uses
`stop_at_safe_boundary`: the process harness rechecks authority before spawn and
before committing its result. Revocation during a running process prevents a
successful result commit but does not promise immediate termination. Use
`agents.session.stop` when the operator needs to cancel the running process tree.

The HTTP and MCP adapters require host-established identity. Browser mutations
also require the session CSRF contract. The local CLI does not expose these
caller-bound operations because it cannot provide the equivalent authenticated
identity binding.

Durable queued and scheduled execution is documented in
[Agent Tasks and Schedules](./TASKS.md).
