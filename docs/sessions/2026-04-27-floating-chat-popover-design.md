---
date: 2026-04-27 08:22:23 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 80d23563
plan: none
agent: Claude (claude-sonnet-4-6 / claude-opus-4-7)
session id: 57f625d6-fcbd-4f64-a502-06e563a51d27
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/57f625d6-fcbd-4f64-a502-06e563a51d27.jsonl
working directory: /home/jmagar/workspace/lab
---

## User Request

Design a floating "chatheads" style chat component for the gateway-admin app — a fixed pill FAB (bottom-right) that opens a draggable/resizable popover with full `/chat` feature parity, a hotkey (`Cmd/Ctrl+/`), session persistence, and session switching. Must be fully compliant with the Aurora design system contract.

## Session Overview

Ran the full `/lavra-design` pipeline from raw idea to locked, swarmable epic. The feature grew from a simple floating chat request into a context-aware operator assistant with a token-safe opt-in page-context system. Final output: epic `lab-gych` with 5 child beads, 3 execution waves, 30+ locked decisions, and a companion backlog bead (`lab-csvf`).

## Sequence of Events

1. **Brainstorm** (`/lavra-brainstorm`) — Interactive dialogue to lock core UX decisions: FAB + draggable popover (not messenger-style bubbles), global on every admin page, shared session state with `/chat`, mobile = full-screen Sheet, `Cmd/Ctrl+/` hotkey, full feature parity, persistence of position/size/open-state configurable in popover header gear panel.

2. **Plan** (`/lavra-plan`) — Built 4 child beads with comprehensive What/Context/Decisions/Testing/Validation/Files/Dependencies sections. Set dependency ordering (Phase 1 → Phase 2 → Phase 3; Phase 2 and Phase 4 can run in parallel after Phase 1). `bd swarm validate` passed.

3. **Research** (`/lavra-research`) — Dispatched 5 domain-matched agents in parallel (architecture-strategist, julik-frontend-races-reviewer, performance-oracle, best-practices-researcher, kieran-typescript-reviewer). 23 findings logged as INVESTIGATION/FACT/PATTERN comments across all beads.

4. **Revise** — Integrated research findings into bead descriptions. Key revisions: "always mounted" pattern replaced with "lazy first-open then stay mounted"; last-session restore moved to lazy `useState` initializer in provider; SSE cache LRU cap added; `resolveSessionStatusFromEvents` memoization added.

5. **CEO Review** (`/lavra-ceo-review`) — Mode: SCOPE EXPANSION. Interactive 10-section review with stop-per-issue decisions. Added `pageContext` slot for context-aware assistant, 3-context split → expanded to 4-context, lazy first-open mount locked, FAB ambient connection indicator added, error boundary, modal stack guard for `AppCommandPalette` conflict, sidebar session indicator backlog bead (`lab-csvf`) created.

6. **Engineering Review** (`/lavra-eng-review`) — 4 agents in parallel (architecture-strategist, code-simplicity-reviewer, security-sentinel, performance-oracle). Found CRITICAL eager SSE contradiction, 6 HIGH performance issues, 4 HIGH security issues. 4 trade-off decisions resolved interactively.

7. **Final Lock** — All 5 bead descriptions updated with every decision. `bd swarm validate` passed (3 waves, max 2 parallel, swarmable: YES).

## Key Findings

- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` — monolithic hook to split; `runs[]` always re-allocates on every SSE token via `current.map()` (HIGH perf issue); `shouldAutoCreateInitialRun` has a phantom session race when provider resolves before sessions list.
- `apps/gateway-admin/lib/chat/use-session-events.ts:20-21` — two module-scope Maps (`sessionEventCache`, `sessionLastSeqCache`) never evicted; must become single atomic Map with LRU cap 5.
- `apps/gateway-admin/components/app-command-palette.tsx` — exact pattern precedent for layout-mounted `'use client'` component with hotkey; modal stack guard needed to prevent Cmd+K firing through `aria-modal`.
- `apps/gateway-admin/app/(admin)/layout.tsx` — currently a Server Component with client island children; `ChatSessionProvider` mounts as a client island in the same pattern.
- `apps/gateway-admin/next.config.mjs` — `output: 'export'` (static export); all new components must be `'use client'`; no server actions; localStorage only in `useEffect`.
- `react-resizable-panels@^2.1.7` already installed — covers resize half without new dependency.
- `swr@^2.4.1` already installed — could be used for `/acp/sessions` and `/acp/provider` fetches.
- FAB and popover must be CSS-hidden (`visibility:hidden`) on `/chat` route, not unmounted — open state preserved across navigation.

## Technical Decisions

| Decision | Rationale |
|---|---|
| 4-context split (Data/Actions/Connection/Stream) | FAB consumes only Connection; without split it re-renders ~4000× during a 4000-token streaming response |
| SSE stream gated by `streamEnabled` (lazy) | Without gating, every admin page load opens a background SSE connection the user may never use; at 50 concurrent admins = 50 idle connections |
| `runs[]` bail-out setter | `current.map()` always creates a new array reference even when no element changes; O(n²) re-renders × token count |
| `shouldAutoCreateInitialRun` `sessionsLoaded` guard | Provider health (~50ms) resolves before sessions list (~200ms); without guard, bootstrap fires with `runs.length===0` creating a phantom session |
| Auto-bootstrap gated behind `streamEnabled` | Prevents phantom sessions for users who never open chat |
| pageContext opt-in (default OFF) | Users unaware of feature shouldn't burn context window tokens silently |
| pageContext as separate request field (not prompt prefix) | Client-side system prompt assembly means any code path can inject arbitrary system prompts; server must assemble the final prompt |
| Compact format `[context: page=gateways]` | Max ~8-15 tokens; hard 30-token server-side budget; no verbose markdown blocks |
| Phase 5 (ACP backend) added | Backend must accept `{ prompt, pageContext }` as distinct fields; client-assembled prefixes are insecure |
| Error boundary renders degraded FAB | Provider throw must not break entire admin UI; "Chat unavailable" is better than a missing icon with no explanation |
| Drag via DOM ref + rAF | React setState at 60-120Hz during pointermove causes visible lag on Chromebooks; direct `style.transform` + commit on pointerup |
| pageContext generation counter → deterministic route key | Monotonic counter races under React 19 concurrent rendering; `[route, entityType, entityId].join('|')` is stable |
| CSS-hide on `/chat` (not close) | Popover state preserved; user navigating through `/chat` returns to same position/session |

## Files Modified

No files were modified in this session — this was a pure design/planning session. All output is in the bead tracker (`bd`).

**Beads created/updated:**

| Bead ID | Title | Action |
|---|---|---|
| `lab-gych` | Brainstorm: Floating Chat Popover | Created (epic) |
| `lab-gych.1` | Phase 1: Session Context Extraction | Created + 3× updated |
| `lab-gych.2` | Phase 2: FAB + Popover Shell | Created + 3× updated |
| `lab-gych.3` | Phase 3: Full Parity Wiring | Created + 3× updated |
| `lab-gych.4` | Phase 4: Design System Sandbox | Created |
| `lab-gych.5` | Phase 5: ACP Gateway pageContext support | Created |
| `lab-csvf` | Sidebar active session indicator | Created (backlog) |

**Files the implementation will touch (planned):**

- `apps/gateway-admin/lib/acp/fetch.ts` — NEW: standalone ACP fetch utility
- `apps/gateway-admin/lib/chat/chat-session-context.tsx` — NEW: 4-context provider
- `apps/gateway-admin/lib/chat/use-floating-chat.ts` — NEW: SSR-safe localStorage persistence hook
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` — REFACTOR: thin surface hook
- `apps/gateway-admin/lib/chat/use-session-events.ts` — UPDATE: atomic cache, incremental status, 404 error
- `apps/gateway-admin/components/chat/chat-shell.tsx` — MINOR: import updates
- `apps/gateway-admin/components/chat/chat-shell.test.tsx` — UPDATE: 4-param bootstrap test
- `apps/gateway-admin/components/chat/floating-chat-fab.tsx` — NEW: pill FAB + hotkey + ambient indicator
- `apps/gateway-admin/components/chat/floating-chat-popover.tsx` — NEW: draggable/resizable shell
- `apps/gateway-admin/components/chat/floating-chat-shell.tsx` — NEW: full wired chat surface
- `apps/gateway-admin/components/design-system/floating-chat-section.tsx` — NEW: sandbox docs
- `apps/gateway-admin/app/(admin)/layout.tsx` — UPDATE: error boundary + provider + openModals ref
- `apps/gateway-admin/app/(admin)/design-system/page.tsx` — UPDATE: add section
- `apps/gateway-admin/app/(admin)/gateways/page.tsx` — UPDATE: useSetChatPageContext
- `crates/lab/src/dispatch/acp/dispatch.rs` — UPDATE: pageContext field in prompt handler
- `crates/lab-apis/src/acp/types.rs` — UPDATE: pageContext in prompt request type

## Commands Executed

```bash
# Bead creation and management
bd create --title="Brainstorm: Floating Chat Popover" --type=epic --labels=brainstorm
bd create --title="Phase 1: Session Context Extraction" --type=task --parent=lab-gych
bd create --title="Phase 2: FAB + Popover Shell" --type=task --parent=lab-gych
bd create --title="Phase 3: Full Parity Wiring" --type=task --parent=lab-gych
bd create --title="Phase 4: Design System Sandbox" --type=task --parent=lab-gych
bd create --title="Phase 5: ACP Gateway pageContext support" --type=task --parent=lab-gych
bd create --title="Sidebar active session indicator" --type=task --priority=4  # lab-csvf

# Dependency wiring
bd dep add lab-gych.2 lab-gych.1
bd dep add lab-gych.3 lab-gych.1
bd dep add lab-gych.3 lab-gych.2
bd dep add lab-gych.3 lab-gych.5
bd dep add lab-gych.4 lab-gych.2
bd dep add lab-gych.5 lab-gych.1

# Validation
bd swarm validate lab-gych
# Result: 3 waves, max 2 parallel, Swarmable: YES
```

