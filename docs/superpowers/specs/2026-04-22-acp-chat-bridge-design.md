# ACP Chat Bridge and Activity Timeline Design

Date: 2026-04-22
Status: Draft approved in chat, pending file review
Scope: `apps/gateway-admin`

## Goal

Replace mock-only chat activity rendering with a real ACP-backed event stream while keeping the UI adapter-agnostic. The first provider is `codex-acp`, but the UI and bridge must not become Codex-specific.

The target outcome is a live chat surface that renders real ACP session activity with full-fidelity event coverage, including transcript messages, tool calls, progress, permission prompts, todos, review/edit events, and other lifecycle updates.

## Non-Goals

- Building a production-grade distributed ACP gateway in the first pass
- Supporting multiple ACP providers on day one beyond the provider abstraction and initial `codex-acp` implementation
- Defining a permanent persistence backend in the first pass
- Exposing raw ACP wire payloads directly to React components

## Requirements

## Functional

- Use real ACP event data for chat rendering
- Support `codex-acp` as the first live ACP provider
- Keep the system adapter-agnostic so other ACP providers can be added later
- Render full-fidelity ACP activity cleanly in the UI
- Preserve streaming behavior and incremental updates
- Support session creation, loading, prompting, cancellation, and event subscription
- Keep live session state in memory with a seam for later persistence

## UX

- Maintain the current transcript-oriented chat layout
- Add a dedicated activity panel or activity lane for ACP events beyond plain messages
- Keep message transcript and activity stream separate but synchronized by session
- Make tool usage, permission prompts, todos, and review flows legible without exposing raw protocol internals
- Keep local development simple with the existing Next.js app workflow

## Technical

- Browser must not talk to ACP subprocesses directly
- ACP transport should terminate server-side
- Browser event delivery should use SSE in the first pass
- Browser mutations should remain ordinary HTTP requests
- Event normalization must be independent of the first provider implementation

## Chosen Approach

Implement a thin ACP bridge inside `apps/gateway-admin` with a provider abstraction and SSE event streaming. The first provider implementation will launch and manage `codex-acp`. The bridge will normalize ACP wire events into a UI event model consumed by the existing chat shell and a new activity timeline component.

This is the fastest route to real ACP rendering while preserving an extraction path later if the bridge needs to become a standalone process.

## Alternatives Considered

### 1. Direct UI integration with `codex-acp`

Rejected because the browser cannot own ACP stdio transport directly and because it would bind the UI too tightly to a single provider.

### 2. Standalone ACP gateway process from day one

Rejected for the first pass because it adds orchestration overhead before the UI semantics are validated.

### 3. Continue with mocked ACP-like data

Rejected because the explicit goal is to validate rendering against real ACP activity rather than invented shapes.

## Architecture

## High-level shape

There will be four layers:

1. React chat UI in `apps/gateway-admin`
2. Server-side ACP bridge in the same app
3. Provider interface with a `codex-acp` implementation
4. ACP agent subprocess managed by the provider

## Boundary definition

### Browser UI

Responsibilities:
- Display transcript messages
- Display normalized activity events
- Send prompt and session actions to the server
- Subscribe to per-session SSE event streams

Non-responsibilities:
- ACP transport handling
- subprocess lifecycle
- provider-specific parsing

### ACP bridge

Responsibilities:
- Manage provider instances and active sessions
- Receive ACP events from providers
- Normalize ACP events into UI events
- Persist in-memory session state and event history
- Fan out updates over SSE
- Expose HTTP endpoints for session mutations

Non-responsibilities:
- provider-specific wire details in the route layer
- UI-specific presentation logic

### Provider

Responsibilities:
- Start and manage ACP transport for a specific backend
- Convert generic bridge commands into ACP lifecycle operations
- Emit raw ACP updates and terminal states

Non-responsibilities:
- UI normalization policy
- browser transport

## Provider contract

The internal provider interface should cover:

- `startSession`
- `loadSession`
- `promptSession`
- `cancelSession`
- `subscribeToSessionEvents`
- `listSessions`
- `shutdownSession`

The return types should be bridge-owned types rather than direct provider payloads where possible.

## Session model

Session state will use a hybrid model.

### In-memory now

The first implementation keeps active session state in server memory:
- provider session ID
- UI session ID
- provider type
- status
- timestamps
- event history
- streaming cursors / sequence counters
- subscribers

### Persistence seam later

A persistence interface should be defined now but can initially use a no-op or in-memory backing implementation. The rest of the bridge should not assume a specific storage backend.

This allows later support for:
- process restart recovery
- transcript replay
- resume by session ID
- browsing historical runs

## Event model

The bridge must define a normalized UI event model. The React layer should only consume this model.

## Core event categories

- `session.lifecycle`
- `message.user`
- `message.assistant`
- `message.system`
- `thinking`
- `tool.started`
- `tool.updated`
- `tool.completed`
- `tool.failed`
- `permission.requested`
- `permission.resolved`
- `todo.updated`
- `review.requested`
- `review.updated`
- `artifact.created`
- `status.updated`
- `error`
- `debug.raw` for unsupported or unmapped cases

## Event design rules

