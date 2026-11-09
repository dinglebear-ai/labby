---
title: "Agent Notifications"
created: "2026-10-02"
updated: "2026-11-08"
---

# Agent Notifications

Labby stores short advisory notices for an authenticated agent inbox. A notice
can appear on a normal Code Mode response or, when enabled, go out through MCP
Events or Claude Code Channels. All paths use the same notice ID, producer
authorization, and separate agent acknowledgment. A notice is never authority
to act or proof that a human read it.

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

## Push delivery

### MCP Events over HTTPS

Start the gateway with `labby serve --mcp-events`. The authenticated root HTTP
MCP route advertises `capabilities.events` on legacy initialization and
`server/discover`. Default, Unix, stdio, and protected subset routes do not
offer webhook subscriptions.

`events/list` describes `labby.notification.created`. Subscribe through
`events/subscribe` with its name, optional exact `arguments.source`, and
webhook `delivery` (`mode`, `url`, `secret`). The signed HTTPS challenge
must succeed before activation. Labby derives the inbox from the authenticated
caller, so subscription arguments cannot select someone else's address.
Subscribe and unsubscribe require `lab` or `lab:admin`; authenticated read
authority can list the definition. OAuth requires a current refresh grant for
the same client, identity, scope, and resource. Product credentials must still
match live access policy. Browser cookies do not enroll subscriptions.

Subscriptions apply to future notices. The default lifetime is one hour and
the granted bounds are one minute to 24 hours. `ttlMs: null` grants 24 hours.
Refresh with the same identity tuple and a fresh authenticated request.
The response gives a finite `refreshBefore`. Use the same event name, filter,
and URL, without the secret, to unsubscribe. Labby has no replay cursor.

Callbacks must resolve exclusively to public addresses. Labby pins the checked
addresses for each request, verifies TLS, and disables redirects and ambient
proxies. Signing keys are encrypted in the private database; keep
`webhook-encryption.key` with any protected inbox backup. A refresh can replace
the signing key, with both signatures valid for up to five minutes. Labby
pauses delivery after restart until an authenticated refresh restores live
authority. Notice and subscription expiry still apply.

