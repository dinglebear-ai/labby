# Agent Session Messaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Register Labby sessions and support direct and all-active messages with long private bodies and existing notice delivery.

**Architecture:** Use the existing SQLite inbox connection and lifecycle. Store one immutable body, one notice reference per recipient, and one stable send receipt in the same transaction. Adapt trusted HTTP and stdio caller bindings to a single shared message service; reuse existing push workers for references.

**Tech Stack:** Rust 1.97.1, Tokio, rusqlite, current RMCP fork, serde/serde_json, SHA-256 and ULID already in Labby.

**Spec:** [Labby session messaging design](session-messaging-design.md).

## Global Constraints

- Continue the isolated Labby worktree at initial messaging base `478077702b1ee466b1a9f6fc7393fda74b132a89`; publish one feature branch against live `dinglebear-ai/labby:main`, including the unpublished delivery and RMCP prerequisites.
- Preserve the 384-byte notice text, three-notice / 1,024-byte pull limits, existing per-inbox and global notice limits, event receiver bounds, and v2 ownership/lease behavior.
- Maximums: 16,384 UTF-8 bytes per body, 160 bytes per optional summary, 64 bytes per nonempty label, 128 bytes per idempotency key, 32 retained sessions per owner and 512 total, 64 retained messages per owner and 256 total, 24-hour message/receipt retention, 128 KiB for a complete serialized message tool result.
- Local stdio renews every 30 seconds with a 120-second lease; HTTP activity lease is five minutes. Registration expires after 24 hours without renewal, except while retained by an unexpired message, receipt, or subscription. An HTTP callback row alone never counts as active.
- Use Rust 1.97.1 with the existing task's installed native dependencies and isolated Cargo target. Preserve current dependency pins. No new scheduler, installed client change, external alerts or production deployment.
- No raw private session header in storage, tool outputs, telemetry, upstream forwarding, or docs examples. Sender and recipient authority never come from action JSON.
- Keep business validation/storage in dispatch, MCP transport identity and projection in MCP; no second delivery engine.
- User requested a plan, independent adversarial review and corrections, inline execution, full review/fixes, branch push, and a Labby PR. Do not merge or modify `main`.

## Review Focus

- Proof confusion: same user with distinct sessions, shared auth with no session evidence, foreign same-named identity, relayed tool metadata, and wrong installation cannot bind an inbox or sender (Tasks 2–4).
- Replay and resource pressure: one target at quota, webhook enqueue failure, or uncertain deadline never produces partial fanout or changes the receipt (Tasks 1–2).
- Reconnect and availability: idle live stdio participates; clean close and stale leases cannot inherit messages or remove another registration. Expired/revoked events cannot deliver (Tasks 2–4).
- Payload expansion and reading: escaped multi-line UTF-8 stays within a full result budget; a guessed real ID is unreadable to the wrong session (Tasks 2–3).
- Compatibility: new SQL, event filters, tool discovery and ACKs preserve legacy notices, IDs, offer witnesses, route authority and absence of the opt-in tool (Tasks 1, 3–5).

---

## Source map

