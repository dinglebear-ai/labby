# Chat Prompt Input Scroll Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make long chat prompt text scroll inside the existing composer after the input reaches its max height.

**Architecture:** Keep the fix inside the current `ChatInput` composer. The textarea already autosizes from `scrollHeight` up to a 200px cap; the implementation should make the capped state internally scrollable without changing chat transport, session behavior, attachment flow, or the surrounding chat shell layout.

**Tech Stack:** Next.js 16 App Router, React 19, TypeScript, Tailwind CSS utilities, existing Aurora tokens, `node:test`, `tsx --test`, Playwright browser tests.

---

## Research And Repo Facts

- Bead: `lab-0v04`, "Make chat prompt input scroll when content overflows".
- Current composer: `apps/gateway-admin/components/chat/chat-input.tsx`.
- The textarea resize path is `handleInput` at `apps/gateway-admin/components/chat/chat-input.tsx:102-107`: it sets `height = auto`, then `height = min(scrollHeight, 200)`.
- The textarea markup is at `apps/gateway-admin/components/chat/chat-input.tsx:209-225`.
- Current bug source: the textarea has `overflow-hidden` at `apps/gateway-admin/components/chat/chat-input.tsx:220`, while `maxHeight` is `200px` at `apps/gateway-admin/components/chat/chat-input.tsx:224`.
- Chat controls sit outside the textarea in the same composer at `apps/gateway-admin/components/chat/chat-input.tsx:227-330`, so they should stay visible if only the textarea scrolls.
- `ChatShell` renders `MessageThread` then `ChatInput` as stacked siblings at `apps/gateway-admin/components/chat/chat-shell.tsx:198-213`.
- `apps/gateway-admin/components/ai/prompt-input.tsx` is a larger library-style component and is not the `/chat` route composer. Do not replace the current composer for this bug.
- Existing gateway-admin unit/component tests are run by `pnpm --dir apps/gateway-admin test:unit`.
- Existing browser tests live in `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts` and are run by `pnpm --dir apps/gateway-admin run test:browser`.
- MDN `<textarea>` docs confirm textarea is the right native multiline editing control and can have resize controlled by CSS: https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/textarea
- MDN `HTMLElement.scrollHeight` documents `scrollHeight` as the height needed to fit content without vertical scrollbars, matching the current autosize calculation: https://developer.mozilla.org/en-US/docs/Web/API/Element/scrollHeight
- MDN CSS `overflow` documents that overflow behavior matters only once the element has a constrained size such as `height` or `max-height`: https://developer.mozilla.org/en-US/docs/Web/CSS/overflow
- Testing Library `fireEvent.change` docs are relevant if an implementer adds DOM-level tests in a browser-like harness: https://testing-library.com/docs/dom-testing-library/api-events/

## CEO Review: HOLD SCOPE

This is a focused layout bug. The accepted scope is:

- Make the existing textarea grow only until the cap, then scroll internally.
- Preserve Enter-to-send and Shift+Enter newline behavior.
- Preserve attachment picker, agent picker, tools placeholder, send button, and disabled/sending states.
- Preserve desktop and mobile chat shell layout.
- Verify long prompt behavior with tests or visual/browser coverage.

Out of scope:

- Replacing `ChatInput` with `components/ai/prompt-input.tsx`.
- Changing ACP/session transport or prompt request payloads.
- Redesigning message thread, floating chat, settings, or provider selection.
- Changing `.gitignore`.

Failure modes to guard:

- The textarea still has `overflow-hidden` when capped, so text remains clipped.
- The textarea always shows a scrollbar before it reaches the cap.
- Sending clears value but leaves stale height or scroll position.
- Mobile viewport height grows because the outer composer expands instead of the textarea scrolling.
- Attachment, agent, and send controls shift, clip, or leave the viewport with long prompt content.

## Engineering Review

Architecture:

- Keep behavior in `ChatInput`; this is the owned boundary for composer DOM, keyboard handling, attachment UI, and agent/send controls.
- Add a tiny exported resize helper only if unit tests need a pure seam. Avoid a new hook or component abstraction unless the implementation proves it is necessary.
- Keep the current max height as the contract unless design review explicitly changes it.

Simplicity:

- The smallest likely fix is a class/style change from hidden overflow to vertical auto overflow once the textarea is capped.
- If conditional overflow is needed to avoid a visible scrollbar before the cap, set `overflowY` in the same resize helper based on `scrollHeight > maxHeight`.

Security:

- No new data leaves the browser.
- No prompt payload or attachment path behavior should change.
- No new HTML injection or markdown rendering surface is introduced.

Performance:

- `handleInput` already reads `scrollHeight` once per change. Keep this O(1).
- Do not add ResizeObserver, polling, or global listeners for this bug.

Failure Mode Table:

| Codepath | Failure Mode | User Sees | Test/Verification |
| --- | --- | --- | --- |
| `handleInput` long prompt | Height caps but overflow remains hidden | Cannot edit/read text past visible area | Unit helper test and browser scroll test |
| `handleInput` short prompt | Scrollbar appears while content fits | Unnecessary scrollbar, cramped input | Unit/static test for non-overflow state |
| `handleSend` reset | Height resets but scrollTop remains nonzero | Empty composer starts scrolled | Unit helper test or browser assertion after send |
| Mobile `/chat` layout | Composer grows past viewport | Send controls or thread clipped | Playwright mobile viewport geometry |
| Keyboard send | Enter behavior regresses | Prompt cannot send or newline semantics change | Existing focused send test plus manual/browser smoke |

## File Structure

- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
  - Keep current component ownership.
  - Introduce a named `CHAT_INPUT_MAX_HEIGHT_PX = 200` constant near the component.
  - Optionally introduce and export `resizeChatPromptTextarea(textarea: HTMLTextAreaElement): void` for focused tests.
  - Update textarea overflow behavior so content scrolls internally only after the cap.
  - Reset both height and scroll position after a successful send.
- Create: `apps/gateway-admin/components/chat/chat-input.test.tsx`
  - Test the pure resize helper and static rendered textarea classes/styles.
  - Keep tests in the existing `node:test` + `tsx --test` style.
- Modify: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`
  - Add one mobile long-prompt browser test if unit tests cannot prove real internal textarea scrolling.
  - Reuse the existing preview-server helpers and ACP mocks.
- Do not modify: `.gitignore`.
- Do not modify: backend Rust crates, ACP dispatch, `ChatShell` transport logic, or `apps/gateway-admin/components/ai/prompt-input.tsx`.

## TDD Tasks

### Task 1: Add Focused Composer Overflow Unit Coverage

**Files:**
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx:36`
- Create: `apps/gateway-admin/components/chat/chat-input.test.tsx`

- [ ] **Step 1: Write the failing helper tests**

Create `apps/gateway-admin/components/chat/chat-input.test.tsx`:

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import {
  CHAT_INPUT_MAX_HEIGHT_PX,
  ChatInput,
  resizeChatPromptTextarea,
} from './chat-input'

function textarea(scrollHeight: number) {
  const style: Record<string, string> = {}
  return {
    scrollHeight,
    scrollTop: 12,
    style,
  } as unknown as HTMLTextAreaElement
}

const selectedAgent = {
  id: 'codex-acp',
  name: 'Codex ACP',
  description: 'codex-acp over local ACP bridge',
  version: 'live',
  capabilities: [],
}

test('resizeChatPromptTextarea grows until max height then enables internal scroll', () => {
  const el = textarea(CHAT_INPUT_MAX_HEIGHT_PX + 80)

  resizeChatPromptTextarea(el)

  assert.equal(el.style.height, `${CHAT_INPUT_MAX_HEIGHT_PX}px`)
  assert.equal(el.style.overflowY, 'auto')
})

