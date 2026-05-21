# ACP Chat Bridge and Activity Timeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the mock-only chat activity path in `apps/gateway-admin` with a real ACP-backed bridge that uses `codex-acp` as the first provider, streams full-fidelity session updates over SSE, and renders both transcript and activity timeline views from normalized ACP events.

**Architecture:** Add a server-side ACP bridge inside `apps/gateway-admin` that owns provider lifecycle, live session registry, normalization, and SSE fanout. Keep the browser side provider-agnostic by consuming bridge-owned session and event models, while the first concrete provider launches and manages `codex-acp` behind a narrow provider interface.

**Tech Stack:** Next.js app routes, React 19, TypeScript, Node child-process management, SSE, ACP provider abstraction, `codex-acp`

---

## File structure map

### Server bridge

- Create: `apps/gateway-admin/lib/acp/types.ts`
  - Bridge-owned session, event, provider, and transport types
- Create: `apps/gateway-admin/lib/acp/provider.ts`
  - Provider interface and provider factory contract
- Create: `apps/gateway-admin/lib/acp/providers/codex-acp.ts`
  - `codex-acp` subprocess lifecycle, ACP session mapping, raw event emission
- Create: `apps/gateway-admin/lib/acp/session-registry.ts`
  - In-memory live session registry, subscriber tracking, sequence counters, replay buffer
- Create: `apps/gateway-admin/lib/acp/normalize.ts`
  - Raw ACP update to bridge event normalization
- Create: `apps/gateway-admin/lib/acp/sse.ts`
  - SSE framing helpers and reconnect cursor support
- Create: `apps/gateway-admin/lib/acp/persistence.ts`
  - Persistence seam and in-memory/no-op implementation
- Create: `apps/gateway-admin/lib/acp/health.ts`
  - Provider availability checks for UI boot/status

### API routes

- Create: `apps/gateway-admin/app/api/acp/sessions/route.ts`
  - List sessions and create session
- Create: `apps/gateway-admin/app/api/acp/sessions/[sessionId]/prompt/route.ts`
  - Prompt a live session
- Create: `apps/gateway-admin/app/api/acp/sessions/[sessionId]/events/route.ts`
  - SSE event stream with replay support
- Create: `apps/gateway-admin/app/api/acp/sessions/[sessionId]/cancel/route.ts`
  - Cancel a session
- Create: `apps/gateway-admin/app/api/acp/provider/route.ts`
  - Provider health / capability status

### Client and chat UI

- Create: `apps/gateway-admin/lib/chat/session-events.ts`
  - Client-side reducer/derivations for transcript and activity lanes
- Create: `apps/gateway-admin/lib/chat/use-session-events.ts`
  - SSE subscription hook and reconnect handling
- Create: `apps/gateway-admin/components/chat/activity-timeline.tsx`
  - Timeline root component
- Create: `apps/gateway-admin/components/chat/activity-group.tsx`
  - Grouped activity sections and collapsible containers
- Create: `apps/gateway-admin/components/chat/activity-card.tsx`
  - Generic event card switcher
- Create: `apps/gateway-admin/components/chat/activity-status-card.tsx`
  - Session/progress/status lifecycle events
- Create: `apps/gateway-admin/components/chat/activity-permission-card.tsx`
  - Permission prompt events
- Create: `apps/gateway-admin/components/chat/activity-todo-card.tsx`
  - Todo updates
- Create: `apps/gateway-admin/components/chat/activity-review-card.tsx`
  - Review/edit events
- Create: `apps/gateway-admin/components/chat/activity-debug-card.tsx`
  - Raw fallback event display for unsupported mappings

### Existing files to modify

- Modify: `apps/gateway-admin/components/chat/types.ts`
  - Replace mock-only assumptions with bridge session/event shapes or adapter types
- Modify: `apps/gateway-admin/components/chat/mock-data.ts`
  - Keep as fallback fixtures or move to recorded ACP fixtures
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx`
  - Replace local mock state with bridge-backed session/event state
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx`
  - Derive transcript lane from normalized session state
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
  - Stop assuming mock-only `parts` semantics and consume transcript-ready message models
- Modify: `apps/gateway-admin/components/chat/tool-call-display.tsx`
  - Render normalized tool lifecycle data from bridge events
- Modify: `apps/gateway-admin/components/chat/session-sidebar.tsx`
  - Load real sessions from bridge endpoints
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
  - Post prompts to live session route

### Tests

- Create: `apps/gateway-admin/lib/acp/normalize.test.ts`
- Create: `apps/gateway-admin/lib/acp/session-registry.test.ts`
- Create: `apps/gateway-admin/lib/acp/providers/codex-acp.test.ts`
- Create: `apps/gateway-admin/lib/chat/session-events.test.ts`
- Create: `apps/gateway-admin/components/chat/activity-timeline.test.tsx`
- Modify: `apps/gateway-admin/components/chat/*.test.tsx` as needed for transcript lane updates
- Add recorded ACP fixtures under: `apps/gateway-admin/lib/acp/__fixtures__/`

