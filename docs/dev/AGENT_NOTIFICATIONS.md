---
title: "Code Mode Agent Notifications"
created: "2026-10-02"
updated: "2026-10-03"
---

# Code Mode Agent Notifications

Labby can append short advisory notices to normal Code Mode MCP responses. This
is an authenticated pull-through inbox, not a push connection, assistant wake-up,
or a claim that a human read a message. The JavaScript result and error semantics
remain intact. Text content and structuredContent carry the same notice fields.

## Agent and producer workflow

The canonical [using-codemode skill](../../plugins/labby/.apm/skills/using-codemode/SKILL.md)
explains when to register, how to consider and acknowledge notices, and why message
bodies never grant instructions or permission. Its [reference contract](../../plugins/labby/.apm/skills/using-codemode/references/code-mode.md#response-notification-contract)
contains complete registration, publication, and ACK examples and exact limits.
The live Code Mode descriptor and schemas advertise these optional controls,
including to clients which have not loaded the skill. Do not use new arguments
unless the connected binary advertises them.

A consumer registers with notification_inbox=true on a normal write-capable call.
The returned opaque address selects one authenticated consumer; it is not a
credential or a broadcast target. A producer with lab:admin submits a bounded JSON
body to POST /v1/notifications/agent. It supplies source, level, message, dedupe_key,
inbox_id, and optional ttl_seconds. The producer's authority comes from HTTP
middleware, never from the body. No public caller can choose an arbitrary actor
inside the Code Mode function. Existing operator notifications remain separate.

The next eligible Code Mode result or executed-script failure includes up to three
notices, no more than 1,024 serialized bytes, within the complete result's byte and
estimated-token limits. Empty results are unchanged. Full results, contention,
malformed text, and storage errors defer delivery instead of rewriting execution
output. Registration and acknowledgments are not accepted by codemode_read.

After considering a notice, the agent sends its ID in ack_notifications on its
next ordinary write-capable call. It deduplicates retries and verifies consequential
claims against live state. Receipt does not imply human reading, task completion,
or authorization. Foreign/unknown/expired/never-offered IDs cannot be acknowledged.

## Durability, isolation, and recovery

The store is LABBY_HOME/agent-notifications/inbox.sqlite3, opened under the existing
validated installation root. Its directory is private, the file is privately
created, and insecure files, symlinks, unsupported schema versions, or corrupt
SQLite state are refused. Startup disables notices on failure; it never silently
switches to an ephemeral inbox. Restore a consistent protected backup through the
normal stopped-daemon maintenance workflow; do not delete or edit the live database
to hide a storage failure. Notification data may be sensitive and must not be
included in public diagnostics or shared backups.

Startup storage diagnostics identify rejected `schema_version`, `application_id`,
`schema_mismatch`, `integrity_check`, and `foreign_keys` validations through a
stable `reason` field. Version and application-ID rejections also include a numeric
`observed_value`. Logs omit schema SQL, integrity-check text, database paths, and
notice payloads. An unsupported version or application ID calls for checking the
binary/backup compatibility; integrity and foreign-key failures call for a
consistent protected backup. Keep the daemon stopped during maintenance and
verify the restored store before serving consumers again.

SQLite transactions durably store registration, idempotent publication, leases,
and acknowledgments. An unacknowledged notice becomes eligible again after 30
seconds, with bounded exponential backoff to five minutes, until its TTL expires.
Dedupe tombstones remain until expiry. Same producer/inbox/key/payload returns the
same ID; changed payload is a conflict. Registration refreshes a 30-day address;
notice TTL is at most 24 hours and cannot outlive the address. Limits are detailed
in the canonical reference, including global and recipient quotas.

OAuth uses the authenticated actor and authorized client ID. Static bearer and
product credentials use verified credential fingerprints, not clientInfo or caller
supplied IDs. Protected-route identity includes its Team, loadout, and effective authority
partition as well as the route label; rebinding a route cannot inherit an old
partition's inbox. Optional hashed conversation metadata narrows routing.
Without that metadata, clients sharing authority share an inbox. Conversation
metadata is not an independent security boundary. Stdio has a random server-owned
connection identity; reconnects must register again rather than inheriting mail.

Each store admits at most one queued/executing blocking database operation. SQLite
request lock contention is bounded to 25 ms; startup allows up to one second
per SQLite lock acquisition. Each request store operation waits at most 250 ms.
A call with controls and delivery can wait for two store operations. A
deadline does not undo a committed or still-running write. Retain the original
idempotency key or ACK IDs, inspect the typed error and live state, then retry with
backoff. Do not retry authorization, unsupported-schema, or changed-payload
conflicts unchanged. Notification failure never converts a script success into a
failure or replaces its original error. Explicit control validation does prevent
the script from starting; a later script failure does not undo accepted controls.
Registration and ACK controls on the same call commit atomically. If a full
response omits their receipt, repeat only the controls with a harmless compact
script or your next ordinary call. Never replay a mutating script solely to
recover a receipt.

## Source ownership

Shared storage, validation, idempotency, and delivery semantics live in
crates/labby/src/dispatch/codemode_notices.rs. The API and MCP adapters only bind
trusted identities and transport the shared contract. Startup owns durable-store
initialization; shared route runtime carries it across stateless HTTP requests.
The MCP schema and description builders own discoverability. Skills are authored
under plugins/labby/.apm/skills and embedded through crates/labby/src/skills.rs.
See [plugin primitives](../../plugins/labby/AGENTS.md#primitive-ownership) for
source paths, aliases, generated clients, and regeneration checks.

## Qualification and release

Focused tests use the notice_production_ filter. They cover durable reopen,
lease retry, idempotency/conflicts, ACK isolation, quotas, filesystem safeguards,
contention/deadline behavior, metadata/error preservation, schemas, and the actual
embedded skill. Explicitly ignored native/HTTP tests require a newly built runner
from the same sources and a private LABBY_HOME and short TMPDIR. Their supported
override is LABBY_CODE_MODE_RUNNER_EXE. The loopback HTTP test uses production auth
middleware and actual JavaScript execution, then reopens the durable store after
server recreation. Do not point qualification at production state.

Run focused and broader Rust tests, warnings-denied clippy, supported feature
slices, plugin generator/check, and documentation gates. Build static web assets
through the repository recipe before creating a user-facing binary. Source
validation, a packaged binary, client installation, publication, and production
deployment are separate outcomes. This document is a contract, not a test receipt
or evidence that the running deployment has been upgraded.