| File | Responsibility |
| --- | --- |
| `crates/labby/src/dispatch/codemode_notices/schema.rs` | Version 3 exact schema and atomic upgrades from empty, v1 or v2 stores. |
| `crates/labby/src/dispatch/codemode_notices.rs` | Existing DB executor, transaction-local notice insertion, optional message reference and pruning. |
| `crates/labby/src/dispatch/agent_messages.rs` and `agent_messages/{store,types,tests}.rs` | Proof-bound session operations, durable send/fanout, ACL, read/unread/ACK, typed input and quotas. |
| `crates/labby/src/mcp/session_messages.rs` and `session_messages/{authority,tests}.rs` | Verified request proof, current authority and thin MCP tool adaptation. |
| `crates/labby/src/dispatch/agent_messages/proof.rs`, `api/session_messages.rs`, `live_gateway.rs` | Protected installation key, domain-separated bridge attestation, private TCP adapter, and bound credential client. |
| `crates/labby/src/mcp/{permanent_tools,handlers_tools,peer_contract,call_tool,server,runtime,bridge}.rs` | Shared descriptor, policy-consistent projection, invocation, and per-connection lifecycle. |
| `crates/labby/src/dispatch/agent_events/{types,worker}.rs`, `mcp/{agent_events,claude_channel}.rs` | Opt-in HTTP session inbox subscription and typed message reference through existing push adapters. |
| `crates/labby/src/{cli/serve,entrypoint}.rs` | Opt-in flags and authoritative local bridge/standalone setup. |
| `crates/labby/tests/architecture_orchestrator.rs` | Classify the shared subsystem without exempting new cross-service imports. |
| `crates/labby/tests/agent_session_messaging.rs` | Listening HTTP/stdio lifecycle acceptance with temporary Labby installations. |
| `docs/dev/AGENT_NOTIFICATIONS.md`, `docs/surfaces/MCP.md`, generated CLI/catalog projections | User-facing current behavior and flags. |

## Adversarial review decisions

Three independent lanes checked the first draft against `478077702`: session
authority and exposure, transactional storage and leases, and native/fixture
feasibility. The final contract requires private HTTP proof instead of chat
metadata, an authenticated and refreshed local bridge, real TCP gating,
current OAuth/project authorization, and rotation isolation (Task 3).
It distinguishes HTTP activity from durable subscriptions, fences activation
after verification, reconciles late commits, partitions typed and ordinary
payloads, and defines receipt/lease/ACK/retention ordering (Tasks 1, 2, 4).
It also separates controlled TLS in-process proof, exact-binary CLI/stdio
evidence, and optional native client acceptance (Task 5).

### Task 1: Upgrade the inbox and share one transaction for references

**Files:** Modify `dispatch/codemode_notices.rs`, `dispatch/codemode_notices/{schema,tests}.rs`, and `dispatch/agent_events/worker.rs`. Paths below are relative to `crates/labby/src/` unless specified.

**Interfaces:** Extend `NoticeConsumer` with `MessageSession { binding: String }` without changing serialization for existing variants. Add `CodeModeNotice.message_id: Option<String>`, omitted for ordinary notices. Extract `pub(super) fn insert_notice_in(tx: &Connection, actor: &str, input: &PublishNotice, now: i64, message_id: Option<&str>) -> Result<PublishReceipt, NoticeError>`. It performs validation, quotas, insertion, expiry and enqueue; it never opens or commits a transaction. Existing `publish_at` owns its transaction and calls it with `None`. Pass `message_id` into `agent_events::enqueue` and include it in signed data only when set. Add a batch capacity preflight for one distinct recipient set; duplicates are resolved before capacity checks.

- [ ] **Step 1: Write migration and delivery compatibility regressions.** `v2_migration_preserves_subscriptions_and_claims` seeds v2 inbox, notice, receipt, subscription generation, claimed outbox and witness and asserts equal rows on v3 open. `failed_v2_migration_is_atomic` limits page count and proves schema/version/rows remain v2 on failure. Keep v1, future-version, schema tampering, duplicate and old JSON tests. `typed_message_reference_fits_notice_budget` measures the final serialized notice with reference and worst metadata/escaping against 1,024 bytes. `typed_message_reference_requires_internal_publish` rejects an external `message_id` key and checks omission on ordinary notices. `message_schema_retains_live_dependencies` rejects duplicate (inbox,message) or dangling body links and keeps a live subscription or retained session during cleanup.
- [ ] **Step 2: Observe RED.** `cargo test -p labby --all-features --locked --lib v2_migration`; expect the existing version or missing schema assertion to fail.
- [ ] **Step 3: Add schema and helper.** Create `agent_message_sessions(id,owner_key,binding_key,inbox_id,label,kind,last_seen,active_until,retained_until,closed)` with NOT NULL binding/owner/inbox, unique binding and inbox, and explicit inbox FK, and `agent_messages(id,owner_key,sender_id,sender_label,body,summary,request_digest,idempotency_key,recipient_ids,created,expires)` with unique sender/key. Add nullable `agent_notices.message_id` referencing the body and a partial unique index on (inbox_id,message_id). Persist `agent_event_subscriptions.session_messages INTEGER NOT NULL DEFAULT 0 CHECK(session_messages IN (0,1))`. Allow a nullable idempotency key so expired rows can release it without deleting leased references. Compare expected schema for each incoming version before migration; prune refs, bodies, registrations then inboxes, preserving leases and retained dependencies. Map SQLITE_FULL to capacity failure. Expose transaction-local register/insert/prune helpers only to shared dispatch; an ACKed notice remains readable until expiry.
- [ ] **Step 4: Observe GREEN.** `cargo test -p labby --all-features --locked --lib codemode_notices` and `cargo test -p labby --all-features --locked --lib agent_events`; no failures, including existing tests and both upgrade paths.
- [ ] **Step 5: Commit.** Stage explicit Task 1 paths; `git commit -m 'feat(notifications): persist message references in the inbox'`.

