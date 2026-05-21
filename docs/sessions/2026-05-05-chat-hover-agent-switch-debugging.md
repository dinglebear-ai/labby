---
date: 2026-05-05 18:16:47 EDT
repo: git@github.com:jmagar/lab.git
branch: main
head: fcac4995
agent: Codex
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  fcac4995 [main]
---

# Chat Hover And Agent Switch Debugging

## User Request

Use the systematic-debugging skill to resolve the two remaining frontend verification issues from the eight chat plans: desktop hover did not reveal message actions/timestamps, and switching agents mid-conversation created a new session instead of switching inside the current chat session.

## Session Overview

Resolved both issues in the gateway-admin chat frontend. Message affordances now reveal through explicit message interaction state, and provider switching now reuses the selected ACP session while sending the requested provider in the prompt body.

## Sequence of Events

- Re-read the systematic-debugging workflow and inspected the affected chat components and tests.
- Confirmed the message bubble used Tailwind group hover classes while click/touch selection already worked.
- Confirmed the prompt controller created a provider-matched session whenever the selected provider differed from the selected run.
- Updated the behavior and tests for message reveal and provider switching.
- Rebuilt the static frontend and verified the real served chat page with `agent-browser`.

## Key Findings

- `apps/gateway-admin/components/chat/message-bubble.tsx` relied on Tailwind hover variants for under-bubble actions and timestamps, but headless browser verification reported `(hover: hover)` as false, so the generated `@media (hover:hover)` selector did not apply in `agent-browser`.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` reused the selected run only when the selected provider matched the run provider; otherwise it created a new session.
- The ACP API already supports provider switching through the prompt request body, so the frontend should send `provider` to `/v1/acp/sessions/{id}/prompt` instead of creating a new session.

## Technical Decisions

- Use explicit React `interactionOpen` state on message bubble mouse/focus events and expose it through `data-interaction-open`; this keeps hover/focus behavior deterministic while preserving mobile click/touch selected-state behavior.
- Keep selected ACP sessions stable during provider switches and include `provider` only when the selected provider differs from the selected run provider.
- Preserve the existing no-selected-session behavior: sending without an active run still creates a new session.

## Files Modified

- `apps/gateway-admin/components/chat/message-bubble.tsx` — added interaction state and changed reveal classes to data-attribute driven group selectors.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` — reused selected runs across provider changes and added provider to prompt payloads on switches.
- `apps/gateway-admin/components/chat/chat-shell.test.tsx` — updated provider-switch expectations and added no-selected-session coverage.
- `apps/gateway-admin/components/chat/message-bubble.test.tsx` — updated timestamp/action reveal class expectations.
- `apps/gateway-admin/components/chat/message-thread.test.tsx` — updated stable bubble markup expectation for the new reveal class.

## Commands Executed

- `pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx components/chat/message-thread.test.tsx components/chat/chat-shell.test.tsx` — passed after updating the regression expectations.
- `just web-build` — passed and regenerated the static chat frontend.
- `just chat-local` — served the local binary-backed UI at `http://127.0.0.1:8766/chat/`.
- `agent-browser` commands opened the chat UI, inspected message opacity, hovered a message bubble, switched the selected agent, sent a prompt, and inspected network requests.

## Errors Encountered

- First implementation used plain `group-hover:opacity-100`, but `agent-browser` headless Chromium reported no hover-capable media environment, so Tailwind's `@media (hover:hover)` rule still did not apply during automated verification.
- Fixed by moving hover/focus detection into React event state and using a data-attribute group selector that is not gated by hover media features.

## Behavior Changes

| Before | After |
| --- | --- |
| Desktop hover did not reveal message actions or timestamps in browser verification. | Hover/focus sets `data-interaction-open="true"` and reveals actions/timestamps. |
| Selecting Claude while on a Codex session created a new Claude session before prompting. | Selecting Claude keeps the current session and sends `provider: "claude-acp"` in the prompt body. |
| Tests encoded provider-matched session creation on provider mismatch. | Tests encode in-session provider switching and no-selected-session creation separately. |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx components/chat/message-thread.test.tsx components/chat/chat-shell.test.tsx` | Focused chat tests pass | 46 passed, 0 failed | Pass |
| `just web-build` | Static frontend builds | Next.js build compiled and generated 76 static pages | Pass |
| `agent-browser` hover opacity probe | Hovered message shows actions/timestamp | First message had `data-interaction-open="true"`, action opacity `1`, timestamp opacity `1` | Pass |
| `agent-browser` network probe after switching Codex to Claude | Prompt uses same session with provider body and no new session create | `POST /v1/acp/sessions/8670d1e8-2a86-4d76-833c-4b5c9a139f5d/prompt` body included `provider:"claude-acp"`; no `POST /v1/acp/sessions` | Pass |

## Risks and Rollback

- The message bubble now uses React hover/focus state, which adds small per-bubble state. Rollback is limited to `message-bubble.tsx` plus the two markup tests.
- Provider switching now depends on the backend prompt endpoint honoring `provider`; rollback is limited to `use-chat-session-controller.ts` and `chat-shell.test.tsx`, but would restore the unwanted new-session behavior.

## Decisions Not Taken

- Did not rely on Tailwind `group-hover` variants because the browser verification environment does not satisfy the hover media query even when `agent-browser hover` moves the mouse over the element.
- Did not change backend ACP switching because existing backend request/dispatch surfaces already support prompt-level provider switching.

## Open Questions

- The live prompt used a real Claude ACP subprocess during verification. No backend behavior issue was observed, but broader UX around displaying turn ownership after multiple provider switches may need separate product review.

## Next Steps

- No unfinished implementation work remains from this debugging pass.
