# Labby session messaging design

A user can name their Labby sessions, see which of them are available, send a
long message to one session, or send it once to all of their active sessions.
The recipient sees who sent it and can read the full text independently of
whether the client can receive a push. This builds on the existing private
notice inbox, signed MCP Events delivery, and Claude Channel delivery.

Homelab monitoring and GitHub alert ingestion can become producers after
this work. This change does not install those integrations, create schedules,
or add an adapter that starts a turn in local Codex.

## Session authority and availability

- Enable the MCP `messages` tool with `labby serve --agent-messages` on the
  authenticated root HTTP route, or `labby mcp --agent-messages` on local
  stdio. Neither flag requires push delivery to use the unread inbox. Do not
  expose the tool on protected routes, browser-cookie auth, Unix HTTP, anonymous
  HTTP, or through an aggregate upstream proxy. The HTTP caller needs `lab`
  or `lab:admin` and current verified identity; a real stdio invocation is
  trusted local. These requirements apply to direct invocation as well as listing.
- A verified external OAuth principal is scoped by the full provider issuer,
  subject and Labby authorization issuer. Recheck a current equivalent consent
  grant through the existing Events authorizer before each sensitive operation;
  do not claim revocation of individual tokens within one equivalent grant.
  A product credential groups the verified principal within its bound
  installation, organization/project and root route realm, and must pass the
  current project policy/generation recheck. Admit it as an explicitly local,
  root-only tool after project authorization and before terminal upstream
  dispatch. A static bearer owner is keyed to the exact verified token with a
  domain-separated HMAC under a private installation key. The middleware's
  constant static slot ID is insufficient; rotation starts a new realm.
- Every HTTP message operation and message-mode subscription requires exactly
  one `Labby-Session-Key` header: 64 lowercase hex characters for 32 random
  bytes provisioned privately in that MCP connection. Reject absent, malformed
  and duplicate keys, even with a copied `openai/session` marker. Bind the
  key under the full verified credential/client/route facts. IDs and chat
  metadata are not proof. Never forward, persist, log, or return the raw key.
  Actual TCP listener selection gates the feature; a Unix socket uses the
  same MCP handler label but is not eligible.
- On stdio use the server-minted connection ID. A standalone server groups
  sessions only inside its private local installation. A local bridge first
  verifies the existing same-installation loopback location, then obtains a
  fresh, authenticated owner proof from the daemon over a private root
  `POST /v1/notifications/agent/message-bridge-proof` call. The request MAC
  binds a fresh nonce, time, installation, and the exact selected bearer;
  the daemon rechecks scope and current grant. Its reply MAC covers the exact
  bearer, installation, nonce, owner, daemon epoch and expiration. Both sides use a protected 32-byte
  `agent-notifications/messaging.key` with separate MAC domains. The client
  checks the reply against its challenge, expected installation and bearer;
  an unsigned discovery marker never grants authority. Proof carries no
  raw credentials and never appears in discovery. Only literal loopback,
  no redirect/proxy, same installation and a current eligible bearer are
  allowed. Verify socket peer address and the actual TCP listener, never
  Host or forwarded headers. Bound and verify both MAC transcripts in
  constant time; mark proof headers sensitive before tracing. Fail closed on any mismatch. A bridged message tool call proves
  owner again before local DB access, and renewal or push of typed references
  requires a current proof no more than five seconds old. Loss, auth change or
  daemon-epoch change stops message renewal/delivery and requires a fresh
  bridge. Legacy notices retain their existing path. A per-connection close
  fence prevents a late register or renewal from reviving an old session.
- `register` gives the current binding one opaque `session_<ULID>` and
  accepts a friendly label. Registering that binding again returns the same
  ID while retained and can update the label. The label never identifies
  or authorizes a send. The owner-scoped directory contains IDs, labels,
  availability and last-seen times only. A different stdio connection always
  receives a new ID; HTTP can return with its same verified binding.
- Active means a live stdio registration with a renewed lease or HTTP
  messaging activity in the last five minutes. Live stdio renews every
  30 seconds, expires after 120 seconds if lost, and closes its binding
  when the service exits. HTTP is stateless: message-mode subscribe/refresh
  counts as recent activity, but a persisted, unexpired callback alone does
  not prove presence. The existing worker rechecks delivery authority.
  Registration expires after 24 hours without renewal, except when retained
  through an unexpired send, message or subscription. Retention alone never
  renews activity. Closed stdio stays inactive and cannot be rebound.

## MCP tool and persistence

A single permanent MCP tool named `messages` takes `{action,params}`.
Reject unknown fields, especially caller-provided sender, owner, inbox,
callback, and recipient-list overrides. The tool is invoked directly on the
client connection, never as a delegated Code Mode or proxy call.

| Action | Input | Result |
| --- | --- | --- |
| `register` | `label` | The current session ID, label and availability |
| `sessions` | empty | Retained sessions belonging to the current owner |
| `send` | `to: {kind:"session",session_id}` or `{kind:"all_active"}`, `body`, optional `summary`, `idempotency_key` | `message_id`, original recipient IDs, `duplicate` |
| `unread` | optional `limit` (1–32, default 10) | Up to that many unread summaries in creation/ID order and `more` |
| `read` | `message_id` | Immutable sender ID and label, full body, summary, creation and expiry |
| `ack` | `message_ids` (up to 32) | IDs acknowledged for this recipient |

