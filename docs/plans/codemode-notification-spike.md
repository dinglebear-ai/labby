# Code Mode piggyback notification spike

> Historical spike notes. Superseded by the durable implementation and
> [Agent Notifications contract](../dev/AGENT_NOTIFICATIONS.md). The ephemeral
> delivery and missing ACK/sender limitations below describe the original spike,
> not the current implementation. They are retained as design history.

Status: experimental local implementation, not a production delivery contract.

## What this proves

A trusted in-process producer can enqueue a short notice for a specific consumer.
The next eligible MCP Code Mode response carries a bounded batch in both its JSON
text block and structuredContent. The JavaScript result, execution error, other
content blocks, and UI metadata are not replaced. Empty inboxes add no fields.

Example extension (siblings of the existing result/trace fields):

    "notifications": [{
      "id": "notice_<opaque-id>",
      "source": "spike",
      "level": "info",
      "message": "Indexing finished."
    }],
    "notifications_remaining": 0,
    "notifications_are_advisory": true

Notices are informational data, not instructions or renewed authorization.
No assistant wake-up, push transport, or human-visible alert is promised.

## Implementation seam

- notifications/codemode.rs extends NotificationCenter with a separate ephemeral
  inbox. Existing operator records are never selected or copied into it.
- McpRouteRuntime owns the center, so stateless HTTP handlers for the same route
  share pending notices. There is no process-global cross-route inbox.
- call_tool_codemode/notices.rs adapts the final MCP envelope, after authorization
  and execution. Both successful results and executed-script failures are eligible;
  early scope, stale-credential, source-validation, and filter rejections are not.
- enqueue_code_mode_notice is a trusted Rust producer seam, not a public sender
  action. This branch does not add a tool that lets arbitrary callers choose another
  recipient. No existing job/health producer is connected automatically.

## Identity and isolation

HTTP delivery requires both the server-established actor key and authorized client
ID. The HTTP relay-session ID changes with every POST and must not identify an inbox.
The key also includes the route and optional hashed openai/session metadata.
That metadata only narrows routing within authenticated authority; it does not grant
access. Without conversation metadata the inbox is client-scoped, not conversation-
scoped. It must not be described as a secure independent task/conversation identity.
Unauthenticated HTTP, missing actor/client identity, and unspecified transports fail
closed for notices. Explicit trusted stdio uses its server-owned connection ID.

## Bounds and delivery semantics

At most three notices and 1,024 serialized UTF-8 bytes are appended per response.
Source is limited to 64 bytes, message to 384 bytes, and dedupe key to 128 bytes;
control characters and blank fields are rejected. A single notice must fit after
JSON escaping. TTL must be greater than zero and no more than 24 hours.
The inbox retains at most 200 entries, at most 32 for one recipient, including
attempted-delivery tombstones retained until expiry for deduplication. Full queues
reject new entries explicitly rather than silently evicting another recipient.

A nonblocking lock prevents inbox contention from delaying execution results. A
candidate is built before any entry is marked attempted. The complete serialized
MCP result, including duplicated text/structured fields, must fit the configured
byte and estimated-token ceilings. Smaller batches are tried; when none fits,
notices stay pending. Existing oversized results are left unchanged. This spike
does not reserve space by truncating or rewriting the user's execution output.
Malformed/non-JSON text and extension-name collisions likewise defer delivery.

Delivery means one successful response-assembly attempt, not network receipt.
Disconnects after assembly can lose a notice. Attempted IDs deduplicate until TTL.
The inbox and delivery state are intentionally excluded from operator persistence:
process/route recreation loses them. There are no ACKs, retries after assembly,
priority sorting, user-wide fan-out, broadcast, or public queue management yet.

## Verification entry points

Focused automated cases cover quiet empty results, isolation, deduplication,
batching, actual JSON byte caps, expiry, input validation, queue bounds, contention,
concurrent consumers, operator-feed exclusion, serialization, metadata/error
preservation, budget deferral, open output-schema compatibility, and scope denial.

    cargo nextest run -p labby --all-features --lib --locked -E 'test(notice_spike_)'

The explicitly ignored native test needs a real binary from the same worktree. It
uses a native RMCP client over a duplex byte transport and executes actual JavaScript
through that binary's Code Mode runner. It is not a mocked upstream response or an
HTTP/ChatGPT UI acceptance test. Build and run it with a fresh private LABBY_HOME
and a short private TMPDIR, using the supported LABBY_CODE_MODE_RUNNER_EXE override:

    cargo build -p labby --all-features --bin labby --locked
    LABBY_CODE_MODE_RUNNER_EXE="$PWD/target/debug/labby" \
      cargo nextest run -p labby --all-features --lib --locked \
      --run-ignored ignored-only -E 'test(notice_spike_native_mcp_real_runner)'

Do not point the test at production state. A raw Cargo runner build does not refresh
web assets and must not be installed as a production/UI build.

## Promotion decisions still required

Choose an authenticated task/channel binding before exposing targeted send tools.
Decide whether user-wide notices are independently delivered to each consumer or
consumed by one. Add durable storage, ACK/retry and reconnect semantics if receipt
matters. Integrate authorized producers and observability for rejection/deferral.
Advertise explicit optional schema fields, regenerate catalogs, and qualify the
real HTTP/client/UI combinations before promoting beyond this spike.