### Task 2: Implement scoped sessions, atomic sends, and full-body retrieval

**Files:** Create `dispatch/agent_messages.rs`, `dispatch/agent_messages/{store,types,tests}.rs`; modify `dispatch.rs` and `crates/labby/tests/architecture_orchestrator.rs`.

**Interfaces:** `SessionBinding { owner_key: String, binding_key: String, recipient: NoticeRecipient, kind: SessionKind }` is server-owned and never deserialized. `SessionKind::{Http,Stdio}`. `MessageRequest` is the typed `{action,params}` union from the design. Implement `impl NoticeStore { async fn message_request(&self, binding: SessionBinding, request: MessageRequest) -> Result<Value, NoticeError>; async fn renew_message_session(&self, binding: SessionBinding, close: bool) -> Result<(), NoticeError>; async fn registered_message_recipient(&self, binding: SessionBinding) -> Result<MessageEnrollment, NoticeError>; }`. The latter returns the exact session ID, binding, owner and inbox identity, without creating or rebinding a session. Name pure transaction operations `execute_in(tx, binding, request, now_ms)` for deterministic tests.

- [ ] **Step 1: Write behavior tests.** `register_is_stable_for_exact_binding`: same binding gets same ID, same label with different binding gets different ID. `broadcast_snapshots_my_active_sessions_atomically`: include active sender and its other active sessions; exclude expired, foreign, inactive, closed and callback-only stale rows. `direct_recipients_own_body_and_ack`: Unicode/newline-heavy max body returns byte-for-byte only to direct target, reading does not ACK, own ACK stops just its notice, wrong session/owner/guessed ID fail equally. `retry_keeps_original_targets`: same key after presence changes returns original IDs, mutated body/target/summary conflicts; include concurrent independent DB connections and same-proof reconnect/restart after a lost response. `logical_expiry_releases_key_while_lease_pins_body` checks exact expiry and new incarnation isolation. `failed_broadcast_has_no_side_effects`: prefill one inbox to 32, abort second enqueue, and induce actual SQLite page exhaustion; assert no body, notice, receipt or outbox partial writes. `limits_cover_serialized_tool_result`: exactly 16,384 bytes valid, +1 rejected, complete escaping-heavy response bounded, owner/global quotas, expiry and cleanup. `close_fences_stdio_renewal`: a close cannot be undone by a stale tick or late request.
- [ ] **Step 2: Observe RED.** `cargo test -p labby --all-features --locked --lib agent_messages`; expect missing dispatch operations or failing behaviors, never zero selected tests.
- [ ] **Step 3: Implement the operations.** Validate inputs before transactions. Resolve and update only the current binding under the same immediate transaction used for activity, snapshot, dedupe, notice insertion, and receipt commit. Persist the complete bounded original recipient IDs; source of internal refs is `labby.messages`. Define publication wake inside the bounded DB worker after successful commit. Refresh sender activity before snapshot; keep exact sender/targets addressable through receipt/message expiry without making inactive targets active. Prune once and preflight all notice quotas in a set; preserve one body and constant-size digest. Every read/ACK joins the exact session's inbox and message reference and filters expiry. Explicit ACK needs no transport offer but shares the pending-outbox stop logic; live claims retain their token. Keep legacy ACK rules untouched. Scope the new module as a shared dispatch subsystem.
- [ ] **Step 4: Observe GREEN.** `cargo test -p labby --all-features --locked --lib agent_messages`; run `cargo nextest run -p labby --all-features --locked --test architecture_orchestrator`. All named cases pass.
- [ ] **Step 5: Commit.** Stage explicit Task 2 paths; `git commit -m 'feat(messages): add durable session-scoped messaging'`.

