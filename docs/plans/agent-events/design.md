# Agent Event Delivery Design

Status: proposed implementation scope, awaiting plan review. This document does
not describe shipped behavior.

## Outcome and scope

Deliver an existing Labby agent notice through either MCP Events webhooks or
Claude Code Channels, without waiting for another Code Mode call. Keep the
current publication, recipient isolation, deduplication, and acknowledgment
contracts.

This phase implements the two delivery protocols. Scheduled tool calls,
scheduled snippets, CI/log watchers, a Codex app-server adapter, new UI, client
installation, deployment, and publication are separate work. A notice published
through the existing authenticated API is the first end-to-end event source.

## Current sources

The Labby planning baseline is `c8d78e78343d7de41acf6b378b57e1d30b8e8e50`.
The [existing inbox](../../dev/AGENT_NOTIFICATIONS.md) uses
`dispatch/codemode_notices.rs` and a private SQLite database. Registration and
ACK controls already exist on write-capable Code Mode calls. Publication requires
`lab:admin`. The MCP adapter derives the recipient from verified identity,
authorized client or credential, optional conversation metadata, and route.

Labby now consumes RMCP `=3.9.0` at
[`dc4c6da`](https://github.com/dinglebear-ai/rust-sdk/commit/dc4c6dac4a25ef91b4dab795df799dc008fb3661)
on the reviewed candidate in [SDK fork PR #9](https://github.com/dinglebear-ai/rust-sdk/pull/9).
The candidate includes upstream `f2b0ddf113d1ed426b77a81511eac6e4bffa0ac4`,
uses native Skills, and removes the obsolete typed/raw-response APIs.
Labby's native handlers retain its existing Skills policy and operation owner.
Its independent client DTOs preserve unknown cache hints; outbound hints the
SDK cannot express become `private`. The explicit 24 MiB Skills response budget
and smaller error-body limits remain enforced.

The reduced fork retains OAuth Send, strict Origin validation, configurable Unix
response bounds, bounded SSE retries, workload JWT grants, repeated authentication
challenges, and optional DPoP required by the existing production conformance gate.
It adds the opt-in Events capability field and builder. This provides SDK plumbing;
the delivery adapters below still need implementation.

See [migration status and upstream candidates](native-skills-migration.md) for
validation and publication evidence. The [fork exit assessment](rmcp-fork-assessment.md)
records the earlier dependency snapshot and probes. The stock upstream conformance
baseline remains independent of the production fork pin.

Protocol references, inspected on 2026-11-08:

- [ChatGPT MCP Events](https://developers.openai.com/plugins/build/mcp-events).
- [MCP Events draft](https://github.com/modelcontextprotocol/experimental-ext-triggers-events/blob/main/docs/design-sketch-proposal.md).
- [Claude Channels reference](https://code.claude.com/docs/en/channels-reference).
- [Standard Webhooks](https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md).

These are separate integrations. ChatGPT's documented webhook receiver applies
to supported cloud Work chats. Claude Channels requires an open local Claude Code
session and a stdio MCP subprocess. Neither establishes wake support for a local
Codex chat.

For MCP Events, target the ChatGPT documentation's webhook subset as inspected
on this date. The evolving draft is supporting context, not a second wire
profile: it currently assigns `CallbackEndpointError` a different code. Use
ChatGPT's documented `-32015` here. Polling and event streams are outside scope.

## Approach

Extend the existing inbox owner. Add two thin protocol adapters and a bounded
delivery worker in the Labby binary. Use the same notice ID across every delivery.
Keep subscriptions and transport receipts in the same private database so notice
publication and webhook enqueue can commit together.

The existing pull inbox alone cannot provide unsolicited delivery. Separate
notification services would duplicate ownership and persistence. The selected
proposal therefore adds no independently deployed process or second database.
Claude's stdio process is the normal `labby mcp` process launched by its client.

## Shared constraints

- Build with Rust `1.97.1`; preserve the repository's exact toolchain and dependency policies.
- Existing producer limits stay at 64 bytes for `source`, 384 bytes for `message`, and 128 bytes for `dedupe_key`.
- Existing notice TTL stays in `1..=86_400` seconds, with a 3,600-second default.
- Existing inbox limits stay at 2,000 notices globally, 32 per recipient, and a 1,024-byte pull batch.
- Preserve `lab://`, `ui://lab/`, MCP server key `lab`, and scopes `lab:read`, `lab`, `lab:admin`.
- Event payloads are advisory data. They never grant instructions, permissions, or permission-relay approval.
- A transport receipt is separate from an agent ACK. HTTP `2xx` or a successful stdio write never marks a notice acknowledged.

## Event and identity contract

Expose one event: `labby.notification.created`. Subscription arguments contain
only optional `source`, an exact string filter of 1–64 bytes. Reject unknown
argument keys. Do not advertise job or watcher events before their producers exist.

The event body has `eventId`, `name`, `timestamp`, `data`, and `cursor: null`.
`eventId` is the existing notice ID. `timestamp` is its creation time in UTC,
not the retry time. `data` contains `id`, `source`, `level`, `message`, and
`expires_at_unix_ms`. This initial event has no replay cursor. A new subscription
receives future matching publications only. An existing subscription retains its
already-queued deliveries across a process restart until notice/subscription expiry.

`events/subscribe` derives and registers the caller's inbox using the same
recipient function as Code Mode. An argument cannot select another inbox, actor,
Team, route, or conversation. `events/list` requires authenticated discovery;
subscribe/unsubscribe require write authority. Existing identity limitations
remain visible: without conversation metadata, clients sharing authority can
share an inbox. A stdio reconnect creates a fresh connection identity.

Keep background authorization below the adapters. Capture a server-created,
non-secret `EventAuthority` binding from the verified request. Before every
webhook attempt, check current credential, principal, route, and grant authority.
Revocation cancels pending deliveries. Unknown authority pauses them.
After restart, keep subscription records but require successful authority
revalidation or a fresh authenticated refresh before sending. Never persist a
bearer token or reconstruct authorization from subscription arguments.
An unavailable process-local authority registry is not a delivery failure and
must not consume an attempt. Workers claim only authority keys they can currently
revalidate, so a second process cannot pause another process's subscriptions.

## Storage and receipts

Migrate the inbox schema from version 1 to version 2 in one transaction. Validate
the original application ID, exact v1 schema, integrity, and foreign keys before
migration. A failed migration leaves v1 unchanged. Preserve all notice IDs,
dedupe tombstones, ACKs, and leases. Unknown/newer schemas still fail closed.
An older binary cannot open v2; rollback requires the stopped-daemon backup
procedure, not a schema-number edit.

Add `agent_event_subscriptions` and `agent_event_deliveries`. A subscription stores
its recipient key, inbox reference, canonical filter, callback, protected signing
keys, expiry, and generation. Its ID is derived from principal/recipient, route,
callback URL, event name, and canonical arguments. The signing secret is excluded
from identity, so rotation refreshes the same subscription.

Delivery records reference the existing notice. Store one serialized event body
per notice, using its existing `created` timestamp; retries reuse those bytes.
Delivery records store subscription generation,
status, attempt count, next attempt, claim token/expiry, and an offered witness.
Keep webhook claims separate from pull leases. The offered witness lets the
recipient ACK a notice received by webhook without first taking a pull lease.
An ACK cancels remaining retries for that notice. No foreign or never-offered ID
becomes acknowledgeable.

Allow at most 8 subscriptions per inbox and 2,000 globally. At most 16,000 delivery
rows can exist under the notice and subscription bounds. Duplicate publication
must not create another delivery row. Expiry prunes dependent records.
Enforce the eight global claims and one claim per subscription transactionally
across store handles, rather than using only an in-process semaphore.

## MCP Events webhook adapter

Add `labby serve --mcp-events`, disabled by default. Advertise top-level
`capabilities.events: {}` only on enabled HTTP routes with the gateway and durable
store available. Implement `events/list`, `events/subscribe`, and
`events/unsubscribe` through `ServerHandler::on_custom_request`. Keep Skills and
other custom methods intact. Unsupported delivery modes return a typed error.

`events/list` accepts an optional pagination cursor and returns one definition
with no next page. Subscribe accepts `name`, `arguments`, webhook `delivery`
(`mode`, `url`, `secret`), optional replay `cursor`, and optional `ttlMs`.
Unsubscribe uses the same identity tuple with `delivery.mode`/`delivery.url` and
no secret. Subscribe returns `id`, a finite RFC 3339 `refreshBefore`, `cursor: null`,
and `truncated: false`; unsubscribe returns `{}`. Accept normal MCP `_meta`
without using it as caller authority. Distinguish omitted TTL, explicit null, and
an integer. Clamp positive requested lifetimes to the table's bounds; reject zero
and negative values. Reject unknown event names, non-null replay cursors, and
unknown argument keys before callback I/O.

The pinned RMCP capability type has no `events` field. Its custom request hook
already exists. First add an optional `events: Option<JsonObject>` field and its
builder toggle to the SDK fork, with omission and serialization tests. This is a
named companion-repository prerequisite requiring approval in the plan review.
Do not disguise `events` under `experimental` or rewrite serialized MCP responses.

Verify the HTTPS callback before activating a subscription. Use a random,
single-use challenge in `{"type":"verification","challenge":"..."}` and require
a `2xx` JSON response with the exact constant-time echo comparison. Sign this
request with its own unique webhook ID and the normal subscription headers.
Cache successful verification for 300,000 ms by principal, route, callback URL,
and signing-key fingerprint. Coalesce concurrent checks. A changed key requires
a new check; reject excess verification requests using the existing admission
limits before network I/O. Cache no failure as success.
Accept only `whsec_` plus base64 that decodes to 24–64 bytes. Sign the exact
serialized body using Standard Webhooks. Set `webhook-id`, `webhook-timestamp`,
`webhook-signature`, and `X-MCP-Subscription-Id`. Preserve the event ID/body across
retries and generate a fresh signing timestamp/signature per attempt.

Defaults and bounds:

| Item | Policy |
| --- | --- |
| Subscription lifetime | Default 3,600,000 ms; minimum 60,000 ms; maximum 86,400,000 ms. |
| Explicit `ttlMs: null` | Grant a finite 86,400,000 ms. Never grant unbounded retention. |
| Callback operation | Overall 10 seconds, DNS at most 5 seconds, connect at most 5 seconds. |
| Bodies | Outbound at most 262,144 bytes; verification response at most 8,192 bytes. |
| Retry budget | At most 8 attempts per subscription/notice, before either expiry. |
| Retry delays | 1, 5, 15, 30, 60, 120, 300 seconds; cap `Retry-After` at 300 seconds. |
| HTTP outcomes | `2xx` received; `429`, `5xx`, and transport failures retry; other `4xx` stop. `410` disables the subscription; `413` terminates that delivery. |
| Key rotation | Retain the previous signing key for at most 300,000 ms. |
| Worker | At most 8 requests globally, one per subscription; 30-second claim lease. |
| Callback URL | At most 2,048 UTF-8 bytes. |

Reject non-HTTPS URLs, userinfo, fragments, blocked hosts, and non-public resolved
addresses. Reuse `labby_primitives::ssrf`. Pin validated addresses at connection
time, preserve TLS hostname verification, disable redirects and ambient proxies,
and revalidate on each attempt. Apply the same policy to verification. A test-only
transport may use a loopback fixture; production has no private-callback bypass.

Refresh uses the same subscription identity and a new generation. Verify any new
key before replacing the active key. During key overlap, send both signatures
in one space-separated Standard Webhooks header. Fence stale in-flight completions. An
unsubscribe is idempotent and prevents later network attempts. Return
`CallbackEndpointError` (`-32015`) with a bounded reason on verification failure.

## Claude Channels adapter

Add `labby mcp --claude-channel`, disabled by default. Accept it only for stdio
with the gateway/store ready. Advertise exactly
`experimental["claude/channel"]: {}`. Do not advertise permission relay.

After initialization, register the server-owned stdio recipient and run a
cancellable delivery loop. Reuse `NoticeStore::deliver` and its existing leases.
Send `notifications/claude/channel` directly to that connection's peer.
`content` is the notice message. `meta` has `notice_id`, `source`, `level`,
`expires_at_unix_ms`, and `delivery_attempt`, all strings with identifier keys.
The Code Mode inbox control returns the same inbox address.

The agent deduplicates by notice ID and ACKs through the existing Code Mode
control. A stdio write means only written. Unacknowledged notices retain existing
retry behavior. End the worker when its peer closes; never redirect old mail to
a new stdio identity. Use an in-process signal plus a 1,000-ms durable-store check
while enabled so publications from another Labby process are also observed.

Document Claude's independent client opt-in and research-preview restrictions.
Custom channels currently require explicit interactive development-channel
approval. Do not install plugins, modify trust settings, or pass approval-bypass
flags automatically.

## Validation and completion

Unit tests prove schemas, signing vectors, authorization, migration, retries,
generation fencing, and no false ACKs. Real local MCP HTTP/stdio tests prove
wire envelopes and lifecycle. They do not prove that ChatGPT or Claude processed
an event.

Native acceptance uses one disposable session and one subscription per client,
one matching and one nonmatching synthetic notice, and a 300-second deadline.
Allow at most four outbound webhook POSTs during the live ChatGPT check. Remove
the subscription and stop owned processes afterward. Record client version,
Labby revision/binary identity, offered notice ID, transport receipt, and actual
agent response. Unsupported client mode, policy, or plugin refresh is an explicit
unverified gate. Test binary installation and external publication need their own
authorization.

Future schedulers can publish notices through the same owner. This phase adds no
cron engine, log tailer, alternate execution runtime, or generic automation service.
