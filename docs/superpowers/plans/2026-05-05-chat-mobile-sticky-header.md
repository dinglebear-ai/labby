# Chat Mobile Sticky Header Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the `/chat` page header visible on mobile while the message thread scrolls, without overlapping message content or the prompt input.

**Architecture:** Preserve the existing chat shell shape: `ChatPage` renders `ChatShell`, `ChatShell` owns the page frame/header/sidebar/input, and `MessageThread` owns only the scrollable conversation viewport. The minimal fix should keep the header outside the scrollable area, give the mobile shell a reliable dynamic viewport height plus safe-area top handling, and add browser coverage that proves the message viewport scrolls while the header remains visible.

**Tech Stack:** Next.js App Router static export, React, TypeScript, Tailwind CSS utilities, Radix ScrollArea, Playwright browser tests via Node test runner.

---

## Context

Current code paths:

- `apps/gateway-admin/app/(admin)/chat/page.tsx`: route entry point; currently returns `<ChatShell />`.
- `apps/gateway-admin/components/chat/chat-shell.tsx`: chat page frame. The root is `flex h-dvh min-h-0 flex-col overflow-hidden bg-aurora-page-bg`; the header is a `h-12 shrink-0` sibling before the main flex body; `MessageThread` and `ChatInput` are stacked inside the body.
- `apps/gateway-admin/components/chat/message-thread.tsx`: wraps messages in `ScrollArea className="min-h-0 min-w-0 flex-1 overflow-hidden"` and auto-scrolls a bottom ref when messages change.
- `apps/gateway-admin/components/ui/scroll-area.tsx`: Radix ScrollArea root sets `relative overflow-hidden`; viewport is `size-full`.
- `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`: existing Playwright-backed browser coverage for `/chat` ACP flows, currently desktop viewport only.
- `apps/gateway-admin/package.json`: `pnpm test` maps to unit/component tests; `pnpm run test:browser` runs `node --test --experimental-strip-types lib/browser/**/*.test.ts`.

Research inputs:

- MDN `position`: sticky positioning needs at least one inset such as `top`, sticks relative to the nearest scrolling ancestor, and can create a new stacking context.
- MDN `env()`: `safe-area-inset-top` and related variables describe viewport-safe insets for device cutouts and browser UI; use them with fallbacks where the header sits at a viewport edge.
- web.dev viewport units: `dvh` tracks the dynamic viewport as browser UI expands or retracts, which is appropriate for full-height mobile app shells.
- Tailwind height docs: `h-dvh` maps to dynamic viewport height and is already in use in `ChatShell`.
- Existing repo precedent: `apps/gateway-admin/components/app-header.tsx` uses `sticky top-0 z-10` for regular admin page headers, while chat uses a dedicated full-height app shell.

Decision: HOLD SCOPE. This is a layout bug, not a chat redesign. Do not add a new shell abstraction, do not change ACP/session behavior, and do not modify desktop affordances unless required to preserve existing behavior.

## File Structure

- Modify `apps/gateway-admin/components/chat/chat-shell.tsx`: apply the minimal mobile layout classes and safe-area/header stacking changes.
- Modify `apps/gateway-admin/components/chat/message-thread.tsx`: only if needed to expose a stable test selector on the scrollable viewport or to preserve bottom padding when the input is present.
- Modify `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`: add a focused mobile viewport test that scrolls the message viewport and verifies the header remains visible and does not overlap the prompt input.
- Do not modify `.gitignore`.
- Do not modify backend, ACP provider, session controller, or floating chat code for this bead.

## Task 1: Add Failing Mobile Layout Coverage

**Files:**
- Modify: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`
- Test target: `/chat/` at a mobile viewport such as `390x844`

- [ ] **Step 1: Add a focused mobile browser test**

Add a test near the existing chat browser tests that:

```ts
test('chat page keeps the header visible while the mobile message thread scrolls', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, isMobile: true })
  const sessions: BrowserSession[] = [session('session-mobile', 'Mobile sticky header')]

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
      const body = Array.from({ length: 40 }, (_, index) =>
        sseFrame(bridgeEvent(sessionId, index + 1, { text: `Mobile message ${index + 1}` })),
      ).join('')

      await route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body,
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
  await page.getByText('Mobile sticky header').first().waitFor()
  await page.getByText('Mobile message 40').waitFor()

  const header = page.getByRole('banner').first()
  const input = page.getByRole('textbox', { name: 'Message' })
  const scrollViewport = page.locator('[data-slot="scroll-area-viewport"]').first()

  const before = await header.boundingBox()
  assert.ok(before, 'header should be measurable before scroll')

  await scrollViewport.evaluate((node) => {
    node.scrollTop = Math.max(0, node.scrollHeight / 2)
    node.dispatchEvent(new Event('scroll', { bubbles: true }))
  })

  const after = await header.boundingBox()
  const inputBox = await input.boundingBox()
  assert.ok(after, 'header should be measurable after scroll')
  assert.ok(inputBox, 'input should be measurable after scroll')
  assert.ok(after.y >= 0, `header should stay within the viewport, got y=${after.y}`)
  assert.ok(after.y < inputBox.y, 'header must not overlap the prompt input')
  assert.equal(Math.round(after.height), Math.round(before.height))
})
```

If the implementation adds a dedicated selector instead of relying on `[data-slot="scroll-area-viewport"]`, update the locator accordingly.

- [ ] **Step 2: Run the new browser test and confirm it fails or exposes the current bug**

Run:

```bash
cd apps/gateway-admin
pnpm run test:browser -- lib/browser/chat-shell.browser.test.ts
```

Expected before implementation: FAIL if the header scrolls offscreen or overlaps the mobile chat surface. If it unexpectedly passes, capture the current screenshot/geometry and continue with the smallest code change that makes the intended behavior explicit and regression-proof.

## Task 2: Apply Minimal Mobile Header/Layout Fix

**Files:**
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx`
- Modify only if needed: `apps/gateway-admin/components/chat/message-thread.tsx`