### Task 3: Expose one MCP tool with verified HTTP and stdio bindings

**Files:** Create `mcp/session_messages.rs`, `mcp/session_messages/{authority,tests}.rs`, `dispatch/agent_messages/proof.rs` and `api/session_messages.rs`; modify `mcp.rs`, `mcp/{permanent_tools,handlers_tools,peer_contract,call_tool,server,runtime,bridge}.rs`, `api.rs`, `api/router.rs`, `live_gateway.rs`, `cli/serve.rs`, and `entrypoint.rs`. Preserve other tool projections.

**Interfaces:** `async fn http_binding(server: &LabMcpServer, context: &RequestContext<RoleServer>) -> Result<SessionBinding, NoticeError>` rechecks existing event authority using current verified root identity. Separate synchronous descriptor eligibility from async admission. `MessageKey` loads the private installation key; derive static owner and HTTP session IDs with distinct HMAC domains. `LiveGateway::prove_message_bridge(&self, key: &MessageKey, installation: &str) -> Result<BridgeProof, ToolError>` uses the selected bearer, a fresh nonce, and a bounded same-installation loopback RPC. `LocalMessageSession` owns `NoticeStore`, `SessionBinding`, initialization, close fence and optional fresh bridge authorization. `async fn wait<S: Service<RoleServer>>(&self, running: RunningService<RoleServer,S>) -> Result<(), JoinError>` owns renewal/close alongside optional channel push. Reuse the channel recipient or its standalone notification session. Share one descriptor builder and context-derived eligibility for listing, peer hash and invocation.

- [ ] **Step 1: Write identity and route tests.** `message_tool_requires_exact_session_proof` rejects missing/invalid/duplicate header, copied metadata, unauthenticated/read-only/protected/Unix/browser/delegated calls, and forged sender; prove independently provisioned keys. `verified_owner_groups_only_current_realm` tests different provider/same subject, current OAuth consent revoked with unexpired JWT, rotated static bearer against retained DB and same key, and current/denied product scope, project, generation and credential bindings. `local_message_bridge_requires_authenticated_installation` rejects remote/wrong installation, fake loopback with copied public binding, wrong bearer or replayed nonce, revoke/rotate after initialization and daemon epoch changes. `tool_and_hash_agree_on_message_visibility` covers opt-in/default, router/atomic/combined, product terminal dispatch and aggregate/qualified fallback. CLI contract tests assert flags and unsupported listeners.
- [ ] **Step 2: Observe RED.** `cargo test -p labby --all-features --locked --lib session_messages`; expect missing binding or dispatch.
- [ ] **Step 3: Implement opt-in and adapters.** Gate actual TCP listener, durable DB, active authentication and current root policy. Reserve the exact name independently of visibility, mark the descriptor transport-local and reject forwarding including qualified Labby aliases; never route a hidden local name upstream. Place the product exception after the exact bound project check but before terminal dispatch. Scrub the secret header from forwarding. The bridge proof endpoint verifies literal-loopback peer, installation, timestamp/nonce, request MAC bound to actual verified bearer and current owner, then returns installation/nonce/owner/epoch/expiry under a reply MAC also bound to the selected bearer. Mount only on actual TCP, verify trusted socket peer and use bounded constant-time verification. Bridge uses its verified selected bearer, validates before calls and requires a proof at most five seconds old for typed pushes; use a short cached proof only for presence/push, not read/send. Never transmit body text during proof. Close on auth/epoch change, prevent stale renewals and keep ordinary notices unchanged.
- [ ] **Step 4: Observe GREEN.** `cargo test -p labby --all-features --locked --lib session_messages`, `cargo nextest run -p labby --all-features --locked --test cli_contract`, plus peer contract and bridge tests. Assert complete serialized `CallToolResult` size including JSON escapes and any text projection for a maximum body; admission checks the same view's bounded encoding.
- [ ] **Step 5: Commit.** Stage explicit Task 3 paths; `git commit -m 'feat(mcp): bind message tools to authenticated sessions'`.