## Task 1: Define bridge-owned ACP types and provider contract

**Files:**
- Create: `apps/gateway-admin/lib/acp/types.ts`
- Create: `apps/gateway-admin/lib/acp/provider.ts`
- Test: `apps/gateway-admin/lib/acp/provider.test.ts` if needed

- [ ] **Step 1: Write the failing type-level and shape tests**

Create tests that assert the bridge model can represent:
- session lifecycle state
- transcript messages
- tool started/updated/completed/failed events
- permission/todo/review/debug fallback events
- provider capability metadata

- [ ] **Step 2: Run the targeted test file to verify it fails**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/provider.test.ts`
Expected: FAIL because the ACP bridge files do not exist yet

- [ ] **Step 3: Implement `types.ts`**

Include:
- `AcpProviderKind`
- `BridgeSession`
- `BridgeEvent`
- `BridgeTranscriptMessage`
- `BridgeToolLifecycle`
- provider metadata types
- replay cursor / sequence number types

- [ ] **Step 4: Implement `provider.ts`**

Define the provider interface with exact methods:
- `startSession`
- `loadSession`
- `promptSession`
- `cancelSession`
- `subscribeToSessionEvents`
- `listSessions`
- `shutdownSession`
- `health`

- [ ] **Step 5: Run the targeted test again**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/provider.test.ts`
Expected: PASS

## Task 2: Build the in-memory session registry and persistence seam

**Files:**
- Create: `apps/gateway-admin/lib/acp/session-registry.ts`
- Create: `apps/gateway-admin/lib/acp/persistence.ts`
- Test: `apps/gateway-admin/lib/acp/session-registry.test.ts`

- [ ] **Step 1: Write failing registry tests**

Cover:
- session creation
- sequence number incrementing
- subscriber fanout
- replay buffer access after reconnect
- session cancellation state updates
- persistence seam invocation points

- [ ] **Step 2: Run the registry test to verify it fails**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/session-registry.test.ts`
Expected: FAIL because registry and persistence seam are missing

- [ ] **Step 3: Implement persistence seam**

Add a minimal interface plus in-memory/no-op implementation. Do not add disk persistence yet.

- [ ] **Step 4: Implement session registry**

Requirements:
- in-memory session map
- subscriber list per session
- bounded replay buffer
- stable monotonically increasing sequence numbers
- session metadata updates independent of UI state

- [ ] **Step 5: Re-run registry tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/session-registry.test.ts`
Expected: PASS

## Task 3: Normalize raw ACP events into bridge events

**Files:**
- Create: `apps/gateway-admin/lib/acp/normalize.ts`
- Create: `apps/gateway-admin/lib/acp/__fixtures__/` with recorded ACP samples
- Test: `apps/gateway-admin/lib/acp/normalize.test.ts`

- [ ] **Step 1: Write failing normalization tests**

Cover mappings for:
- lifecycle events
- assistant/user message events
- tool lifecycle events
- permission requests
- todo updates
- review/update events
- unknown event fallback to `debug.raw`

- [ ] **Step 2: Run normalization tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/normalize.test.ts`
Expected: FAIL because normalizer does not exist

- [ ] **Step 3: Add fixture payloads**

Start with small recorded or hand-constructed ACP samples shaped like expected `codex-acp` output. Keep provider-specific details nested under metadata.

- [ ] **Step 4: Implement the normalizer**

Rules:
- bridge-owned event kinds only
- preserve provider event identity where available
- assign fallback `debug.raw` instead of dropping data
- keep message lane and activity lane derivation possible from the same event set

- [ ] **Step 5: Re-run normalization tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/normalize.test.ts`
Expected: PASS

## Task 4: Implement the `codex-acp` provider

**Files:**
- Create: `apps/gateway-admin/lib/acp/providers/codex-acp.ts`
- Create: `apps/gateway-admin/lib/acp/health.ts`
- Test: `apps/gateway-admin/lib/acp/providers/codex-acp.test.ts`

- [ ] **Step 1: Write failing provider tests**

Cover:
- subprocess startup failure surfaces provider unavailable state
- session creation maps to bridge session
- prompt calls produce raw event emissions
- cancel shuts down in-flight work cleanly
- provider health status distinguishes missing binary vs healthy runtime

- [ ] **Step 2: Run provider tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/providers/codex-acp.test.ts`
Expected: FAIL because provider implementation does not exist

- [ ] **Step 3: Implement provider startup and health check**

Requirements:
- launch `codex-acp`
- detect missing executable / auth failure / startup failure
- expose health metadata for UI boot

- [ ] **Step 4: Implement session lifecycle integration**

Requirements:
- start/load/prompt/cancel/list/shutdown mapping
- raw event subscription callback path for the registry
- no UI formatting logic inside provider

- [ ] **Step 5: Re-run provider tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/providers/codex-acp.test.ts`
Expected: PASS

