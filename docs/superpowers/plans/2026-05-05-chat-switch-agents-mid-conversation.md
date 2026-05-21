# Chat Switch Agents Mid-Conversation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an existing Lab ACP chat conversation switch its active agent/provider, record turn ownership, and make any context carry-forward or reset explicit to the user.

**Architecture:** Treat the Lab ACP session as the durable conversation container and the ACP provider runtime as the active turn executor. A prompt may request a provider different from the session's current provider; the backend emits a provider-switch event, starts a provider-specific runtime, sends a bounded transcript handoff or explicit reset notice, and records provider ownership on every subsequent event/message. The gateway-admin UI keeps provider selection inside the current run, sends that provider with the prompt, and renders prior turn ownership.

**Tech Stack:** Rust 2024, `lab-apis` ACP domain types, Lab ACP dispatch/registry/runtime/persistence, Axum API, Next.js/React gateway-admin chat provider, Node tests, Playwright browser tests, `cargo nextest`, `pnpm test`.

---

## Scope Lock

This plan is for bead `lab-a4qa` only. It does not implement product code. It deliberately holds scope to mid-conversation provider switching in the existing ACP chat surface and does not add marketplace install flows, model-level routing, multi-agent orchestration, or a generic memory system.

The current repo already has:

- Provider discovery from `GET /v1/acp/provider` through `provider.list`.
- A chat agent picker in `apps/gateway-admin/components/chat/chat-input.tsx`.
- Frontend helpers that currently create a new run when selected provider differs from the selected run.
- ACP session persistence and event replay in `crates/lab/src/dispatch/acp/persistence.rs`.
- A single runtime handle per Lab ACP session in `crates/lab/src/acp/registry.rs`.

The gap is that switching providers currently means creating a different run/session, not continuing one conversation with explicit switch state.

## Research Findings

Repo facts:

- `crates/lab/src/dispatch/acp/catalog.rs` defines `session.prompt` without a provider parameter, so the server cannot verify the UI's intended provider at prompt time.
- `crates/lab/src/dispatch/acp/dispatch.rs` forwards `session.prompt` to `registry.prompt_session(session_id, text, principal)` with no active-provider choice.
- `crates/lab/src/acp/registry.rs` stores one `RuntimeHandle` per session and reattaches only when the handle is missing.
- `crates/lab-apis/src/acp/types.rs` stores `provider` only on `AcpSessionSummary`; most `AcpEvent` variants do not carry provider ownership.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` has `ensurePromptRunIdForProvider()`, which creates a new run when `selectedProviderId` differs from `selectedRun.provider`.
- `apps/gateway-admin/components/chat/chat-shell.test.tsx` and `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts` already test the current new-run behavior and must be inverted for this bead.

External protocol evidence:

- ACP has separate session setup and prompt-turn concepts; clients create/load sessions, send `session/prompt`, and receive `session/update` notifications during the turn. See https://agentclientprotocol.com/protocol/overview.
- ACP session resume/load support is not universal; the completed resume RFD calls out proxy/adapter fallback when an agent cannot provide full history. See https://agentclientprotocol.com/rfds/session-resume.
- ACP `session_info_update` establishes the pattern that dynamic session metadata belongs in session updates and should persist into list/session views. See https://agentclientprotocol.com/rfds/session-info-update.
- OpenAI Agents SDK sessions show the desired mental model: durable session history can be shared by different agents, but the session layer must retrieve and store turns explicitly. See https://openai.github.io/openai-agents-python/sessions/.
- Vercel AI SDK resume-stream docs reinforce that resumable chat requires app-owned message and stream persistence. See https://ai-sdk.dev/docs/ai-sdk-ui/chatbot-resume-streams.

## CEO Review: HOLD SCOPE

The correct user outcome is not "make the picker create a different session." The outcome is "I am in one conversation, I intentionally switch from Claude to Codex, the next prompt really goes to Codex, and the UI/history tells me what happened."

Minimum viable scope:

- One Lab ACP session remains selected.
- The prompt request carries the intended provider.
- Backend detects provider mismatch, switches runtime, records the switch, and makes the continuity mode visible.
- Events/messages after the switch have the new provider as owner.
- UI renders active provider and prior message ownership.
- Tests prove no silent wrong-runtime send.

Deferred out of scope:

- True provider-native shared memory across different ACP providers.
- Marketplace agent install/config UX.
- Switching while a turn is actively running.
- Multi-provider parallel turns.
- Automatic summarization with an LLM. Use deterministic bounded transcript handoff first.

Architecture diagram:

```text
ChatInput picker
  |
  | selectedProviderId + prompt
  v