Register before other actions. An exact target can be any unexpired registered
same-owner HTTP session, even if its activity lease is idle, or a live stdio
session. Unknown, closed and foreign targets give the same unavailable result.
An all-active send takes an owner-scoped snapshot in its transaction, includes
the sender when active, and commits all recipients or none. The same sender and
idempotency key replay the original receipt before recalculating presence. A
changed target, body or summary under that key conflicts. A successful send
renews the sender's activity before taking the snapshot in the same transaction.
Sender ID and label are fixed at commit. Only the exact recipient can read the
body, even if another session under the same owner knows the message ID.
Reading, push transport receipts and listing do not acknowledge a message.

Store immutable bodies in the existing installation-private inbox SQLite
database, once per message. Schema v3 preserves v1/v2 inbox addresses, notices,
acknowledgments and Events leases. Link one existing notice row per recipient
to the immutable body. Make the body, recipient snapshot, notices and existing
webhook outbox atomic in one immediate transaction. Use the existing bounded
database executor and publisher wake after commit, including commits that finish
after a request deadline. On an uncertain send, retry only with the same key.
The old admin producer cannot inject typed message references or address the
dedicated HTTP message inbox. The exact `messages` name is reserved even if
disabled. Do not resolve it to a same-name upstream, and prevent aggregation
of a Labby messaging tool through qualified aliases. Keep unrelated upstream
tools visible. Enforce the same predicate in listing, catalog hash and calls.

- Maximums: 16,384 UTF-8 bytes per body, 160 bytes per optional summary,
  64 bytes per nonempty label, 128 bytes per idempotency key, 32 retained
  sessions per owner and 512 total, 64 retained messages per owner and 256 total,
  24-hour message/receipt retention, 128 KiB for a complete serialized message
  tool result. Preserve newlines and tabs in bodies; reject other controls
  except CR. Validate a complete serialized result before committing a message
  that would not fit. Do not expose body bytes in debug/error telemetry.
- Preserve the 384-byte notice text, three-notice / 1,024-byte pull limits,
  existing per-inbox and global notice limits, event receiver bounds, and
  v2 ownership/lease behavior. Backpressure rejects a send atomically. Do
  not delete a reference with a live Events lease to make space. Clear an
  expired receipt's dedupe key even if a lease pins the old row. Prune expired
  references, then unreferenced expired bodies, then unneeded registrations
  and inboxes. Retain each sender and recipient incarnation through any
  unexpired message/receipt, and guard retained sessions and live
  subscriptions from cascading deletes. A same-proof HTTP reconnect can
  replay its receipt; a different proof or stdio connection cannot inherit
  it. ACK stops pending outbox work without releasing a live claim early;
  it cannot recall bytes already sent. Prune and preflight once per send.
  SQLite page exhaustion is a typed capacity failure with no partial commit.
  Acknowledged bodies are readable until expiry while their binding is valid.


## Delivery

Each notice carries a bounded advisory summary and a typed `message_id` when
it is a reference. Do not put the full body on push or on unrelated tool
responses. If no summary was supplied, use a fixed short notice that identifies
the sender's label. The sender's durable ID and original label are on
`messages.read`. `messages.ack` acknowledges the recipient's reference and
clears its unread state. Ordinary Code Mode controls cannot offer or acknowledge
typed references because those controls do not verify the message session.

A stdio session uses its existing exact connection inbox; Claude Channels
include the reference in `meta.message_id`. An HTTP messaging binding uses
a dedicated, separate inbox so old credential-wide notices cannot leak
into session delivery. Add an optional `session_messages: true` subscription
argument to `labby.notification.created`. Persist the flag and enforce it
during enqueue: typed refs only go to message subscriptions, normal notices
only to normal subscriptions. Preserve legacy filter identity when absent.
Bind the subscription only to the already enrolled session proven on the
request. Recheck the exact session incarnation/inbox inside the activation
transaction after the callback challenge and authorization recheck. Authorization
revoked after that check may leave a stored subscription, but the worker
revalidates it before delivery. Renew
activity and retain that registration through the granted expiration in the
same transaction, without generic inbox re-registration. If the caller's
database response times out, reconcile actual completion in the database
worker so a late commit does not lose its live authorizer. Message Events
carry optional `data.message_id`; keep existing signing, verification,
retry, offer and cancellation semantics. Refresh is another subscribe.

Both push envelopes are advisory. Claude's instructions teach the recipient
to read the referenced body and acknowledge it through `messages`. For
HTTP, native clients must present the same private key on tool and subscription
requests. In particular, do not claim ChatGPT Work support without proving
that a native subscribed chat can present that key and read the body; missing
proof cannot be replaced by metadata. A client with no push can still read
and acknowledge over an eligible HTTP or stdio connection.

## Acceptance

Prove migrations and rollback; authenticated identity isolation; private
session proof and live leases; stable direct/broadcast receipt under retries
and concurrent changes; full UTF-8 payload read only by its recipients;
atomic rejection at capacity; preservation of short-notice paths; signed
HTTPS reference delivery and unsolicited stdio reference delivery; no ACK
from a push receipt; process exit, persistence and documented reconnect
behavior. Use disposable local installations and pinned tooling. Record native
client wake evidence separately from protocol fixtures.