## Task 5: Add Next API routes for sessions, prompts, cancel, provider health, and SSE

**Files:**
- Create: `apps/gateway-admin/app/api/acp/sessions/route.ts`
- Create: `apps/gateway-admin/app/api/acp/sessions/[sessionId]/prompt/route.ts`
- Create: `apps/gateway-admin/app/api/acp/sessions/[sessionId]/events/route.ts`
- Create: `apps/gateway-admin/app/api/acp/sessions/[sessionId]/cancel/route.ts`
- Create: `apps/gateway-admin/app/api/acp/provider/route.ts`
- Create: `apps/gateway-admin/lib/acp/sse.ts`
- Test: route-level tests if the app already has matching API route test patterns

- [ ] **Step 1: Write failing route tests or request-level assertions**

Cover:
- create/list sessions endpoint
- prompt endpoint validation
- cancel endpoint validation
- provider health response
- SSE event framing and headers

- [ ] **Step 2: Run route tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- app/api/acp`
Expected: FAIL because routes are missing

- [ ] **Step 3: Implement SSE helper**

Requirements:
- `text/event-stream`
- no-cache headers
- last event ID / replay cursor support
- clean shutdown when client disconnects

- [ ] **Step 4: Implement the route handlers**

Rules:
- route layer stays thin
- route layer delegates to registry/provider modules
- serialize only bridge-owned responses/events

- [ ] **Step 5: Re-run route tests**

Run: `cd apps/gateway-admin && pnpm test -- app/api/acp`
Expected: PASS

## Task 6: Build client-side session event hook and derivation layer

**Files:**
- Create: `apps/gateway-admin/lib/chat/session-events.ts`
- Create: `apps/gateway-admin/lib/chat/use-session-events.ts`
- Test: `apps/gateway-admin/lib/chat/session-events.test.ts`

- [ ] **Step 1: Write failing reducer/selector tests**

Cover:
- transcript derivation from normalized event stream
- tool lifecycle grouping
- activity timeline grouping
- reconnect merge behavior using sequence numbers
- session metadata state changes

- [ ] **Step 2: Run selector tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- lib/chat/session-events.test.ts`
Expected: FAIL because derivation layer is missing

- [ ] **Step 3: Implement `session-events.ts`**

Include:
- transcript selectors
- activity selectors
- grouped timeline derivation
- tool lifecycle correlation helpers

- [ ] **Step 4: Implement `use-session-events.ts`**

Requirements:
- open SSE connection for selected session
- track reconnect cursor
- merge incremental updates without duplicating events
- surface connection state to UI

- [ ] **Step 5: Re-run selector tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/chat/session-events.test.ts`
Expected: PASS

## Task 7: Add the activity timeline UI

**Files:**
- Create: `apps/gateway-admin/components/chat/activity-timeline.tsx`
- Create: `apps/gateway-admin/components/chat/activity-group.tsx`
- Create: `apps/gateway-admin/components/chat/activity-card.tsx`
- Create: `apps/gateway-admin/components/chat/activity-status-card.tsx`
- Create: `apps/gateway-admin/components/chat/activity-permission-card.tsx`
- Create: `apps/gateway-admin/components/chat/activity-todo-card.tsx`
- Create: `apps/gateway-admin/components/chat/activity-review-card.tsx`
- Create: `apps/gateway-admin/components/chat/activity-debug-card.tsx`
- Test: `apps/gateway-admin/components/chat/activity-timeline.test.tsx`

- [ ] **Step 1: Write failing UI tests**

Cover:
- grouped activity sections render in order
- tool lifecycle card states render correctly
- permission/todo/review cards are distinguishable
- debug/raw fallback card appears for unsupported events

- [ ] **Step 2: Run the timeline tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- components/chat/activity-timeline.test.tsx`
Expected: FAIL because timeline components are missing

- [ ] **Step 3: Implement the timeline components**

Rules:
- do not render raw provider payloads directly except inside debug fallback card
- keep components small and event-kind focused
- follow existing Aurora styling and chat component patterns

- [ ] **Step 4: Re-run the timeline tests**

Run: `cd apps/gateway-admin && pnpm test -- components/chat/activity-timeline.test.tsx`
Expected: PASS

## Task 8: Replace mock state in `ChatShell` with bridge-backed session and event state