ChatSessionProvider.sendPrompt()
  |
  | POST /v1/acp/sessions/:id/prompt { prompt, provider, continuityMode? }
  v
api/services/acp.rs
  |
  | dispatch session.prompt
  v
dispatch/acp/dispatch.rs
  |
  | PromptSessionRequest { session_id, text, provider, continuity_mode }
  v
AcpSessionRegistry
  |
  | if provider changed:
  |   close/drop old runtime
  |   launch new provider runtime
  |   emit provider_switch event
  |   prepend handoff/reset notice to first prompt
  v
provider runtime
  |
  | AcpEvent { provider ownership }
  v
SQLite persistence + SSE
  |
  v
MessageThread/MessageBubble provider badges
```

Data-flow shadow paths:

```text
Happy: selected provider differs -> server switches -> records provider_switch -> sends prompt to new runtime.
Nil: provider omitted -> server uses current active provider and does not switch.
Empty: provider="" -> validation treats it as omitted or rejects consistently with ACP param rules.
Error: provider unavailable or switch unsupported -> prompt is rejected before sending; UI keeps draft and shows visible error.
```

## Engineering Review

Critical gaps to guard before implementation:

| Codepath | Failure mode | Required handling |
| --- | --- | --- |
| `session.prompt` with provider mismatch | Prompt goes to old runtime because UI-only state drifted | Backend must accept intended provider and switch/validate server-side before prompt dispatch |
| Provider switch while current turn is running or waiting for permission | Old runtime continues producing events after new runtime starts | Reject with `invalid_state` until session is idle/completed; no implicit cancel |
| New provider cannot receive old context natively | User assumes continuity that does not exist | Emit provider-switch event with `continuity_mode: "handoff"` or `"reset"` and render it |
| Old persisted events lack provider fields | UI mislabels historical turns | Bridge fallback should use session summary provider for old events and provider field for new events |
| Runtime launch fails after old runtime is dropped | Session is left with no active runtime | Launch new runtime first where possible, then atomically swap; on failure keep old runtime and return visible error |
| Prompt request omits provider after UI switched | Backend cannot distinguish omission from intentional current provider | Frontend must always send selected provider; backend treats omitted provider as current provider for old clients only |

Recommended implementation direction:

1. Extend `session.prompt` rather than adding a separate mandatory `session.switch_provider` call. This makes the prompt operation atomic and prevents "selected in UI but not actually switched" drift.
2. Add a provider-switch event plus provider ownership on message/tool/status events. The event explains the transition; per-event ownership powers transcript badges and later analytics.
3. Keep deterministic handoff small: include recent user/assistant text only, with hard byte/turn caps and a system-style preamble. If no safe handoff can be built, reset with a visible notice.
4. Prefer backend truth over frontend convenience. The UI picker only selects intent; the server decides whether the switch is valid.

## File Map

Modify these product files during implementation:

- `crates/lab-apis/src/acp/types.rs`
  - Add provider ownership to `AcpEvent` variants that represent turn output.
  - Add `AcpProviderSwitch` event variant or equivalent provider-switch event data.
  - Add small enums/types for continuity mode/status if needed.
- `crates/lab/src/dispatch/acp/catalog.rs`
  - Add `provider` and optional `continuity_mode` params to `session.prompt`.
  - Document provider-switch semantics in the action description.
- `crates/lab/src/dispatch/acp/dispatch.rs`
  - Parse provider and continuity params.
  - Pass them through to the registry.
  - Keep `help`/`schema` generated from the same catalog.
- `crates/lab/src/api/services/acp.rs`
  - Add `provider` and optional `continuityMode` to `PromptBody`.
  - Pass provider through to dispatch as snake_case params.
- `crates/lab/src/acp/registry.rs`
  - Store active provider/runtime metadata per session.
  - Implement switch-before-prompt validation, runtime launch/swap, provider-switch event emission, and handoff/reset prompt prefix.
  - Keep old runtime if new launch fails.
- `crates/lab/src/acp/runtime.rs`
  - Ensure launched runtime reports provider id and emits events with provider ownership.
  - Add or expose test hooks so registry tests can assert which provider received a prompt.
- `crates/lab/src/dispatch/acp/persistence.rs`
  - Persist/read new provider fields and switch events.
  - Add backward-compatible decoding/fallback for old rows.
- `apps/gateway-admin/lib/acp/types.ts`
  - Add provider-switch bridge event shape and provider ownership fields.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`
  - Replace "provider mismatch creates a new run" with "same run, prompt body includes selected provider".