- Preserve ordering with a stable sequence number
- Preserve provider event IDs where available
- Store enough metadata to correlate timeline cards with transcript moments
- Do not discard unknown ACP events; map them to a debug-preserving envelope
- Keep Codex-specific details nested under provider metadata rather than flattening them into the generic model

## Browser transport

## SSE for session updates

Use per-session SSE endpoints for one-way updates.

Reasoning:
- simpler than WebSockets for the first pass
- naturally fits streaming updates
- easy reconnect semantics
- low implementation overhead inside Next app routes

The browser should reconnect using last-seen sequence metadata so the server can resume or replay missed buffered events when possible.

## HTTP for commands

Use ordinary HTTP endpoints for:
- create session
- load session
- send prompt
- cancel session
- list sessions

## UI design

## Transcript lane

Keep the current message thread as the transcript lane. It should continue to render user and assistant text messages, but the assistant bubble structure should be updated to consume normalized event-derived message state instead of mock-only parts.

## Activity timeline

Add a dedicated activity timeline component that renders ACP events outside the message bubble body.

This timeline should support:
- collapsible grouped reasoning/activity sections
- source chips / tool chips
- status badges for running/success/failure
- expandable detail panels for tool input/output
- permission prompt cards
- todo cards
- review/edit cards
- artifact previews when event payloads support them

## Naming

The safer product label is `Activity` or `Reasoning & Activity` rather than a strict `Chain of Thought` label, because ACP events may include operational state, tool work, permissions, and reviews that are broader than internal reasoning.

## Synchronization model

Transcript and activity are separate render lanes backed by the same event stream. The transcript should derive only the message-relevant subset. The activity lane should derive everything else plus any explicit reasoning events.

## Initial route and module layout

Proposed server-side layout under `apps/gateway-admin`:

- `lib/acp/types.ts`
- `lib/acp/provider.ts`
- `lib/acp/providers/codex-acp.ts`
- `lib/acp/session-registry.ts`
- `lib/acp/normalize.ts`
- `lib/acp/events.ts`
- `app/api/acp/sessions/route.ts`
- `app/api/acp/sessions/[sessionId]/prompt/route.ts`
- `app/api/acp/sessions/[sessionId]/events/route.ts`
- `app/api/acp/sessions/[sessionId]/cancel/route.ts`

Proposed client-side additions:

- `components/chat/activity-timeline.tsx`
- `components/chat/activity-card.tsx`
- `components/chat/activity-group.tsx`
- `lib/chat/use-session-events.ts`
- updates to existing chat types and shell components

## Error handling

The bridge must explicitly handle:

- provider binary missing
- provider startup failure
- ACP initialization failure
- authentication/setup failure
- session creation failure
- session prompt failure
- stream disconnects
- provider subprocess exit during active session
- unsupported ACP update payloads

UI behavior should distinguish:
- recoverable disconnects
- provider unavailable
- per-session terminal failure
- event parse degradation with raw debug fallback

## Testing strategy

## Unit tests

- normalization from ACP events to UI events
- codex provider mapping behavior
- session registry sequencing and replay behavior
- reducer/selector logic for transcript and activity views

## Browser and integration tests

- render transcript from recorded real ACP fixtures
- render tool activity lifecycle from recorded real ACP fixtures
- render permission/todo/review cards from recorded real ACP fixtures
- verify SSE reconnect behavior with buffered replay

## Verification target

The first successful verification should be a real `codex-acp` session producing live events that render in both:
- transcript lane
- activity timeline

## Performance and React concerns

- Keep raw event parsing and normalization server-side
- Stream minimal UI event payloads to the browser
- Use stable IDs and sequence numbers for list rendering
- Avoid passing raw provider payloads through multiple component layers
- Keep transcript selectors and activity selectors separate to limit rerenders

## Risks

### 1. `codex-acp` event semantics may not align perfectly with current mock types

Mitigation:
- normalize server-side
- avoid tying React types to the current mock message shape

### 2. Unsupported ACP event categories may appear during real runs

Mitigation:
- use `debug.raw` fallback
- preserve source payloads under provider metadata

### 3. SSE replay semantics may become tricky under reconnects

Mitigation:
- use per-session sequence counters and in-memory ring buffers from the start

### 4. Local process management may be brittle in dev

Mitigation:
- isolate provider startup and shutdown in one module
- surface provider health explicitly in the UI

## Rollout plan

### Phase 1

- establish server bridge primitives
- implement `codex-acp` provider
- define normalized event model
- expose session create, prompt, and SSE stream routes

### Phase 2

- replace mock message source with bridge-driven state
- add activity timeline rendering
- map core ACP event categories

### Phase 3

- map full-fidelity categories: permissions, todos, reviews, artifacts, raw debug fallback
- improve reconnect, cancellation, and session reload behavior

### Phase 4

- add persistence backend behind the session store seam
- support historical session browsing and resume

## Decision summary

- Use a thin in-app ACP bridge
- Use `codex-acp` as the first provider
- Keep the bridge provider-agnostic
- Use hybrid session state with in-memory first and a persistence seam now
- Use SSE for browser streaming
- Add a dedicated activity timeline driven by normalized ACP events
- Render full-fidelity ACP activity, not just messages and tool calls