## Errors Encountered

None — pure planning session, no implementation commands run.

## Behavior Changes (Before/After)

No runtime behavior changed. Design artifacts only.

| Before | After |
|---|---|
| `/chat` is the only way to access chat | Floating FAB available on every admin page |
| No floating chat surface exists | `lab-gych` epic with 5 beads ready for `/lavra-work` |
| `ChatShell` owns all session state | 4-context provider planned; `ChatShell` becomes thin consumer |

## Risks and Rollback

- **Phase 1 is the highest risk bead** — refactoring `useChatSessionController` affects the only existing chat surface. The locked decision "ChatShell interface must not change" and the incremental migration strategy (wrap existing controller first, then refactor) mitigate this. Rollback: revert `chat-session-context.tsx` + restore `useChatSessionController` to original.
- **Phase 5 (ACP backend) is a Rust change** — touches `dispatch/acp/dispatch.rs`. Rollback: the feature degrades gracefully (absent `pageContext` field = no injection). The frontend change is additive; removing the backend change has no user-visible impact beyond disabling the context feature.
- **Phase 2 drag implementation** — no drag library currently installed. If `react-rnd` is chosen, adds a dependency. Custom pointer events adds complexity. Either path is reversible.

## Decisions Not Taken

| Alternative | Why Rejected |
|---|---|
| Facebook Messenger-style circular bubbles | Too consumer-generic for operator tool; pill FAB chosen |
| FAB draggable (only popover drags) | Simpler — FAB is always in the same corner; no need to reposition it |
| URL params for session state | No server, static export; localStorage is correct |
| Always-mounted `FloatingChatShell` | Eager SSE connection on every admin page load for users who never open chat |
| pageContext as prompt text prefix | Client-side system prompt assembly is insecure; server must own prompt construction |
| 3-context split (skip Connection context) | FAB would re-render ~4000× per streaming response; 4-context isolates it |
| Monotonic generation counter for pageContext | Races under React 19 concurrent rendering; deterministic route key used instead |
| Zustand for shared session state | Not installed; React Context is sufficient and already matches existing patterns |
| Single modal for both FAB + Command Palette | Both use `window.addEventListener` independently; modal stack guard prevents conflict |
| Close popover when navigating to /chat | CSS-hide chosen instead — preserves open state for when user navigates away |

## References

- `docs/design/design-system-contract.md` — Aurora token system, component contracts, elevation tiers
- `apps/gateway-admin/components/app-command-palette.tsx` — exact pattern precedent for layout-mounted hotkey UI
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` — source of truth for state to lift
- `apps/gateway-admin/lib/chat/use-session-events.ts` — SSE streaming hook with module-level caches
- react-rnd v10.5.3 (March 2026) — current, Next.js App Router callback ref fix in current release
- W3C APG Dialog Pattern — `role="dialog"`, `aria-modal`, focus trap, `inert` attribute
- MDN ResizeObserver — resize-end detection via 150ms debounce

## Open Questions

- Should `react-rnd` be added as a dependency, or should drag be implemented via custom pointer events? Both are in-scope per the plan; implementer has discretion.
- Should SWR be used for `/acp/sessions` and `/acp/provider` fetches inside the provider? It's already installed (`swr@^2.4.1`) and would give automatic deduplication, but adds SWR as a pattern in the chat domain.
- What is the exact ACP gateway API contract for the `pageContext` field in Phase 5? The bead specifies the shape but the Rust implementation details depend on how the ACP bridge forwards the system prompt to the underlying LLM.
- `apps/gateway-admin/app/dev/layout.tsx` — does the floating chat FAB need to appear on dev routes as well? The plan mounts it only in `(admin)/layout.tsx`.

## Next Steps

**Ready to implement (Wave 1):**
- `lab-gych.1` — Session Context Extraction (foundation for all other phases)

**Ready after Wave 1 (Wave 2, parallel):**
- `lab-gych.2` — FAB + Popover Shell (frontend)
- `lab-gych.5` — ACP Gateway pageContext support (Rust backend)

**Ready after Wave 2 (Wave 3, parallel):**
- `lab-gych.3` — Full Parity Wiring (frontend, requires lab-gych.1 + lab-gych.2 + lab-gych.5)
- `lab-gych.4` — Design System Sandbox (documentation)

**Backlog (post-ship):**
- `lab-csvf` — Sidebar active session indicator (requires `ChatSessionEventsContext` from lab-gych.1)

Start with: `/lavra-work lab-gych.1` or `/lavra-work lab-gych` (parallel execution)