- `apps/gateway-admin/lib/chat/chat-session-provider.tsx`
  - Keep `selectedProviderId` as active intent for the selected run.
  - Send selected provider with every prompt.
  - Surface backend switch errors without marking all providers unavailable.
- `apps/gateway-admin/lib/chat/session-events.ts`
  - Map provider-owned ACP events into provider-owned `BridgeEvent`s and messages.
  - Derive switch notices for the transcript/activity stream.
- `apps/gateway-admin/components/chat/types.ts`
  - Add `providerId` and display label metadata to `ACPMessage`.
- `apps/gateway-admin/components/chat/message-bubble.tsx`
  - Render compact provider ownership badges for prior turns.
- `apps/gateway-admin/components/chat/chat-input.tsx`
  - Keep current picker, but ensure accessible labeling clearly reflects the active provider for the next send.
- `apps/gateway-admin/components/chat/chat-shell.tsx`
  - Pass provider metadata through to transcript/input as needed.
- `apps/gateway-admin/components/floating-chat-shell.tsx`
  - Mirror the same send/ownership behavior in the floating chat.

Modify or add these tests:

- `crates/lab/tests/acp_backend_contract.rs`
  - Add backend prompt-provider switch coverage.
- `crates/lab-apis/tests/acp_types.rs` or existing ACP type tests if present
  - Add serde round-trip coverage for provider-owned events and provider-switch event.
- `apps/gateway-admin/components/chat/chat-shell.test.tsx`
  - Invert provider mismatch expectation: no new run, prompt posts to selected provider in same run.
- `apps/gateway-admin/lib/chat/session-events.test.ts`
  - Add provider ownership and switch notice derivation tests.
- `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`
  - Update browser test to assert same session id is prompted after selecting another provider.

Docs/generated follow-up:

- If the ACP action catalog changes generated docs, refresh the generated MCP/API help with the existing repo command used for action catalogs. If unsure, inspect `Justfile` and `scripts/` first; do not hand-edit generated catalog files.

## Task 1: ACP Domain Types And Serde Contract

**Files:**
- Modify: `crates/lab-apis/src/acp/types.rs`
- Test: `crates/lab-apis/tests/acp_types.rs` or the nearest existing ACP serde test file

- [ ] **Step 1: Write failing serde tests for provider-owned events**

Add tests that serialize and deserialize:

```rust
#[test]
fn acp_message_chunk_round_trips_provider_owner() {
    let event = AcpEvent::MessageChunk {
        id: "evt-1".into(),
        created_at: "2026-05-05T00:00:00Z".into(),
        session_id: "session-1".into(),
        seq: 1,
        provider: "claude-acp".into(),
        role: "assistant".into(),
        text: "I can continue from the handoff.".into(),
        message_id: "msg-1".into(),
    };

    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["provider"], "claude-acp");
    let decoded: AcpEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.provider_id(), Some("claude-acp"));
}
```

Also add a `provider_switch` round-trip:

```rust
#[test]
fn acp_provider_switch_event_round_trips_visible_continuity() {
    let event = AcpEvent::ProviderSwitch {
        id: "evt-switch".into(),
        created_at: "2026-05-05T00:00:00Z".into(),
        session_id: "session-1".into(),
        seq: 2,
        from_provider: "codex-acp".into(),
        to_provider: "claude-acp".into(),
        continuity_mode: "handoff".into(),
        message: "Continuing with Claude ACP using a bounded transcript handoff.".into(),
    };

    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["kind"], "provider_switch");
    assert_eq!(value["to_provider"], "claude-acp");
    let decoded: AcpEvent = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.seq(), 2);
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p lab-apis acp_provider_switch_event_round_trips_visible_continuity acp_message_chunk_round_trips_provider_owner
```