### Task 4: Reuse push adapters and retain independent acknowledgments

**Files:** Modify `dispatch/agent_events.rs`, `dispatch/agent_events/{types,worker,subscriptions,tests}.rs`, `mcp/{agent_events,claude_channel}.rs`, and local lifecycle glue.

**Interfaces:** Add optional default-false `EventFilter.session_messages`, omitted in canonical identity when false and enforced by stored flag. A true flag carries the Task 2 `MessageEnrollment` through callback verification to final activation; no generic register. Existing signed data and channel formatter add `message_id` only for a typed reference. Extend `AgentEvents` activation with a completion callback run after the actual transaction result so a caller response deadline cannot strand a newly committed authority.

- [ ] **Step 1: Write adapter regressions.** `message_subscription_requires_same_proof_and_enrollment`: missing/wrong proof, raw metadata, unknown/expired/replaced enrollment during TLS challenge, revoke before the final grant check, and disabled flags fail. `message_subscription_filters_typed_references`: normal admin notice cannot reach message callback, typed pointer cannot reach ordinary callback; actual signed data and closed schema agree and omit full body. `message_ack_is_independent_of_webhook_receipt`: 2xx leaves unread, exact ACK before offer/while leased/after receipt stops only own pending rows, late completion remains fenced. `late_subscription_commit_keeps_current_authority`: forced response deadline followed by commit and rollback exercise both dispositions. `claude_channel_delivers_reference_to_its_peer`: unsolicited reference only to target, read exact body and sender, stop message refs on expired bridge proof without dropping old notice behavior. `normal_event_subscription_identity_unchanged` compares v2 canonical ID/payload.
- [ ] **Step 2: Observe RED.** `cargo test -p labby --all-features --locked --lib message_subscription` and `cargo test -p labby --all-features --locked --lib claude_channel_delivers_reference`; expect missing binding/filter/reference behavior.
- [ ] **Step 3: Implement reference delivery.** Recheck current authorization after verification and exact registration/binding/inbox inside the activation transaction; a retired row cannot be recreated. Retain the registration through grant expiry and renew recent activity at successful subscribe/refresh, not from SQL callback state alone. Finalize old/new authority from actual commit/rollback inside the blocking worker before releasing the DB permit (or fence by generation); a timeout is unknown until completion. Delivery rechecks grant even if it changed after final subscribe admission. Enforce typed filter in enqueue and update payload schema. Add opt-in Claude instructions for advisory reference read/ACK. Existing signing, retries and offer semantics remain shared.
- [ ] **Step 4: Observe GREEN.** `cargo test -p labby --all-features --locked --lib agent_events` and `cargo test -p labby --all-features --locked --lib claude_channel`; existing cancellation, early ACK and cross-peer tests remain green.
- [ ] **Step 5: Commit.** Stage explicit Task 4 paths; `git commit -m 'feat(messages): deliver session references through Events and Channels'`.

### Task 5: Qualify the feature and make it discoverable

