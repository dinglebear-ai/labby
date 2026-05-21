# ACP Chat UI Design

Status: Draft implemented in product
Scope: `apps/gateway-admin` chat surface at `/chat`

## Goal

The chat UI should feel like a normal transcript-first chat product while exposing real ACP reasoning and tool activity in a structured, readable way.

The interface must not split conversation and activity into competing panes. Reasoning and action flow belong to the assistant turn itself.

## Interaction Model

The chat surface is a single transcript column.

Each assistant turn may contain three ordered sections:

1. Reasoning block
2. Action flow block
3. Assistant response text

These sections are optional and may appear incrementally while streaming.

## Reasoning Block

Reasoning is rendered as a compact collapsible block owned by the assistant turn.

Behavior:
- appears only when ACP emits thought/reasoning chunks
- opens automatically while reasoning is streaming
- can be collapsed by the user
- remains attached to the turn that produced it

Visual rules:
- compact summary row at rest
- body text uses normal Body typography, not mono
- styling should feel quieter than the assistant response bubble
- reasoning is secondary context, not the main answer

## Action Flow

Tool and agent operations are rendered inline as a connected, compact action timeline.

### Product intent

Action flow is not a debug inspector and not a second pane.

It is the product rendering for visible agent work inside a normal assistant turn.

The action flow exists so the user can answer three questions quickly:

1. What is the agent doing right now?
2. What happened before this answer was produced?
3. Where can I inspect a specific operation if I need more detail?

The default reading mode must feel conversational, not protocol-centric.

Behavior:
- action flow appears before, during, or after the assistant response text depending on event arrival order
- each row is readable as plain language first
- each row can expand to show raw tool-call detail
- raw payloads are secondary detail, not the default view

Examples:
- Reading workflow guidance
- Searching for profiles for Hayden Bleasel
- Found the profile photo for Hayden Bleasel
- Reading file
- Editing file
- Running command

Visual rules:
- connected vertical flow, not isolated utility cards
- icon on the left, action label on the right
- optional metadata chips or inline artifacts may follow the row
- status should be quiet and subordinate to the action label
- expanded detail should remain visually nested under the row

### Row model

Each action-flow row has five conceptual layers:

1. Leading icon
2. Human-readable action label
3. Quiet status/meta line
4. Optional inline artifacts
5. Expandable nested detail

The first two layers are the primary UX.

The remaining layers are supporting context.

### Row copy rules

The label must be humanized and task-oriented.

Prefer:
- `Reading workflow guidance`
- `Searching sources`
- `Reading app/api/router.rs`
- `Editing crates/lab/src/acp/runtime.rs`
- `Running cargo build`
- `Reviewing changes`
- `Found the profile photo for Hayden Bleasel`

Avoid:
- raw tool names
- transport-level event names
- generic labels like `Tool call started`
- large JSON-shaped summaries

When possible, row copy should read as a fragment of a live work narrative.

### Grouping rules

Action flow belongs to an assistant turn, but rows may be grouped inside the turn.

Grouping should happen when:
- the agent changes task
- a search phase clearly ends and a result phase begins
- a new tool family begins
- a long-running sequence needs compression for readability

Examples:
- `Searching for profiles...`
- `Found the profile photo...`
- `Searching for recent work...`

The grouping goal is narrative legibility, not protocol fidelity.

### Update rules

Rows may be created before assistant response text exists.

When additional ACP updates arrive for the same operation:
- update the existing row in place when correlation is clear
- do not create duplicate rows for every intermediate update
- preserve the operation’s position in the turn

If the operation transitions from running to completed or failed:
- keep the original row
- update status and inline artifacts
- do not move the row

### Expansion rules

Rows are compact by default.

Expanding a row should reveal:
- raw tool input when useful
- raw tool output when useful
- structured details like file paths, command lines, diff snippets, source URLs, or artifacts
- provider/debug metadata only when it helps explain the operation

Expansion should not be required to understand the main flow of the turn.