Expected: FAIL because the new fields/variant/helper do not exist yet.

- [ ] **Step 3: Implement the smallest type changes**

In `crates/lab-apis/src/acp/types.rs`:

- Add `provider: String` to event variants that represent a runtime/provider-owned turn artifact:
  - `MessageChunk`
  - `ReasoningChunk`
  - `ToolCallStart`
  - `ToolCallUpdate`
  - `PermissionRequest`
  - `PermissionOutcome`
  - `UsageUpdate`
  - `ContentBlocks`
  - `SessionUpdate`
- Add `ProviderSwitch { from_provider, to_provider, continuity_mode, message }`.
- Add `provider_id(&self) -> Option<&str>` helper.
- Use serde defaults only where needed to keep old persisted JSON readable. For old message events, decode missing provider as empty string and let bridge conversion fall back to the session provider.

- [ ] **Step 4: Run tests again**

Run:

```bash
cargo test -p lab-apis acp_provider_switch_event_round_trips_visible_continuity acp_message_chunk_round_trips_provider_owner
```

Expected: PASS.

- [ ] **Step 5: Commit**

Do not commit unless the user explicitly asks. If implementing later with commits enabled:

```bash
git add crates/lab-apis/src/acp/types.rs crates/lab-apis/tests/acp_types.rs
git commit -m "feat(acp): record provider ownership on session events"
```

## Task 2: Backend Prompt-Time Provider Switching

**Files:**
- Modify: `crates/lab/src/dispatch/acp/catalog.rs`
- Modify: `crates/lab/src/dispatch/acp/dispatch.rs`
- Modify: `crates/lab/src/api/services/acp.rs`
- Modify: `crates/lab/src/acp/registry.rs`
- Modify: `crates/lab/src/acp/runtime.rs`
- Modify: `crates/lab/src/dispatch/acp/persistence.rs`
- Test: `crates/lab/tests/acp_backend_contract.rs`

- [ ] **Step 1: Write failing backend contract test for same-session switch**

Add a test shaped like:

```rust
#[tokio::test]
async fn prompt_with_new_provider_switches_runtime_inside_same_lab_session() {
    let registry = test_registry_with_fake_providers(["codex-acp", "claude-acp"]).await;
    let principal = "user@example.test";

    let summary = registry
        .create_session(StartSessionInput {
            provider: Some("codex-acp".into()),
            title: Some("Switch test".into()),
            cwd: "/tmp/lab-switch-test".into(),
            principal: Some(principal.into()),
        }, principal)
        .await
        .unwrap();

    registry
        .prompt_session_with_options(
            &summary.id,
            "Continue this with Claude",
            principal,
            PromptSessionOptions {
                provider: Some("claude-acp".into()),
                continuity_mode: Some("handoff".into()),
            },
        )
        .await
        .unwrap();

    let updated = registry.get_session(&summary.id).await.unwrap();
    assert_eq!(updated.id, summary.id);
    assert_eq!(updated.provider, "claude-acp");

    let events = registry.get_events_since(&summary.id, 0, principal).await.unwrap();
    assert!(events.iter().any(|event| matches!(event, AcpEvent::ProviderSwitch { from_provider, to_provider, continuity_mode, .. }
        if from_provider == "codex-acp" && to_provider == "claude-acp" && continuity_mode == "handoff"
    )));
    assert_eq!(fake_provider_prompt_count("codex-acp"), 0);
    assert_eq!(fake_provider_prompt_count("claude-acp"), 1);
}
```

If existing runtime test hooks cannot express provider-specific fake runtimes, add a narrow test-only launcher abstraction in `runtime.rs` rather than using sleeps or process spawning.

- [ ] **Step 2: Write failing invalid-state test**

Add:

```rust
#[tokio::test]
async fn provider_switch_is_rejected_while_session_is_running() {
    let registry = test_registry_with_fake_providers(["codex-acp", "claude-acp"]).await;
    let principal = "user@example.test";
    let session = create_running_test_session(&registry, "codex-acp", principal).await;

    let err = registry
        .prompt_session_with_options(
            &session.id,
            "switch now",
            principal,
            PromptSessionOptions {
                provider: Some("claude-acp".into()),
                continuity_mode: Some("handoff".into()),
            },
        )
        .await
        .unwrap_err();

    assert_eq!(err.kind(), "invalid_state");
    assert_eq!(fake_provider_prompt_count("claude-acp"), 0);
}
```

- [ ] **Step 3: Run backend tests to verify failure**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --all-features prompt_with_new_provider_switches_runtime_inside_same_lab_session provider_switch_is_rejected_while_session_is_running
```

Expected: FAIL because prompt options and switch logic do not exist.

- [ ] **Step 4: Extend `session.prompt` params**

In `catalog.rs`, add:

- `provider`, optional string, "Provider to use for this prompt; if different from the current session provider, Lab switches runtime before dispatch."
- `continuity_mode`, optional string, allowed values documented as `handoff` or `reset`.

In `dispatch.rs`, parse these params and call a new method:

```rust
registry
    .prompt_session_with_options(
        session_id,
        &effective_text,
        principal,
        PromptSessionOptions {
            provider: opt_str(&params, "provider").map(str::to_string),
            continuity_mode: opt_str(&params, "continuity_mode").map(str::to_string),
        },
    )
    .await?;
```

- [ ] **Step 5: Extend HTTP prompt body**

In `api/services/acp.rs`:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PromptBody {
    prompt: String,
    provider: Option<String>,
    continuity_mode: Option<String>,
    page_context: Option<PageContextBody>,
}
```

Pass `"provider": body.provider` and `"continuity_mode": body.continuity_mode` into dispatch params.

- [ ] **Step 6: Implement registry switch semantics**

Add:

```rust
pub struct PromptSessionOptions {
    pub provider: Option<String>,
    pub continuity_mode: Option<String>,
}
```

Rules:

- Normalize requested provider with `normalize_provider_id`.
- If omitted or equal to current provider, reuse current runtime.
- If different:
  - Require current state to be `Idle` or `Completed`.
  - Validate provider exists and is available via `provider_healths()`.
  - Build bounded handoff text from recent persisted/in-memory transcript unless `continuity_mode == "reset"`.
  - Launch the new provider runtime before dropping the old runtime if feasible.
  - Emit `AcpEvent::ProviderSwitch`.
  - Update summary `provider`, `provider_session_id`, `agent_name`, `agent_version`, and `updated_at`.
  - Persist updated summary and switch event.
  - Send the prompt to the new runtime with handoff/reset prefix.

The first implementation should use deterministic transcript handoff:

```text
You are continuing a Lab conversation that was previously handled by {from_provider}.
Continuity mode: handoff.
Recent transcript:
User: ...
Assistant ({from_provider}): ...

New user prompt:
{prompt}
```

Caps:

- Max 10 prior messages.
- Max 12 KiB handoff prefix.
- Exclude tool raw input/output by default.
- Redact with existing dispatch redaction helpers before including text.

- [ ] **Step 7: Make runtime events carry provider**

In `runtime.rs`, include the provider id in every `AcpEvent` produced by that runtime. Use `normalize_provider_id(input.provider.as_deref())` at runtime launch and thread that value through event constructors.

- [ ] **Step 8: Preserve old event compatibility**

In persistence and bridge conversion:

- New events persist provider fields.
- Old persisted rows missing provider still decode.
- Old rows displayed in UI fall back to the session summary provider or `"unknown"` rather than `"codex"` hard-coding.

- [ ] **Step 9: Run backend tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --all-features prompt_with_new_provider_switches_runtime_inside_same_lab_session provider_switch_is_rejected_while_session_is_running
```

Expected: PASS.

- [ ] **Step 10: Run broader ACP backend tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features acp
```

Expected: PASS for ACP-related tests.

- [ ] **Step 11: Commit**

Do not commit unless explicitly asked. Later commit command:

```bash
git add crates/lab/src/dispatch/acp/catalog.rs crates/lab/src/dispatch/acp/dispatch.rs crates/lab/src/api/services/acp.rs crates/lab/src/acp/registry.rs crates/lab/src/acp/runtime.rs crates/lab/src/dispatch/acp/persistence.rs crates/lab/tests/acp_backend_contract.rs
git commit -m "feat(acp): switch providers within a chat session"
```

## Task 3: Frontend Same-Run Provider Intent

**Files:**
- Modify: `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`
- Modify: `apps/gateway-admin/lib/chat/chat-session-provider.tsx`
- Modify: `apps/gateway-admin/components/chat/chat-shell.test.tsx`

- [ ] **Step 1: Update failing unit test for same-run switch**

Change the current provider mismatch test in `chat-shell.test.tsx` so it expects:

- `createSession` is not called.
- Prompt path is the existing run id.
- Prompt body includes `provider: "claude-acp"`.

Target shape:

```ts
test('sendPromptForSelectedProvider sends selected provider through the existing run', async () => {
  let createCalls = 0
  const requests: Array<{ path: string; body: unknown }> = []

  await sendPromptForSelectedProvider({
    payload: { text: 'hello', attachments: [] },
    selectedRun: { ...run('run-codex'), provider: 'codex-acp' },
    selectedProviderId: 'claude-acp',
    createSession: async () => {
      createCalls += 1
      return run('unexpected')
    },
    isMobileViewport: false,
    fetchAcp: async (path, init) => {
      requests.push({ path, body: JSON.parse(String(init?.body)) })
      return new Response(JSON.stringify({ ok: true }), { status: 200 })
    },
    refreshSessions: async () => {},
    addOptimisticMessage: () => {},
    removeOptimisticMessage: () => {},
  })

  assert.equal(createCalls, 0)
  assert.deepEqual(requests, [{
    path: '/sessions/run-codex/prompt',
    body: { prompt: 'hello', provider: 'claude-acp' },
  }])
})
```

- [ ] **Step 2: Run frontend unit test to verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin test -- chat-shell.test.tsx
```

Expected: FAIL because existing helper creates `run-claude` and omits provider in the prompt body.

- [ ] **Step 3: Replace provider-mismatch session creation**

In `use-chat-session-controller.ts`:

- Keep `ensurePromptRunId()` for missing selected run.
- Remove or repurpose `ensurePromptRunIdForProvider()` so provider mismatch does not create a new run.
- Include `provider: selectedProviderId ?? selectedRun?.provider` in prompt body.
- Include `continuityMode: "handoff"` only when selected provider differs from selected run provider. Omit it for same-provider prompts.

Body shape:

```ts
const requestedProvider = selectedProviderId ?? selectedRun?.provider
const body = {
  prompt: payload.text,
  ...(requestedProvider && { provider: requestedProvider }),
  ...(selectedRun && requestedProvider && selectedRun.provider !== requestedProvider && { continuityMode: 'handoff' }),
  ...(payload.attachments.length > 0 && { attachments: payload.attachments }),
  ...(includePageContext && pageContext !== null && pageContext !== undefined && { pageContext }),
}
```

- [ ] **Step 4: Keep selected provider synced without erasing explicit intent**

In `chat-session-provider.tsx`:

- `selectRun(runId)` should keep setting `selectedProviderId(run.provider)`.
- `selectAgent(providerId)` should only change selected provider intent, not create a run.
- After `refreshSessions()`, if the currently selected run provider changed because the backend switched, keep `selectedProviderId` equal to that provider unless the user has since selected another provider.

Add a tiny ref if needed:

```ts
const providerIntentTouchedRef = React.useRef(false)
```

Use it only to avoid clobbering a user selection during a refresh race.

- [ ] **Step 5: Run frontend unit tests**

Run:

```bash
pnpm --dir apps/gateway-admin test -- chat-shell.test.tsx
```

Expected: PASS.

- [ ] **Step 6: Commit**

Do not commit unless explicitly asked. Later commit command:

```bash
git add apps/gateway-admin/lib/chat/use-chat-session-controller.ts apps/gateway-admin/lib/chat/chat-session-provider.tsx apps/gateway-admin/components/chat/chat-shell.test.tsx
git commit -m "feat(chat): send selected provider in the current session"
```

## Task 4: Transcript Ownership And Switch Notices

**Files:**
- Modify: `apps/gateway-admin/lib/acp/types.ts`
- Modify: `apps/gateway-admin/lib/chat/session-events.ts`
- Modify: `apps/gateway-admin/components/chat/types.ts`
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`