**Files:** Create `crates/labby/tests/agent_session_messaging.rs` and focused test helpers only if needed; modify `docs/dev/AGENT_NOTIFICATIONS.md`, `docs/surfaces/MCP.md`, and actual generated projections from their owner. Use existing disposable-process harness `crates/labby/tests/support/live_labby.rs`.

**Interfaces:** CLI flags and MCP wire shape from Tasks 3–4. Test against the current built artifact in a private Labby installation; use separate independent client connections and bind receipt to the branch/tree and binary digest.

- [ ] **Step 1: Add live acceptance.** `session_messages_live_http_and_stdio`: start real loopback daemon, authenticate distinct HTTP sessions and two stdio clients, register and discover only own sessions, send long message direct, deliver only to chosen peer, broadcast to all active including sender, retain exact receipt/body across reopen/restart for stable HTTP proof, verify no cross-session access or stale stdio resurrection. Cover bridged and standalone stdio, one connection idle then clean exit, default/no flag, and negative shared identity. Separately bind the in-process production MCP router to a real loopback socket and use the existing cfg(test)-only HTTPS receiver connector in `dispatch/agent_events/tests.rs::message_session_over_real_http_and_https`. A spawned CLI binary cannot inject it. Exercise real HTTP subscribe, repeated subscribe as refresh, unsubscribe, and exact signed pointer. The actual CLI binary must reject a private callback with its production connector. Test old notice send/pull/ACK and v2 compatibility separately.
- [ ] **Step 2: Prove the wire tests.** `cargo nextest run -p labby --all-features --locked --test agent_session_messaging`; run delivery/identity/storage/legacy targeted suites and real runner tests with pinned Rust. Expected no new failures. Keep real ChatGPT/Claude acceptance separate. A native ChatGPT Work chat must use the same private proof for tools/subscriptions and read its own message ID/body, with sibling negative coverage. Custom Claude Channels require interactive enabled Claude (not -p/SDK), opt-in approval and organization policy, and observed read of the message body. Do not configure installed clients without authorization.
- [ ] **Step 3: Document and regenerate.** Explain enrollment/proof configuration, which sessions count active, CLI flags, routing, long-body retrieval, limits, late-send recovery, opt-in subscription argument and native client limitations. `just docs-generate` and `just docs-check`; use authored plugin source and regenerate only if affected.
- [ ] **Step 4: Run qualification.** `just check`, `just lint`, `bash scripts/check-feature-slices.sh`, and workspace Nextest. On unchanged debug stack failures, reproduce with the saved pre-feature binary before classifying baseline; run affected live cases with the exact candidate and if needed optimized binary without changing product stack limits. Record separate receipts; a focused pass is not a workspace pass.
- [ ] **Step 5: Commit.** Stage explicit Task 5 paths; `git commit -m 'test(messages): qualify and document agent session messaging'`.

### Task 6: Review the complete PR diff and publish

**Files:** Frozen candidate commit range, all files changed against live `origin/main`, reviewed plans/specs, and task-owned review evidence.

**Interfaces:** User authorized push/PR, not merge. Preserve external platform validation distinctions. Current verified remote is `dinglebear-ai/labby`; head branch is `codex/mcp-events-channels-20261108` unless live state requires a safe branch decision.

- [ ] **Step 1: Run the full-review skill** with independent lanes against the publication range (not only changes since `478077702`). Review each changed file including generated projections through their source/generator contract. Consolidate and validate the report and seal its evidence. Keep private scratch out of the PR.
- [ ] **Step 2: Reproduce and fix actionable findings.** Verify on the current head, use focused regression checks, rerun affected gates, and reconcile findings against the final head. Record refuted findings with evidence. Do not merge or request manual automated Codex review.
- [ ] **Step 3: Verify publication.** Recheck live remote branch/base, duplicate PRs, exact diff and secret scan. Push the authorized branch without force. Create one Labby PR to `main` with a reviewer-facing summary and actual checks/limits. Read back PR head/base/URL and verify pushed SHA. Leave CI, merge and deployment claims distinct.