### Artifact rules

Inline artifacts should appear only when they materially improve understanding.

Good artifact candidates:
- source chips
- file path chips
- short file snippets
- compact diff previews
- image previews
- command excerpts

Artifact rendering must stay compact enough that the action flow still reads as a timeline, not as a stack of heavy cards.

### Status rules

Status must be visible but quiet.

Allowed states:
- running
- completed
- failed
- blocked
- cancelled

Status belongs below or beside the label as secondary information.

The label should remain the most prominent text in the row.

### Relationship to reasoning

Reasoning and action flow are separate constructs.

Reasoning answers:
- what the agent is thinking

Action flow answers:
- what the agent is doing

Reasoning should never be used as a substitute for operation history.

Action flow should never be styled as hidden chain-of-thought text.

### Relationship to assistant response text

Action flow supports the answer but does not replace the answer.

Ordering inside a turn is:
1. reasoning block
2. action flow
3. assistant response text

This ordering may be partial during streaming, but the stable resting structure should follow it.

### Action-flow sources

Action flow should be derived from ACP events including:
- `tool_call`
- `tool_call_update`
- `plan`
- permission events when they affect visible work
- selected session-status events when they explain the run

Not every ACP event deserves a visible action-flow row.

Low-signal protocol noise should remain hidden or debug-only.

### What does not belong in action flow

Do not render these as first-class timeline rows unless they materially affect the user:
- generic keepalive or transport noise
- repetitive status churn without new information
- raw protocol names
- duplicate updates that only restate the same state

### Acceptance criteria for action flow

- a user can understand the main work narrative without opening any row
- rows read like natural language work steps
- the flow remains visually connected and compact
- expanding a row reveals useful details without changing surrounding layout dramatically
- action rows stay attached to the assistant turn that produced them
- the assistant answer remains more readable than the action metadata

## Transcript Rules

Transcript remains the primary surface.

Rules:
- assistant answer text remains the most readable element in a turn
- user messages stay visually distinct and compact
- action flow and reasoning should support the answer, not compete with it
- no separate top-of-page reasoning or activity strip

## Mobile Behavior

The transcript remains primary on mobile.

Rules:
- session list becomes a drawer
- the drawer should not be the default resting state after selecting a session or creating one
- message width should widen on narrow screens
- input stays pinned to the bottom
- vertical space is prioritized for the transcript over secondary chrome

## ACP Mapping Rules

UI is derived from ACP events but is not a raw event inspector.

Mapping:
- thought chunks -> reasoning block
- tool calls / tool updates -> action flow rows
- plans -> structured action-flow groups or plan blocks attached to the turn
- permission requests / outcomes -> inline permission cards attached to the turn
- assistant chunks -> assistant response text
- raw input/output -> expandable nested detail

When ACP events arrive before assistant response text, the UI should still create an assistant turn container so reasoning and action flow can render in place.

## States

Empty:
- concise operational empty state

Running:
- subtle live indicator
- action rows may show running state inline

Error:
- provider unavailable and transport errors should not crash the page
- transcript and session list should degrade gracefully

## Design-System Alignment

The chat UI must follow the Aurora contract:
- transcript-first product surface
- Aurora tokens only
- compact operator spacing
- restrained active states
- mobile secondary navigation via drawer/sheet
- scrollbar styling on all overflow containers

## Implementation Notes

Primary files:
- `components/chat/chat-shell.tsx`
- `components/chat/message-thread.tsx`
- `components/chat/message-bubble.tsx`
- `components/chat/tool-call-display.tsx`
- `components/chat/session-sidebar.tsx`
- `lib/chat/session-events.ts`

## Acceptance Criteria

- `/chat` renders as one conversation-first transcript
- reasoning is inline and collapsible
- tool activity is inline as a connected action flow
- action rows expand into raw detail on demand
- mobile uses a session drawer instead of a permanent left rail
- auth or ACP failures do not crash the shell
- action flow remains readable as plain-language work history without requiring expansion