- [ ] **Step 1: Write failing transcript derivation tests**

Add tests:

```ts
test('deriveTranscriptAndActivity preserves provider ownership per message', () => {
  const derived = deriveTranscriptAndActivity([
    bridgeEvent('evt-1', { kind: 'message.chunk', provider: 'codex-acp', role: 'assistant', messageId: 'm1', text: 'Codex turn' }),
    bridgeEvent('evt-2', { kind: 'provider_switch', provider: 'claude-acp', fromProvider: 'codex-acp', toProvider: 'claude-acp', continuityMode: 'handoff' }),
    bridgeEvent('evt-3', { kind: 'message.chunk', provider: 'claude-acp', role: 'assistant', messageId: 'm2', text: 'Claude turn' }),
  ])

  assert.equal(derived.messages[0]?.providerId, 'codex-acp')
  assert.equal(derived.messages[1]?.role, 'system')
  assert.match(derived.messages[1]?.text ?? '', /Switched from Codex ACP to Claude ACP/)
  assert.equal(derived.messages[2]?.providerId, 'claude-acp')
})
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
pnpm --dir apps/gateway-admin test -- session-events.test.ts
```

Expected: FAIL because messages do not carry provider ownership and switch events are unknown/debug events.

- [ ] **Step 3: Extend frontend event/message types**

In `apps/gateway-admin/lib/acp/types.ts`:

- Add `provider_switch` or `provider.switch` bridge event kind fields:
  - `fromProvider`
  - `toProvider`
  - `continuityMode`
  - `text`

In `components/chat/types.ts`:

```ts
export interface ACPMessage {
  providerId?: string
  providerName?: string
  // existing fields...
}
```

- [ ] **Step 4: Update event bridge conversion**

In `session-events.ts`:

- For Rust `AcpEvent::ProviderSwitch`, produce a bridge event with explicit fields.
- For message/tool/status events, preserve `event.provider`.
- For old events with missing/empty provider, fallback to the event/session provider supplied by the API response, not hard-coded `"codex"` where avoidable.
- In `deriveTranscriptAndActivity()`, set message provider fields from the originating bridge event.
- Insert a system message for provider switch:

```text
Switched from Codex ACP to Claude ACP. Continuing with a bounded transcript handoff.
```

For reset:

```text
Switched from Codex ACP to Claude ACP. Context was reset for this provider.
```

- [ ] **Step 5: Render ownership badges**

In `message-bubble.tsx`:

- Render a compact text badge for assistant and user messages when `providerName` exists.
- Do not put provider badges inside nested cards.
- Keep message layout stable for mobile; badge should wrap above text if needed.

Acceptance:

- Prior turns visibly show their provider where relevant.
- Current active provider still appears in `ChatInput`.
- System switch notices are visually distinct from assistant/user bubbles.

- [ ] **Step 6: Run transcript tests**

Run:

```bash
pnpm --dir apps/gateway-admin test -- session-events.test.ts
```

Expected: PASS.

- [ ] **Step 7: Commit**

Do not commit unless explicitly asked. Later commit command:

```bash
git add apps/gateway-admin/lib/acp/types.ts apps/gateway-admin/lib/chat/session-events.ts apps/gateway-admin/components/chat/types.ts apps/gateway-admin/components/chat/message-bubble.tsx apps/gateway-admin/components/chat/message-thread.tsx apps/gateway-admin/lib/chat/session-events.test.ts
git commit -m "feat(chat): show provider ownership in conversation history"
```

## Task 5: Browser Flow Verification

**Files:**
- Modify: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Update browser test for same-session switching**

Change `chat shell agent picker switches provider and sends through a provider-matched session` to assert:

- Existing selected session is `session-1` with `provider: "codex-acp"`.
- Selecting Claude does not POST `/v1/acp/sessions`.
- Prompt POST goes to `/v1/acp/sessions/session-1/prompt`.
- Prompt body includes `{ provider: "claude-acp", continuityMode: "handoff" }`.
- Mock events include a provider-switch event and a Claude message.
- UI renders the selected agent as Claude ACP and shows a switch notice.