Each event carries the existing notice ID, creation time, and advisory data.
Transient delivery failures retry up to eight attempts, with one request per
subscription and eight across workers. A successful HTTP callback ends webhook
retries. The notice remains available to its recipient until ACK or expiry.
An ACK stops pending retries for that notice. See the [ChatGPT MCP Events
profile](https://developers.openai.com/plugins/build/mcp-events) for the client
and workspace requirements; local Codex sessions do not use that receiver.

### Claude Code Channels

Configure a local stdio MCP server with command `labby` and arguments
`["mcp", "--claude-channel"]`. The gateway and Code Mode inbox controls must be
available. Then opt in to the channel in Claude Code. Follow the current
[Claude Channels documentation](https://code.claude.com/docs/en/channels) for
client policy and approval for custom channels. Channel mode negotiates a
legacy MCP initialization handshake because Claude Code does not register a
channel on MCP revision `2026-07-28`. Labby does not relay permissions.

After the MCP initialization handshake, Labby registers this connection's inbox
and sends `notifications/claude/channel` for pending notices. A normal
`notification_inbox: true` Code Mode call returns the address to share with an
authorized producer. The publishing API and channel must use the same private
Labby installation; this also applies when the stdio connection bridges to a
running daemon. The bridge requires an authenticated loopback daemon that
confirms the same local installation. A new connection gets a new address. The worker ends when the
connection closes.

Treat channel content as advisory and deduplicate on `meta.notice_id`.
Send that ID in `ack_notifications` on a normal write-capable Code Mode call
after considering it. A successful stdio write does not acknowledge the notice.

These options deliver existing notices. Scheduling scripts, running tools on a
timer, and watching CI or logs are separate producer features.

## Private session messages

Start an authenticated TCP MCP daemon with `labby serve --agent-messages`, or
enable messaging on a direct local connection with `labby mcp --agent-messages`.
On HTTP, configure each logical client session to send its own
`Labby-Session-Key` header on every message tool call. Generate 32 random
bytes and encode them as 64 lowercase hexadecimal characters. Keep the key
private in the client configuration and reuse it for that session's reconnects.
Labby assigns a public `session_id` when the connection registers.

Call the `messages` tool with an action and a params object.

| Action | Params | Result |
| --- | --- | --- |
| `register` | `{"label":"build monitor"}` | This session's ID and presence |
| `sessions` | `{}` | The owner's retained session IDs, labels, and presence |
| `send` | `to`, `body`, `idempotency_key`, optional `summary` | Message ID and the original recipient IDs |
| `unread` | Optional `limit` from 1 to 32 | Unread summaries and IDs |
| `read` | `message_id` | The full body, sender ID, original label, and timestamps |
| `ack` | `message_ids`, up to 32 | IDs acknowledged by this session |

Use `{"kind":"session","session_id":"…"}` for a direct message or
`{"kind":"all_active"}` to broadcast. Broadcast snapshots the owner's active
sessions, including the sender. HTTP sessions count as active for five minutes
after successful messaging activity or subscription refresh. Stdio sessions
renew a two-minute lease while the connection is live. A clean disconnect
closes the registration. You can still send directly to a retained HTTP
session whose activity lease has expired.

Bodies can contain up to 16 KiB of UTF-8. An optional summary is limited to
160 bytes. Labby keeps each message and its send receipt for 24 hours. Use the
same idempotency key and payload if the inbox is busy or a send times out. Labby returns the
original recipient list on replay. A changed payload with the same key is a
conflict.

Only the addressed session can read or acknowledge a message. Bearer and OAuth
sessions use their verified owner identity and the private session key. A
bridged stdio connection requires a daemon at a literal loopback address,
the same private Labby installation, and a current authenticated bridge proof.
Standalone stdio uses the installation's local owner group. A standalone
process holds the installation lock, so use a shared daemon and stdio bridges
for multiple concurrent local clients. Reopening stdio creates a new session.

For HTTPS push, run `labby serve --agent-messages --mcp-events`. Register
first, then subscribe to `labby.notification.created` with
`arguments.session_messages: true` and the same bearer authority and
`Labby-Session-Key`. Use the same proof for refresh and unsubscribe. For
Claude Code, configure `["mcp","--agent-messages","--claude-channel"]` and
follow the client approval process for custom channels. The short advisory
push carries `data.message_id` over MCP Events or `meta.message_id` over
Claude Channels. Call `messages.read` for the full body and `messages.ack`
after handling it. Ordinary Code Mode inbox controls continue to handle
short notices.

An HTTP client must support the private header on both tool and Events
requests to use message push. Native ChatGPT Work delivery with that header
and native Claude Code acceptance require client verification. The direct
`messages.unread` tool is available to eligible clients without push.

## Durability, isolation, and recovery

The store is LABBY_HOME/agent-notifications/inbox.sqlite3, opened under the existing
validated installation root. Its directory is private, the file is privately
created, and insecure files, symlinks, unsupported schema versions, or corrupt
SQLite state are refused. Startup disables notices on failure; it never silently
switches to an ephemeral inbox. Schema v3 adds session registrations and
private message references while preserving earlier notices, subscriptions,
and delivery leases. Keep `messaging.key` alongside the protected inbox
backup so HTTP clients can reconnect to the same session. An older binary
cannot open the upgraded store. Restore a consistent protected backup through the
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

OAuth uses the full verified provider issuer and subject, the transport issuer,
and the authorized client ID. OAuth inboxes from older versions did not record
issuer provenance. Their rows remain until expiry, but OAuth consumers must
register a new address and update the producer after upgrading. Static bearer and
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

The [inbox owner](../../crates/labby/src/dispatch/codemode_notices.rs) owns
storage, validation, idempotency, and ACKs. [Event dispatch](../../crates/labby/src/dispatch/agent_events.rs)
owns subscription lifecycle, live authority checks, signing, and retries. The API and MCP adapters only bind
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