**Files:**
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx`
- Modify: `apps/gateway-admin/components/chat/types.ts`
- Modify: `apps/gateway-admin/components/chat/session-sidebar.tsx`
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
- Modify: `apps/gateway-admin/components/chat/mock-data.ts`
- Test: existing chat component tests plus new tests as needed

- [ ] **Step 1: Write failing integration-oriented component tests**

Cover:
- session list loads from API
- selecting a session binds transcript and activity stream
- sending a prompt posts to API instead of mutating local mock state
- provider health / unavailable state is visible

- [ ] **Step 2: Run the affected chat component tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- components/chat`
Expected: FAIL because shell still depends on mock local state

- [ ] **Step 3: Refactor `ChatShell` to use bridge state**

Requirements:
- fetch sessions from bridge
- use SSE hook for selected session
- show provider health and session connection state
- preserve current layout controls where possible

- [ ] **Step 4: Re-run chat component tests**

Run: `cd apps/gateway-admin && pnpm test -- components/chat`
Expected: PASS

## Task 9: Update transcript rendering to consume normalized transcript messages

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx`
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
- Modify: `apps/gateway-admin/components/chat/tool-call-display.tsx`
- Test: related chat component tests

- [ ] **Step 1: Write failing transcript rendering tests**

Cover:
- transcript messages render from normalized message models
- tool cards render from correlated tool lifecycle state rather than mock `parts`
- streaming state renders incrementally

- [ ] **Step 2: Run the transcript tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- components/chat/message-thread.test.tsx components/chat/message-bubble.test.tsx`
Expected: FAIL because transcript components still assume mock-only shapes

- [ ] **Step 3: Refactor transcript components**

Rules:
- keep transcript lane separate from activity lane
- remove hard dependency on mock ACP part arrays where no longer needed
- keep copy and collapsible affordances where they still make sense

- [ ] **Step 4: Re-run transcript tests**

Run: `cd apps/gateway-admin && pnpm test -- components/chat/message-thread.test.tsx components/chat/message-bubble.test.tsx`
Expected: PASS

## Task 10: Add full-fidelity bridge event coverage and graceful fallback

**Files:**
- Modify: `apps/gateway-admin/lib/acp/normalize.ts`
- Modify: `apps/gateway-admin/components/chat/activity-*`
- Modify: tests across ACP and chat modules

- [ ] **Step 1: Add failing tests for the final unmapped categories**

Cover:
- permission resolution updates
- todo state transitions
- review state transitions
- artifact preview metadata
- unsupported event fallback card

- [ ] **Step 2: Run those tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/normalize.test.ts components/chat/activity-timeline.test.tsx`
Expected: FAIL on the newly added coverage

- [ ] **Step 3: Implement the remaining mappings and cards**

Keep provider-specific details nested, but make the UI state complete enough to exercise the full-fidelity target.

- [ ] **Step 4: Re-run the targeted tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/normalize.test.ts components/chat/activity-timeline.test.tsx`
Expected: PASS

## Task 11: Add provider availability and local-dev operator guidance

**Files:**
- Modify: `apps/gateway-admin/README.md`
- Modify: any local ACP setup docs under `apps/gateway-admin/docs/` if appropriate
- Test: none required beyond documenting exact startup expectations

- [ ] **Step 1: Document local setup requirements**

Add:
- required `codex-acp` installation/runtime assumptions
- any auth env needed for `codex-acp`
- how the provider health endpoint behaves when unavailable
- how to run the app in local-view mode while still using real ACP

- [ ] **Step 2: Add startup troubleshooting notes**

Cover:
- missing binary
- auth errors
- provider startup failure
- SSE reconnect behavior

## Task 12: End-to-end verification pass

**Files:**
- No new files required unless gaps are found

- [ ] **Step 1: Run ACP bridge unit and component tests**

Run: `cd apps/gateway-admin && pnpm test -- lib/acp/**/*.test.ts lib/chat/**/*.test.ts components/chat/**/*.test.tsx`
Expected: PASS

- [ ] **Step 2: Run the dev server with real ACP provider enabled**

Run: `cd apps/gateway-admin && LAB_ALLOWED_DEV_ORIGINS=dookie pnpm dev --hostname 0.0.0.0`
Expected: server boots with provider health reporting either ready or explicit unavailable reason

- [ ] **Step 3: Manually exercise one real `codex-acp` session**

Verify:
- session appears in sidebar
- transcript messages stream
- activity timeline updates in real time
- at least one tool event, one status event, and one non-message event render correctly

- [ ] **Step 4: Capture any fixture payloads needed from the real run**

Store minimal sanitized ACP samples under `lib/acp/__fixtures__/` if gaps were found during manual exercise.

## Notes for execution

- Keep route handlers thin; logic belongs in `lib/acp/*`
- Do not bind the React layer to `codex-acp` specifics
- Do not skip the `debug.raw` fallback path; it is the safety valve for unknown ACP updates
- Prefer small focused components over one large activity renderer
- Keep transcript selectors and activity selectors separate to avoid unnecessary rerenders
- Preserve the existing local dev workflow and host binding setup already being used in this session