Expected request assertions:

```ts
assert.deepEqual(createRequests, [], 'switching provider must not create a new Lab session')
assert.deepEqual(promptRequests, [{
  sessionId: 'session-1',
  prompt: 'Use Claude for this',
  provider: 'claude-acp',
  continuityMode: 'handoff',
}])
```

- [ ] **Step 2: Run the browser test to verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin test:browser -- chat-shell.browser.test.ts
```

Expected: FAIL before implementation, PASS after Tasks 3 and 4.

- [ ] **Step 3: Fix mocks and assertions only**

Do not loosen assertions. The test should prove same-session prompt routing, provider request body, and visible switch notice.

- [ ] **Step 4: Run browser test**

Run:

```bash
pnpm --dir apps/gateway-admin test:browser -- chat-shell.browser.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit**

Do not commit unless explicitly asked. Later commit command:

```bash
git add apps/gateway-admin/lib/browser/chat-shell.browser.test.ts
git commit -m "test(chat): cover same-session provider switching"
```

## Task 6: Catalog Docs And Full Verification

**Files:**
- Modify generated catalog docs only if the repo generator changes them.
- Candidate generated files: `docs/generated/mcp-help.json`, `docs/generated/feature-matrix.md`

- [ ] **Step 1: Refresh generated action docs if required**

Inspect the repo's existing generated-doc command first:

```bash
rg -n "mcp-help|generated action|action catalog|docs/generated" Justfile scripts crates/lab -g '*.rs' -g '*.sh' -g 'Justfile'
```

Run the established generator only if `session.prompt` catalog changes are expected to alter generated docs.

Expected: generated docs include `provider` and `continuity_mode` for `acp.session.prompt`.

- [ ] **Step 2: Run focused backend suite**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features acp
```

Expected: PASS.

- [ ] **Step 3: Run focused frontend tests**

Run:

```bash
pnpm --dir apps/gateway-admin test -- chat-shell.test.tsx session-events.test.ts
```

Expected: PASS.

- [ ] **Step 4: Run browser regression**

Run:

```bash
pnpm --dir apps/gateway-admin test:browser -- chat-shell.browser.test.ts
```

Expected: PASS.

- [ ] **Step 5: Run broad verification**

Run:

```bash
cargo nextest run --workspace --all-features
pnpm --dir apps/gateway-admin test
```

Expected: PASS. If unrelated failures exist, capture exact failing tests and prove all `lab-a4qa` focused tests pass.

- [ ] **Step 6: Manual smoke**

Run Lab server and gateway-admin as the repo normally documents. Then:

1. Open `/chat`.
2. Use an existing Codex ACP session.
3. Select Claude ACP in the agent picker.
4. Send `Continue this conversation with Claude`.
5. Verify the prompt request body includes `provider: "claude-acp"`.
6. Verify the session id is unchanged.
7. Verify the transcript shows a switch notice and Claude-owned subsequent turn.

Expected: the next prompt goes to the selected provider and the user can see which provider handled each relevant turn.

## Non-Goals And Follow-Up Beads

Create follow-up beads only if implementation reveals they are necessary:

- Provider-native `session/load` or `session/resume` negotiation for true cross-provider memory.
- LLM-generated transcript summaries for long handoffs.
- Switching while running via explicit cancel-and-switch flow.
- Provider-specific capability warnings in the picker.
- Marketplace flow to install missing ACP providers from the picker.

## Completion Checklist

- [ ] `session.prompt` accepts and validates provider intent server-side.
- [ ] Switching within one Lab ACP session updates active provider and runtime.
- [ ] Provider switch event is persisted and replayed.
- [ ] Provider ownership is present on new turn events/messages.
- [ ] Old persisted events remain readable.
- [ ] UI does not create a new run for provider mismatch.
- [ ] UI sends selected provider with the prompt.
- [ ] UI shows active provider and prior turn ownership.
- [ ] Unsupported/incompatible continuity is visible, not silent.
- [ ] Backend, frontend unit, and browser tests pass.