test('resizeChatPromptTextarea hides vertical overflow while content fits', () => {
  const el = textarea(88)

  resizeChatPromptTextarea(el)

  assert.equal(el.style.height, '88px')
  assert.equal(el.style.overflowY, 'hidden')
})

test('chat input textarea renders with max height and without whole-composer overflow behavior', () => {
  const markup = renderToStaticMarkup(
    <ChatInput
      onSend={() => {}}
      selectedAgent={selectedAgent}
      agents={[selectedAgent]}
      onSelectAgent={() => {}}
    />,
  )

  assert.match(markup, /aria-label="Message"/)
  assert.match(markup, /max-height:200px/)
  assert.doesNotMatch(markup, /overflow-hidden bg-transparent/)
})
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/chat-input.test.tsx
```

Expected: FAIL because `CHAT_INPUT_MAX_HEIGHT_PX` and `resizeChatPromptTextarea` do not exist, and the textarea still uses hidden overflow in its static class list.

### Task 2: Implement Minimal Internal Scroll Behavior

**Files:**
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx:36-107`
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx:219-225`

- [ ] **Step 1: Add the constant and resize helper**

Add near the prop types:

```tsx
export const CHAT_INPUT_MAX_HEIGHT_PX = 200

export function resizeChatPromptTextarea(el: HTMLTextAreaElement) {
  el.style.height = 'auto'
  const nextHeight = Math.min(el.scrollHeight, CHAT_INPUT_MAX_HEIGHT_PX)
  el.style.height = `${nextHeight}px`
  el.style.overflowY = el.scrollHeight > CHAT_INPUT_MAX_HEIGHT_PX ? 'auto' : 'hidden'
}
```

- [ ] **Step 2: Use the helper from `handleInput`**

Replace the inline resize logic:

```tsx
const handleInput = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
  setValue(e.target.value)
  resizeChatPromptTextarea(e.target)
}
```

- [ ] **Step 3: Reset height and scroll position after send**

Inside the existing `if (textareaRef.current)` block after successful send:

```tsx
textareaRef.current.style.height = 'auto'
textareaRef.current.style.overflowY = 'hidden'
textareaRef.current.scrollTop = 0
```

- [ ] **Step 4: Update textarea classes and style**

Change the textarea class from hidden overflow to a vertical-scroll-ready state. Prefer conditional helper-owned overflow if using `style.overflowY`; keep the static class from forcing hidden clipping:

```tsx
'w-full resize-none bg-transparent px-3 pt-2.5 pb-1.5 text-[13px] leading-[1.55] sm:px-4 sm:pt-3 sm:pb-2',
```

Keep the max height wired to the constant:

```tsx
style={{ minHeight: '44px', maxHeight: `${CHAT_INPUT_MAX_HEIGHT_PX}px`, overflowY: 'hidden' }}
```

- [ ] **Step 5: Run the focused unit test and verify it passes**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/chat-input.test.tsx
```

Expected: PASS for all `chat-input.test.tsx` tests.

### Task 3: Add Browser Coverage For Real Internal Scroll

**Files:**
- Modify: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Add a mobile long-prompt browser test**

Add a focused test after the existing send/bootstrap coverage. Reuse `startPreviewServer`, `mockAuthenticatedSession`, `session`, `bridgeEvent`, and `sseFrame` patterns already in the file.