- [ ] **Step 1: Make the chat header explicitly persistent on mobile**

In `ChatShell`, keep the header as a sibling before the scrollable body and add mobile-safe persistence/stacking without changing the desktop visual contract:

```tsx
<header className="sticky top-0 z-20 flex h-12 shrink-0 items-center gap-2 border-b border-aurora-border-default bg-aurora-nav-bg px-2.5 pt-[env(safe-area-inset-top,0px)] sm:px-3 md:static md:z-auto md:pt-0">
```

If the added safe-area padding makes the mobile header too tall, split the safe-area padding onto a wrapper and keep the visible control row at `h-12`. The final layout must keep controls reachable without covering messages.

- [ ] **Step 2: Confirm the message area is the only vertical scroller**

Inspect the live DOM after the change:

- `ChatShell` root remains `h-dvh min-h-0 overflow-hidden`.
- The body below the header remains `flex min-h-0 flex-1`.
- `MessageThread` remains `flex-1` and scrolls via the Radix viewport.
- `ChatInput` remains visible below the message viewport.

Only change `MessageThread` if the test needs a more stable selector or if bottom spacing is required to avoid the last message hiding under the input.

- [ ] **Step 3: Run the focused browser test**

Run:

```bash
cd apps/gateway-admin
pnpm run test:browser -- lib/browser/chat-shell.browser.test.ts
```

Expected after implementation: PASS. The mobile test verifies the header is still visible after scrolling the message viewport and that the input remains below it.

## Task 3: Verify Existing Chat and Gateway Admin Tests

**Files:**
- No new files

- [ ] **Step 1: Run existing chat unit tests**

Run:

```bash
cd apps/gateway-admin
pnpm exec tsx --test components/chat/chat-shell.test.tsx components/chat/message-bubble.test.tsx components/chat/tool-call-presentation.test.ts
```

Expected: PASS. These should not change because the bead is layout-only.

- [ ] **Step 2: Run gateway-admin default test gate**

Run:

```bash
cd apps/gateway-admin
pnpm test
```

Expected: PASS. This is the default gateway-admin gate for repo-local frontend work.

- [ ] **Step 3: Optional manual viewport check**

Run:

```bash
cd apps/gateway-admin
LAB_ALLOWED_DEV_ORIGINS=127.0.0.1 NEXT_PUBLIC_MOCK_DATA=false pnpm exec next dev -H 127.0.0.1 -p 3103
```

Open `/chat/` at a mobile viewport (`390x844`). Expected: the chat header remains visible while messages scroll, session drawer overlay still covers below the header intentionally, and the prompt input remains usable.

## Verification Checklist

- [ ] Mobile viewport: header visible after message scroll.
- [ ] Mobile viewport: header does not overlap messages or prompt input.
- [ ] Mobile viewport: safe-area top does not clip controls on devices with a notch/cutout.
- [ ] Desktop viewport: header appearance and chat sizing remain unchanged or only intentionally improved.
- [ ] Session drawer still opens/closes on mobile.
- [ ] Settings panel remains usable.
- [ ] `pnpm run test:browser -- lib/browser/chat-shell.browser.test.ts` passes.
- [ ] `pnpm test` passes in `apps/gateway-admin`.

## Open Questions

- Does the bug reproduce because the page itself scrolls on a real mobile browser, or because the Radix viewport scroll is losing the header during dynamic viewport changes? The implementation should verify this with browser geometry before choosing between `sticky` and keeping the existing fixed-height sibling layout explicit.
- Should `safe-area-inset-top` change the total header height, or should the safe-area padding live in a wrapper above the fixed `h-12` control row? Prefer whichever preserves usable message/input space on `390x844`.
- If the existing browser test static export build is slow, keep the mobile assertion in `lib/browser/chat-shell.browser.test.ts` rather than adding a second browser harness.
