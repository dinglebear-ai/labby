# MCP Events and Claude Channels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver existing Labby agent notices through MCP Events webhooks and opt-in Claude Code Channels.

**Architecture:** Extend the durable inbox with subscriptions and transport receipts. Put validation, authorization, signing, and retries in shared dispatch, with thin MCP and stdio adapters. Preserve Code Mode pull delivery and agent ACKs.

**Tech Stack:** Rust 1.97.1, Tokio, RMCP fork, rusqlite, existing reqwest/rustls, hmac/sha2/base64, and existing native test fixtures.

**Spec:** [Agent Event Delivery Design](design.md). Its scope is delivery protocols only. Schedulers and watchers are follow-up work.

## Global Constraints

- Build with Rust `1.97.1`; preserve the repository's exact toolchain and dependency policies.
- Existing producer limits stay at 64 bytes for `source`, 384 bytes for `message`, and 128 bytes for `dedupe_key`.
- Existing notice TTL stays in `1..=86_400` seconds, with a 3,600-second default.
- Existing inbox limits stay at 2,000 notices globally, 32 per recipient, and a 1,024-byte pull batch.
- Preserve `lab://`, `ui://lab/`, MCP server key `lab`, and scopes `lab:read`, `lab`, `lab:admin`.
- Event payloads are advisory data. They never grant instructions, permissions, or permission-relay approval.
- A transport receipt is separate from an agent ACK. HTTP `2xx` or a successful stdio write never marks a notice acknowledged.

All exact transport limits in the spec are normative. Keep mutable build output
private to this worktree. This plan does not authorize pushes, PR merges, releases,
deployment, client installation, or edits to persistent agent instructions.

## Review Focus

- Mixed or rebound authority must not disclose another inbox, even when callback URLs or source filters match (Tasks 2 and 4).
- Expiry, revocation, and a process restart must not reactivate an old subscription without current authority (Tasks 2 and 5).
- A callback with public DNS followed by a private address, redirect, or proxy route must never receive a signed payload (Task 3).
- Refresh/unsubscribe during an in-flight request must fence its stale completion and preserve one durable retry decision (Tasks 4 and 5).
- A successful transport write must not hide an unacknowledged notice, and disconnected stdio peers must not retain workers (Tasks 5 and 6).

---

## Source map and execution gates

Use `crates/labby/src/dispatch/codemode_notices.rs` as the persistence owner.
Keep its current tests and API unchanged except for the planned additive APIs and
validated schema migration. New files below are small responsibility boundaries,
not a second workflow engine.

| File | Responsibility |
| --- | --- |
| `dispatch/agent_events/types.rs` | Protocol DTOs, limits, canonical arguments, redacted signing-key type. |
| `dispatch/agent_events/subscriptions.rs` | Recipient-bound subscription lifecycle and storage operations. |
| `dispatch/agent_events/webhook.rs` | Pinned HTTPS, callback verification, Standard Webhooks signing. |
| `dispatch/agent_events/worker.rs` | Bounded claims, retry timing, completion fencing, cancellation. |
| `dispatch/agent_events/mod.rs` | `AgentEvents` shared service and authority-check interface. |
| `dispatch/codemode_notices/schema.rs` | Exact v1/v2 schemas and transactional migration. |
| `mcp/notice_identity.rs` | Existing trusted recipient derivation, shared with both adapters. |
| `mcp/agent_events.rs` | Native MCP Events method adaptation and capability gate. |
| `mcp/claude_channel.rs` | Connection-bound stdio delivery and channel envelope. |

Paths in this table are relative to `crates/labby/src/`. Register new modules in
`dispatch.rs` and `mcp.rs`. Keep focused tests beside their owner and register each
test module explicitly.