```ts
test('chat prompt scrolls internally after reaching max height on mobile', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, isMobile: true })
  const sessions: BrowserSession[] = [session('session-long-prompt', 'Long prompt session')]

  await mockAuthenticatedSession(page)
  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          provider: {
            provider: 'codex',
            ready: true,
            command: 'npx',
            args: ['@zed-industries/codex-acp'],
            message: 'ready',
          },
        }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ sessions }),
      })
      return
    }

    const ticketMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/subscribe_ticket$/)
    if (ticketMatch && request.method() === 'POST') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ticket: `ticket-${decodeURIComponent(ticketMatch[1]!)}` }),
      })
      return
    }

    const eventMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/events$/)
    if (eventMatch && request.method() === 'GET') {
      const sessionId = decodeURIComponent(eventMatch[1]!)
      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body: sseFrame(bridgeEvent(sessionId, 1, { text: 'Ready for a long prompt' })),
      })
      return
    }

    await route.fulfill({
      status: 404,
      contentType: 'application/json',
      body: JSON.stringify({ message: `Unhandled ACP request: ${url.pathname}` }),
    })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })
  const input = page.getByRole('textbox', { name: 'Message' })
  await input.fill(Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join('\n'))

  const metrics = await input.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement
    textarea.scrollTop = textarea.scrollHeight
    return {
      clientHeight: textarea.clientHeight,
      scrollHeight: textarea.scrollHeight,
      scrollTop: textarea.scrollTop,
      overflowY: getComputedStyle(textarea).overflowY,
    }
  })

  assert.ok(metrics.clientHeight <= 200, `textarea should cap at 200px, got ${metrics.clientHeight}`)
  assert.ok(metrics.scrollHeight > metrics.clientHeight, 'textarea should have overflow content')
  assert.ok(metrics.scrollTop > 0, 'textarea should allow internal scrolling')
  assert.equal(metrics.overflowY, 'auto')

  const sendButton = page.getByRole('button', { name: 'Send message' })
  const inputBox = await input.boundingBox()
  const sendBox = await sendButton.boundingBox()
  assert.ok(inputBox, 'input should be measurable')
  assert.ok(sendBox, 'send button should be measurable')
  assert.ok(sendBox.y >= inputBox.y, 'send button should remain aligned below the scrollable textarea')
})
```

- [ ] **Step 2: Run the browser test and verify it fails before implementation, passes after implementation**

Run:

```bash
pnpm --dir apps/gateway-admin run test:browser -- lib/browser/chat-shell.browser.test.ts
```

Expected before implementation: FAIL because the textarea does not allow internal scrolling or reports hidden overflow.

Expected after implementation: PASS, with `scrollHeight > clientHeight`, `scrollTop > 0`, and the send button measurable and aligned.

### Task 4: Run The Focused And Normal Gateway-Admin Gates

**Files:**
- No edits.

- [ ] **Step 1: Run focused component test**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/chat-input.test.tsx
```

Expected: PASS.

- [ ] **Step 2: Run full gateway-admin unit suite**

Run:

```bash
pnpm --dir apps/gateway-admin test:unit
```

Expected: PASS for all unit/component tests.

- [ ] **Step 3: Run browser chat coverage if Task 3 was added**

Run:

```bash
pnpm --dir apps/gateway-admin run test:browser -- lib/browser/chat-shell.browser.test.ts
```

Expected: PASS for the chat browser tests, including the long-prompt internal-scroll test.

- [ ] **Step 4: Manual smoke check on mobile viewport**

Run:

```bash
pnpm --dir apps/gateway-admin dev
```

Open `/chat/` at about `390x844`, type or paste 30 newline-separated lines, and verify:

- The composer height stops near 200px.
- The textarea scrolls internally with mouse wheel, trackpad, keyboard, and touch.
- Send, attachment, tools, and agent controls remain visible.
- Shift+Enter inserts a newline.
- Enter sends.
- After send, the input clears, returns to one-line height, and `scrollTop` is 0.

## Final Validation Checklist

- [ ] `apps/gateway-admin/components/chat/chat-input.tsx` is the only product code modified unless browser coverage requires a test selector elsewhere.
- [ ] `.gitignore` is unchanged.
- [ ] No backend, ACP dispatch, or prompt payload code is modified.
- [ ] No replacement with `apps/gateway-admin/components/ai/prompt-input.tsx`.
- [ ] Long prompt content remains editable after the 200px cap.
- [ ] Mobile viewport does not overflow vertically or horizontally due to the prompt.
- [ ] All focused and relevant gateway-admin tests pass.
