# ACP Chat UI Implementation Plan

Status: Active
Spec: `docs/superpowers/specs/2026-04-23-acp-chat-ui-design.md`
Scope: `apps/gateway-admin` `/chat` surface and exported assets consumed by `lab serve`

## Phase 1: Transcript-first rendering model

1. Keep `/chat` as a single conversation column.
2. Ensure assistant turns can materialize from ACP tool/reasoning events before assistant text arrives.
3. Remove any remaining primary split-pane activity presentation from the chat surface.

Acceptance:
- assistant turns render in a stable order
- inline reasoning and action flow attach to the correct turn
- the transcript remains the dominant reading surface

## Phase 2: Inline reasoning and action flow

1. Render thought chunks as a collapsible reasoning block inside the assistant turn.
2. Render tool activity as compact connected action rows.
3. Keep raw tool payloads behind expansion instead of showing them by default.
4. Prefer plain-language action labels over protocol-shaped titles where possible.

Acceptance:
- reasoning is secondary, collapsible, and readable
- action flow feels conversational rather than inspector-like
- action rows expand to raw input/output correctly

## Phase 3: Mobile behavior and shell responsiveness

1. Convert the session rail into a drawer on narrow screens.
2. Default mobile to the transcript after session selection/creation.
3. Tighten transcript width, spacing, and input behavior for small viewports.
4. Keep overflow ownership explicit and scrollbar styling aligned with Aurora.

Acceptance:
- mobile prioritizes transcript space
- the drawer does not remain unintentionally open after selection
- no collapsed or unusable scroll regions remain

## Phase 4: Reliability and degraded states

1. Guard `/provider` and `/sessions` fetches so auth or transport failures do not crash the page.
2. Preserve a usable empty state and unavailable-provider state.
3. Keep ACP runtime/auth wiring stable between frontend dev and Rust backend.

Acceptance:
- `/chat` does not throw on failed ACP responses
- unavailable ACP surfaces degrade cleanly
- the shell remains usable while disconnected

## Phase 5: Static export and binary-served `/chat`

1. Build `apps/gateway-admin` to `out/`.
2. Ensure `lab serve` serves the updated exported assets from its existing web-assets fallback.
3. Smoke-test `/chat` through the binary-served path after export.

Acceptance:
- exported `/chat` matches the current interaction model
- `lab serve` serves the updated static UI without a separate frontend server

## Verification

1. `pnpm build` in `apps/gateway-admin`
2. binary-served `/chat` responds successfully
3. ACP provider health is reachable from the configured backend

## Follow-up polish

1. richer inline artifacts for search/image-style tool rows
2. stronger reasoning summary treatment when duration metadata becomes available
3. optional hidden debug inspector for raw ACP events without affecting the primary UX