The SDK fork sync is a completed separate prerequisite: fork `main` is
[`01537b5abaaaa055e439d5382a2f6c989ce2912e`](https://github.com/dinglebear-ai/rust-sdk/commit/01537b5abaaaa055e439d5382a2f6c989ce2912e),
containing upstream `5596a8a0bba2a1ec5519cc855cdadf23e4dcecdc`. Its local SDK
checks and source review passed; Labby's dependency remains unchanged.
The additional SDK capability change in Task 1 is a
companion-repository scope decision for this plan's review. Do not edit the SDK
or update Labby's dependency merely because its source is cached by Cargo.

Before Rust work, select the installed toolchain that resolves `rustc` and
`cargo` to 1.97.1. Run `bash scripts/check-rust-toolchain-sync.sh` and
`cargo metadata --offline --locked --no-deps --format-version 1`. The planning
phase verified these inputs; it did not run Labby's Rust test suite.

### Task 1: Make MCP Events discoverable in RMCP

**Files:**

- Companion modify: `dinglebear-ai/rust-sdk`, `crates/rmcp/src/model/capabilities.rs`.
- Companion test: `crates/rmcp/tests/test_events_capability.rs`.

**Interfaces:** Consumes the validated fork-sync revision. Produces
`ServerCapabilities.events: Option<JsonObject>` and
`ServerCapabilitiesBuilder::enable_events()`, serialized as top-level `events`.
The absent field remains omitted. Preserve the existing `experimental` field.

- [ ] **Step 1: Obtain plan-review approval for the named SDK companion scope.** If not approved, continue independent notice/Channels tasks and keep MCP Events explicitly blocked.
- [ ] **Step 2: Write the capability tests.** Assert that `serde_json::to_value(ServerCapabilities::default()).unwrap().get("events").is_none()` and that the enabled builder serializes `value["events"] == json!({})`. Test preservation of `experimental["claude/channel"]` beside `events`.
- [ ] **Step 3: Run the failing tests.** In the SDK checkout: `cargo test -p rmcp --features server --test test_events_capability`. Expect a missing field/builder error before implementation.
- [ ] **Step 4: Add the optional field and builder toggle.** Do not add a transport serializer shim or put this capability under `experimental`.
- [ ] **Step 5: Run the capability tests and affected SDK serialization tests.** Expect all tests executed and passing, with normal capability JSON unchanged.
- [ ] **Step 6: Commit the SDK change locally.** Stage only the two named files and use `feat(model): expose MCP Events capability`.

### Task 1a: Upgrade Labby without losing its existing native Skills surface

**Files:** Modify `Cargo.toml`, `Cargo.lock`, `scripts/ci/mcp-conformance.sh`,
the production-fork pin in `tools/verification/conformance/upstream-baseline.json`,
`crates/labby/src/mcp/server.rs`, and `crates/labby/src/mcp/skills.rs`.
Extend `crates/labby/tests/skills_mcp_e2e.rs`.

**Interfaces:** Consume Task 1's available SDK revision. Implement RMCP's
`list_skills(Option<PaginatedRequestParams>, RequestContext<RoleServer>) ->
Result<ListSkillsResult, ErrorData>`, `get_skill(GetSkillRequestParams,
RequestContext<RoleServer>) -> Result<GetSkillResult, ErrorData>`, and
`read_resource_directory(ReadResourceDirectoryRequestParams,
RequestContext<RoleServer>) -> Result<ReadResourceDirectoryResult, ErrorData>`.
These native hooks adapt to the existing shared Skills handlers and preserve
their full request context. Preserve current capability JSON, pagination,
private cache scope, manifest checks, and caller/route isolation.
Read the [fork exit assessment](rmcp-fork-assessment.md) before changing the pin.
Its current OAuth and Origin behavior must remain intact. Resolve the 16 MiB SDK
Unix JSON ceiling against Labby's 24 MiB Skills transport budget before treating
the upgrade as compatible. Preserve Labby's unknown-cache-scope contract or
make its changed handling an explicit reviewed decision.

- [ ] **Step 1: Write wire regressions before changing adapters.** Exercise all three methods through the actual RMCP service for legacy initialization and `2026-07-28` discovery. Assert unchanged payloads and errors for authorized, foreign-route, and disabled-Skills callers. A direct `on_custom_request` unit test alone cannot cover the new dispatch interception.
- [ ] **Step 2: Use a command-local Cargo path patch against the task's SDK candidate and run `cargo test -p labby --all-features --test skills_mcp_e2e`.** Expect the current custom-only handlers to fail via native dispatch. Do not commit workstation paths or unavailable Git revisions.
- [ ] **Step 3: Add the three native adapters and confirm capability negotiation.** Delegate to the existing Skills operation owner. Convert protocol DTOs with checked serialization; never drop unknown/error fields silently. Keep unrelated custom methods, including the future Events adapter, intact.
- [ ] **Step 4: Once the SDK revision is available under separately authorized publication, update all Labby fork pins and the lockfile.** Use version `=3.9.0` and the actual reviewed revision. Keep the stock-upstream conformance baseline distinct. Regenerate dependency projections using their owner.
- [ ] **Step 5: Verify and commit.** Run `cargo metadata --locked --format-version 1`, the Skills E2E target, `cargo test -p labby --all-features --lib mcp::skills`, and `just check`. Confirm the resolved fork identity and existing Skills wire behavior. Commit explicit paths as `chore(deps): adopt rmcp 3.9 with native Skills adapters`.

### Task 2: Preserve inbox ownership while adding durable subscription state

**Files:**

- Create: `crates/labby/src/mcp/notice_identity.rs`.
- Create: `crates/labby/src/dispatch/agent_events/{mod.rs,types.rs,subscriptions.rs,tests.rs}`.
- Create: `crates/labby/src/dispatch/codemode_notices/schema.rs`.
- Modify: `crates/labby/src/dispatch/codemode_notices.rs`, its `tests.rs`, `crates/labby/src/mcp/call_tool_codemode/notices.rs`, `crates/labby/src/dispatch.rs`, `crates/labby/src/mcp.rs`.

**Interfaces:** Extract the existing recipient calculation as
`notice_recipient(&LabMcpServer, &RequestContext<RoleServer>) -> Option<NoticeRecipient>`.
Define `EventOwnerBinding` from that recipient plus server-captured authority,
never deserialized request identity. Define `EventAuthorityDecision` as
`Current | Revoked | RefreshRequired`, and an `EventAuthorizer::check` async
interface with signature `check<'a>(&'a self, owner: &'a EventOwnerBinding) ->
BoxFuture<'a, EventAuthorityDecision>`. Use existing identity/access sources; retain
no bearer tokens. `AgentEvents::new(NoticeStore, authorizer, sender)` composes the
later adapters.

`EventOwnerBinding` contains the trusted `NoticeRecipient` and a server-minted
authority-registry key; it has no request-JSON constructor. `EventAuthority`
stores only selected verified identity, client/credential, route and grant facts
inside that registry. Restoring persisted subscription rows never constructs a
verified identity: absent live registry/revalidation returns `RefreshRequired`.
Use `Arc<dyn EventAuthorizer>` and `Arc<dyn WebhookSender>` at the service boundary.
Define `EventError` in `types.rs` with typed notice-store, invalid-params,
unsupported-mode, callback-category, payload-size, and transport-category cases;
the MCP adapter maps them to protocol errors without endpoint/body leakage.

Storage APIs on `NoticeStore` are `activate_event_subscription(owner, validated,
now_ms) -> Result<EventSubscription, NoticeError>`,
`remove_event_subscription(owner, identity, now_ms) -> Result<(), NoticeError>`,
and the claim/finish APIs specified in Task 5. `ValidatedSubscription` contains
the checked callback, canonical arguments, protected signing keys, finite expiry,
and proposed generation. Test-only `EventFixture::new()` supplies two distinct
recipients, a temporary store, and an explicit clock. Its `publish(recipient,
source, key)` returns the existing `PublishReceipt`.

- [ ] **Step 1: Write storage and isolation regressions.** `v1_migration_preserves_notice_state` seeds v1 IDs, dedupe keys, ACKs and leases, reopens as v2, and asserts exact equality. `foreign_subscription_is_not_visible` asserts a foreign owner gets the same not-found result as an absent ID. `failed_v1_migration_is_atomic` asserts original schema/version/rows survive a migration failure.
- [ ] **Step 2: Run the failing tests.** `cargo test -p labby --all-features --lib agent_events`. Expect missing APIs or the missing v2 migration. Do not count zero selected tests as a result.
- [ ] **Step 3: Implement the shared types and migration.** Retain the existing application ID and exact v1 validation. Add the two spec tables and one nullable serialized-event-body column on notices in a single migration transaction. Existing v1 notices have no queued webhook history. Expose the existing bounded DB executor only to sibling dispatch code; do not create another connection manager.
- [ ] **Step 4: Extract recipient derivation without changing its output.** Test OAuth/client, product/static credential, Team/route rebinding, optional conversation metadata, and fresh stdio identities. Add the production authorizer using the existing verified identity/access runtime; make unavailable revalidation return `RefreshRequired`.
- [ ] **Step 5: Add quota and canonicalization assertions.** An eighth subscription succeeds, a ninth fails; 2,001 global subscriptions fail; changed JSON key order preserves the same subscription ID. Duplicate notice publication creates one delivery row. Preserve all `notice_production_` regressions.
- [ ] **Step 6: Verify and commit.** Run `cargo test -p labby --all-features --lib agent_events` and `cargo test -p labby --all-features --lib notice_production_`. Expect no failures. Commit explicit changed paths as `feat(notifications): persist event delivery subscriptions`.

### Task 3: Implement bounded, signed HTTPS callback delivery

**Files:**

- Create: `crates/labby/src/dispatch/agent_events/webhook.rs` and `webhook_tests.rs`.
- Modify: `crates/labby/src/dispatch/agent_events/types.rs` and `mod.rs`.
- Reuse without copying policy: `crates/labby-primitives/src/ssrf.rs`.

**Interfaces:** `WebhookSecret::parse(&str) -> Result<WebhookSecret, EventError>`
has redacted `Debug`. `sign_webhook(secret, event_id, unix_seconds, body: &[u8])
-> String` returns the Standard Webhooks signature header. `WebhookSender` exposes
async `verify(destination, subscription_id, secret) -> Result<(), EventError>`
and `send(claim: &EventDeliveryClaim) -> DeliveryOutcome`. Its production
implementation pins validated DNS addresses and uses the existing reqwest/rustls
and SSRF primitives. Tests inject a resolver/transport, never a production
private-address bypass.
Use boxed futures on `WebhookSender` so its `Arc<dyn WebhookSender>` service
boundary remains object-safe. The concrete production sender owns its resolver
and HTTPS client; fixtures inject that same interface.

- [ ] **Step 1: Write the fixed signing-vector assertion.** For key bytes `[b'a'; 32]`, ID `notice_vector`, signing time `1700000000`, and body `b"{\"message\":\"ready\"}"`, assert the header equals `v1,4373CS2hzQF5XAy4/Pzw5DW/XXg1BLfOJ0bwaI6ALss=`. Assert a changed byte/time/ID fails independent verification.
- [ ] **Step 2: Write the network-boundary tests.** Cover private/mixed DNS answers, IPv4-mapped IPv6, redirects to private addresses, userinfo, fragments, ambient proxy settings, a 2,049-byte callback URL, oversized challenge responses, and a failed challenge echo. Assert zero payload requests for rejected destinations and no activation after failed verification. Repeated/concurrent equivalent subscriptions share a successful 300,000-ms verification cache entry; a new signing key never reuses it.
- [ ] **Step 3: Run the failing tests.** `cargo test -p labby --all-features --lib agent_events::webhook`. Expect missing transport/signing behavior.
- [ ] **Step 4: Implement verification and delivery.** Use the spec's 10/5/5-second deadlines, 8,192-byte verification limit, 262,144-byte outbound limit, exact headers, and 24–64-byte secret validation. Sign the verification control envelope and require its exact JSON echo before activation. Reuse the once-serialized event bytes. Disable redirects/proxies and preserve TLS hostname verification. Do not log URLs, secrets, challenge bodies, or payloads.
- [ ] **Step 5: Verify and commit.** Run the webhook tests, including a controlled real HTTPS exchange through the test-only connector. Expect the independent receiver to verify the exact bytes. Commit as `feat(notifications): verify and sign MCP event webhooks`.

### Task 4: Expose native MCP Events methods behind an explicit HTTP flag

**Files:**

- Create: `crates/labby/src/mcp/agent_events.rs` and `agent_events/tests.rs`.
- Modify: `crates/labby/src/mcp/server.rs`, `runtime.rs`, `crates/labby/src/api/state.rs`, `crates/labby/src/cli/serve.rs`, and `crates/labby/src/mcp.rs`.
- Update contract: `docs/dev/AGENT_NOTIFICATIONS.md`.

**Interfaces:** Define `EventsListParams`, `EventsSubscribeParams`,
`EventsUnsubscribeParams`, and their protocol results in Task 2's `types.rs`.
The DTO fields follow the spec's wire contract; use
`RequestedTtl::Default | Millis(u64) | NoExpiryRequested` so omitted and null
remain distinct. Event arguments contain only optional `source`; normal `_meta`
is accepted outside that argument object. Unsubscribe accepts the identity tuple,
never a caller-supplied owner or a bare subscription ID.
`AgentEvents::list(owner)`, `subscribe(owner, params)`, and `unsubscribe(owner,
params)` return typed results/errors. `handle_agent_event_request(server,
request, context) -> Result<CustomResult, ErrorData>` is an MCP-only adapter.
`events_visible(server) -> bool` controls capability advertisement using the
HTTP/gateway/store/flag/route gates. `--mcp-events` defaults to false.

- [ ] **Step 1: Write native request tests.** Assert enabled `server/discover` and legacy initialization expose top-level `events: {}`; disabled/unsupported routes omit it. Assert `events/list` advertises only `labby.notification.created` and webhook delivery. Retain existing Skills/custom-request tests.
- [ ] **Step 2: Write authorization and lifecycle tests.** Use a test-local `McpEventHarness::rpc_as(owner, method, params)` over the real MCP service. Assert unauthorized subscription fails before callback I/O; an argument cannot override owner/inbox; refresh preserves ID and increments generation; unsubscribe is idempotent and owner-bound.
- [ ] **Step 3: Run the failing tests.** `cargo test -p labby --all-features --lib mcp::agent_events`. Expect method/capability/flag failures.
- [ ] **Step 4: Implement the shared operations and thin adapter.** Register the authenticated recipient's inbox. Test omitted TTL gives 3,600,000 ms, requested `1` gives 60,000 ms, and requested `86,400,001` or null gives 86,400,000 ms; zero/negative fail. Return a finite RFC 3339 `refreshBefore`, `cursor: null`, and `truncated: false`. Reject non-null replay cursors and unsupported delivery modes. Verify the callback before activation and use the ChatGPT profile's `-32015` with a bounded reason for callback errors. Preserve `_meta` handling through the existing SDK request context.
- [ ] **Step 5: Pin refresh races.** A failed key verification leaves the old subscription intact. Canonical-equivalent refresh is idempotent. During key overlap an independent receiver verifies either space-separated signature; old-key overlap ends after 300,000 ms. Foreign route/Team/client bindings never receive old payloads.
- [ ] **Step 6: Verify documentation/discovery and commit.** Run the MCP tests plus `cargo test -p labby --all-features --lib cli::`. Update the current contract and generated CLI help through the repository generator. Commit as `feat(mcp): expose authenticated event subscriptions`.

### Task 5: Join publication to a cancellable delivery worker

**Files:**

- Create: `crates/labby/src/dispatch/agent_events/worker.rs` and `worker_tests.rs`.
- Modify: `crates/labby/src/dispatch/codemode_notices.rs`, `agent_events/subscriptions.rs`, `crates/labby/src/cli/serve.rs`, and `crates/labby/src/api/state.rs`.
- Extend: `crates/labby/src/testkit/notification_http.rs`.

**Interfaces:** `NoticeStore::claim_event_delivery(eligible_authority_keys: &[String], now_ms, lease_ms) ->
Result<Option<EventDeliveryClaim>, NoticeError>` creates a unique claim token.
`finish_event_delivery(claim_token, generation, outcome, now_ms) -> Result<(),
NoticeError>` fences stale completions. `run_webhook_worker(events, cancel) ->
Result<(), EventError>` owns and joins its children. `DeliveryOutcome` is
`Received | Retryable { retry_after_ms } | Permanent | Gone | Oversized`.
It is never an agent ACK.
`EventDeliveryClaim` carries subscription ID, generation, claim token, owner
binding, validated destination, notice/event ID, persisted body bytes, active
signing keys, attempt number and expiry. Define it in Task 2's `types.rs`;
its `Debug` redacts destination, keys and body. The durable claim token and
generation are the only accepted completion authority.

- [ ] **Step 1: Write durable delivery tests.** Assert publish plus enqueue is atomic, publication dedupe does not enqueue twice, and a reclaimed lease keeps the same event ID/body. Assert an HTTP `2xx` leaves the notice unacknowledged and a later valid recipient ACK cancels retries.
- [ ] **Step 2: Write boundary tests.** Assert at most eight attempts, one in-flight request per subscription, eight globally across independent store handles, and no attempts after expiry/revocation/unsubscribe. A changed generation cannot apply an old completion. An unknown authority pauses delivery, including after reopen, until revalidated or refreshed, without consuming an attempt. A second process with no matching registry key cannot claim or suspend another process's deliveries.
- [ ] **Step 3: Run the failing tests.** `cargo test -p labby --all-features --lib agent_events::worker`. Expect missing enqueue/claim/worker behavior.
- [ ] **Step 4: Implement the publication hook and worker.** Enqueue matching active subscriptions inside `publish_at`'s transaction. Keep pull leases separate. Use a 30-second delivery lease and the spec's seven retry delays with an eight-attempt ceiling. Honor bounded `Retry-After`; terminate `413` deliveries and disable `410` subscriptions. Record only IDs, outcome categories, counts, and elapsed time.
- [ ] **Step 5: Verify cross-process and shutdown behavior.** Open two independent store handles, publish through one, and assert the worker on the other observes the committed notice within its 1,000-ms check interval. Cancel the service and assert all requests/worker tasks end and are joined.
- [ ] **Step 6: Verify and commit.** Run worker and `notice_production_` tests, plus the actual HTTP publication adapter tests. Commit as `feat(notifications): deliver queued events with bounded retries`.

### Task 6: Add opt-in Claude Code Channels to the existing stdio server

**Files:**

- Create: `crates/labby/src/mcp/claude_channel.rs` and `claude_channel/tests.rs`.
- Modify: `crates/labby/src/cli/serve.rs`, `crates/labby/src/mcp/server.rs`, `runtime.rs`, `mcp.rs`, and `notice_identity.rs`.
- Update authored guidance: `plugins/labby/.apm/skills/using-codemode/SKILL.md`, its `references/code-mode.md`, and `docs/dev/AGENT_NOTIFICATIONS.md`.

**Interfaces:** `--claude-channel` defaults to false on `labby mcp`.
`stdio_notice_recipient(&LabMcpServer) -> Option<NoticeRecipient>` uses the same
server-owned connection identity as Code Mode. `channel_notification(&CodeModeNotice)
-> CustomNotification` constructs the documented payload.
`run_claude_channel(store, recipient, peer, cancel) -> Result<(), EventError>`
uses the existing `NoticeStore::deliver` lease and returns only after its worker ends.

- [ ] **Step 1: Write CLI/capability tests.** The default CLI and HTTP routes expose no channel capability. Enabled stdio exposes `experimental["claude/channel"] == {}` and never exposes permission relay. Unsupported feature/transport combinations fail before starting a worker.
- [ ] **Step 2: Write the real stdio wire regression.** Initialize an owned stdio server with channel mode enabled, register its inbox, publish a notice through a separate store handle, and read `notifications/claude/channel` without making another Code Mode call. Assert the exact ID, content and five string metadata fields. A second connection receives none of the first connection's mail.
- [ ] **Step 3: Run the failing tests.** `cargo test -p labby --all-features --lib mcp::claude_channel`. Expect missing flag/capability/unsolicited delivery behavior.
- [ ] **Step 4: Implement connection-scoped delivery.** Start only after initialization. Use a signal plus the 1,000-ms durable-store check. Reuse the normal lease and ACK semantics. Do not send on a closed peer or transfer its identity on reconnect. On write failure retain the notice for the normal retry/expiry policy.
- [ ] **Step 5: Verify ACK and cancellation.** Successful write leaves the notice unacknowledged; an ordinary Code Mode ACK for the offered ID succeeds; a foreign ACK fails. Cancel/drop the peer and assert its worker ends without residual tasks or socket listeners.
- [ ] **Step 6: Update authored client guidance and regenerate projections.** Document the inbox address, dedupe/ACK flow, stdio-only requirement, client opt-in, preview limitations and no permission relay. Run `python3 scripts/generate-native-plugins.py` and `--check`. Do not hand-edit generated client packages or installed plugins.
- [ ] **Step 7: Verify and commit.** Run channel tests, `cargo test -p labby --lib cli:: --all-features`, `cargo test -p labby --lib entrypoint:: --all-features`, and `cargo test -p labby --test cli_contract --all-features`. Commit as `feat(mcp): deliver agent notices through Claude Channels`.

### Task 7: Qualify both protocols and record remaining client gates

**Files:**

- Create: `crates/labby/tests/agent_event_delivery.rs`.
- Extend current contracts: `docs/dev/AGENT_NOTIFICATIONS.md`, `docs/surfaces/MCP_CONFORMANCE.md`.
- Generate: current CLI/schema docs and plugin client projections through their owners.

**Interfaces:** The native fixture launches the task's built binary under a
private `LABBY_HOME`, uses generated test credentials without printing them,
owns every process, and enforces a 300-second deadline. Record actual application
acceptance separately from local protocol fixtures.

- [ ] **Step 1: Add end-to-end lifecycle assertions.** Cover registration, one matching and one filtered-out publication, stable IDs across a retry/restart, ACK, refresh, unsubscribe, and no late delivery. Exercise real HTTP auth and stdio frames. Test-only callback injection must not be available in the production CLI.
- [ ] **Step 2: Run repository qualification.** Run `just check`, `just test`, `just lint`, `bash scripts/check-feature-slices.sh`, `just docs-generate`, `just docs-check`, and `python3 scripts/generate-native-plugins.py --check` with the pinned toolchain and private build output. Do not repeat costly checks after they pass unless the candidate changes.
- [ ] **Step 3: Exercise supported native clients when the test installation and connection refresh are authorized.** Use the spec's one-session/one-subscription budget, two synthetic notices, four-webhook-POST ceiling, and 300-second deadline. Require an observed agent response containing the notice ID. A callback `2xx`, SDK fixture, or successful stdio write does not satisfy this step.
- [ ] **Step 4: Record the exact evidence and clean up.** Bind every receipt to the source and built binary, client version/mode, subscription, notice ID, and observed response. Unsubscribe and join owned processes. Preserve unsupported/unauthorized native checks as unverified, without weakening local assertions.
- [ ] **Step 5: Review generated diffs and commit the acceptance work.** Stage explicit task files only. Use `test(notifications): qualify MCP Events and Claude Channels`. Leave publication, releases, deployment, and PR merges to their separately authorized flows.

## Plan self-review

The tasks cover both requested protocols, their existing notice source, the
SDK discovery prerequisite and the native Skills compatibility gate introduced
by upstream 3.9. The five Review Focus cases have owning tests.
Storage and recipient interfaces are shared; protocol DTOs and envelopes remain
separate. The SDK companion scope, finite TTL policy, schema-v2 rollback,
restart authority pause, and native-client gates require review before execution.
No scheduler, watcher, Codex adapter, new daemon, new database, or UI is included.
