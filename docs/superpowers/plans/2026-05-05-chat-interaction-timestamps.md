# Chat Interaction Timestamps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reveal each chat message timestamp only during interaction: desktop hover/focus and mobile touch/selection.

**Architecture:** Use the timestamp metadata already present on `ACPMessage.createdAt`. Keep timestamp formatting and rendering inside the existing chat bubble surface, while `MessageThread` owns one selected message id for touch/mobile reveal and dismissal. Render the timestamp as a reserved under-bubble metadata row so it cannot overlap message text, copy actions, reasoning panels, agent actions, or adjacent bubbles.

**Tech Stack:** Next.js 16, React 19, TypeScript, Tailwind/Aurora classes, existing `formatUiTime` and `formatUiDateTime`, `node:test`, `tsx --test`, `react-dom/server`, `react-dom/client`.

---

## Research And Repo Facts

- Bead: `lab-80qg` - Show chat message timestamps on interaction.
- Current message type already has `createdAt: Date` in `apps/gateway-admin/components/chat/types.ts:76-82`.
- Current bubble rendering lives in `apps/gateway-admin/components/chat/message-bubble.tsx:163-292`.
- Existing copy action is `CopyButton` in `apps/gateway-admin/components/chat/message-bubble.tsx:25-49`, rendered absolutely at `apps/gateway-admin/components/chat/message-bubble.tsx:264-266`.
- Current message list maps directly to bubbles in `apps/gateway-admin/components/chat/message-thread.tsx:87-89`; it has no selected-message state yet.
- Existing timestamp helpers live in `apps/gateway-admin/lib/format-ui-time.ts:47-60`; use `formatUiTime` for visible timestamp text and `formatUiDateTime` for accessible/title detail.
- Existing focused component tests live in `apps/gateway-admin/components/chat/message-bubble.test.tsx`.
- Gateway-admin unit tests run with `pnpm --dir apps/gateway-admin test:unit`; focused bubble tests run with `pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx`.
- WCAG 1.4.13 requires content shown on hover/focus to be dismissible, hoverable, and persistent: https://w3c.github.io/wcag/understanding/content-on-hover-or-focus.html
- MDN notes that many mobile primary input mechanisms cannot hover conveniently, so touch devices need an explicit non-hover reveal path: https://developer.mozilla.org/en-US/docs/Web/CSS/%40media/hover

## Hold Scope Decisions

- Do not add backend fields, API changes, message grouping, date separators, relative time headings, user preferences, or a tooltip/popover dependency.
- Do not refactor the existing copy action into a larger message action toolbar in this bead.
- Do not make timestamps permanently visible.
- Do reveal timestamps for keyboard users via focus-within, not hover only.
- Do use selected/touched state for mobile; CSS hover alone is insufficient.
- Do preserve `.gitignore` and unrelated worktree changes.

## File Structure

- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
  - Import `formatUiTime` and `formatUiDateTime`.
  - Add pure timestamp formatting/reveal helper exports.
  - Accept optional timestamp reveal props from `MessageThread`.
  - Render the timestamp row below the bubble content, aligned with the message side.
  - Include desktop reveal classes for `group-hover/bubble` and `group-focus-within/bubble`.
  - Include selected-state reveal for mobile/touch.
  - Update memo comparison to include `createdAt`.
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx`
  - Add tests for timestamp formatting, desktop reveal classes, selected reveal, invalid timestamp fallback, and memo comparison behavior if exported.
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx`
  - Add `selectedMessageId` state.
  - Pass selected state and selection handlers to `MessageBubble`.
  - Clear selection on Escape.
  - Clear selection when clicking/tapping outside the selected bubble.
- Create: `apps/gateway-admin/components/chat/message-thread.test.tsx`
  - Test mobile/touch selection, selecting another message, copy button non-interference, Escape dismissal, and outside-click dismissal.
- Optional browser verification only if layout risk remains after unit tests:
  - Modify: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`
  - Add viewport checks for mobile and desktop timestamp reveal without overlap.

## Behavior Contract

- A message with a valid `createdAt` can reveal a timestamp on interaction.
- Desktop: timestamp appears on bubble hover and focus-within.
- Keyboard: tabbing into the bubble/copy button reveals the timestamp.
- Mobile/touch: tapping/selecting a bubble reveals its timestamp; only one selected message is open.
- Selection clears on Escape, outside click/tap, or selecting a different bubble.
- Timestamp row is below the bubble, not absolutely over message text.
- Timestamp does not crowd the existing copy button at top-right.
- Invalid or missing timestamp renders no visible timestamp or a consistent `Unknown` label; pick one and test it. Preferred: omit the row when the formatter returns `Unknown`.
- Streaming messages use the same `createdAt` behavior; no timer updates are needed.

## TDD Tasks

### Task 1: Add Pure Timestamp Presentation Helpers

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx`

- [ ] **Step 1: Write failing helper tests**

Add tests near the existing copy text test:

```tsx
import {
  MessageBubble,
  getMessageCopyText,
  getMessageTimestampLabels,
  shouldRenderMessageTimestamp,
} from './message-bubble'

test('formats message timestamp labels from createdAt metadata', () => {
  const labels = getMessageTimestampLabels(userMessage({
    createdAt: new Date('2026-05-04T12:34:00Z'),
  }))

  assert.equal(labels.visible, '12:34 PM UTC')
  assert.equal(labels.detail, 'May 4, 2026, 12:34 PM UTC')
})

test('omits timestamp presentation when createdAt is invalid', () => {
  const message = userMessage({ createdAt: new Date(Number.NaN) })

  assert.equal(shouldRenderMessageTimestamp(message), false)
})
```

- [ ] **Step 2: Run focused test and verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: FAIL with `getMessageTimestampLabels` and `shouldRenderMessageTimestamp` not exported.

- [ ] **Step 3: Implement minimal helpers**

In `apps/gateway-admin/components/chat/message-bubble.tsx`, add:

```tsx
import { formatUiDateTime, formatUiTime } from '@/lib/format-ui-time'

export type MessageTimestampLabels = {
  visible: string
  detail: string
}

function hasValidDate(value: Date) {
  return !Number.isNaN(value.getTime())
}

export function shouldRenderMessageTimestamp(message: Pick<ACPMessage, 'createdAt'>) {
  return hasValidDate(message.createdAt)
}

export function getMessageTimestampLabels(
  message: Pick<ACPMessage, 'createdAt'>,
): MessageTimestampLabels {
  return {
    visible: formatUiTime(message.createdAt),
    detail: formatUiDateTime(message.createdAt),
  }
}
```

- [ ] **Step 4: Run focused test and verify it passes**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: PASS for all existing and new `message-bubble.test.tsx` tests.

### Task 2: Render A Non-Overlapping Timestamp Row

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx`

- [ ] **Step 1: Write failing render tests**

Add tests:

```tsx
test('renders timestamp row after message content without overlapping copy action', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble message={userMessage({ createdAt: new Date('2026-05-04T12:34:00Z') })} />,
  )

  assert.match(markup, /aria-label="Message sent at May 4, 2026, 12:34 PM UTC"/)
  assert.match(markup, /12:34 PM UTC/)
  assert.match(markup, /data-message-timestamp/)
  assert.match(markup, /group-hover\/bubble:opacity-100/)
  assert.match(markup, /group-focus-within\/bubble:opacity-100/)
  assert.ok(
    markup.indexOf('Hello.') < markup.indexOf('data-message-timestamp'),
    'timestamp should render after the message text',
  )
  assert.ok(
    markup.indexOf('Copy message') < markup.indexOf('data-message-timestamp'),
    'timestamp row should not share the absolute copy-button slot',
  )
})

test('aligns assistant and user timestamp rows with their bubbles', () => {
  const assistantMarkup = renderToStaticMarkup(
    <MessageBubble message={assistantMessage({ isStreaming: false, thoughts: [], toolCalls: [] })} />,
  )
  const userMarkup = renderToStaticMarkup(<MessageBubble message={userMessage()} />)

  assert.match(assistantMarkup, /items-start|self-start/)
  assert.match(userMarkup, /items-end|self-end/)
})
```

- [ ] **Step 2: Run focused test and verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: FAIL because no timestamp row exists.

- [ ] **Step 3: Add timestamp row component**

Add a small local component:

```tsx
function MessageTimestamp({
  message,
  selected,
}: {
  message: ACPMessage
  selected?: boolean
}) {
  if (!shouldRenderMessageTimestamp(message)) return null

  const labels = getMessageTimestampLabels(message)

  return (
    <div
      data-message-timestamp
      aria-label={`Message sent at ${labels.detail}`}
      title={labels.detail}
      className={cn(
        'min-h-4 text-[11px] leading-4 text-aurora-text-muted/60 transition-opacity duration-150',
        'opacity-0 group-hover/bubble:opacity-100 group-focus-within/bubble:opacity-100',
        selected && 'opacity-100',
      )}
    >
      {labels.visible}
    </div>
  )
}
```

Add props:

```tsx
type MessageBubbleProps = {
  message: ACPMessage
  selected?: boolean
  onSelect?: (messageId: string) => void
}
```

Render below the bubble, not inside the absolute copy button slot:

```tsx
{message.text && (
  <>
    <div className={cn('relative max-w-full overflow-hidden rounded-aurora-2 px-4 py-3', ...)}>
      ...
    </div>
    <MessageTimestamp message={message} selected={selected} />
  </>
)}
```

Keep the row inside the existing flex column so user messages inherit `items-end` and assistant messages inherit default start alignment.

- [ ] **Step 4: Run focused test and verify it passes**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: PASS.

### Task 3: Add Touch Selection State In MessageThread

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx`
- Create: `apps/gateway-admin/components/chat/message-thread.test.tsx`
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`

- [ ] **Step 1: Write failing client interaction tests**

Create `apps/gateway-admin/components/chat/message-thread.test.tsx` with a `react-dom/client` test harness following existing component test style:

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { createRoot } from 'react-dom/client'

import { MessageThread } from './message-thread'
import type { ACPMessage, ACPRun } from './types'

function run(): ACPRun {
  return {
    id: 'run-1',
    projectId: 'project-1',
    agentId: 'agent-1',
    provider: 'codex',
    title: 'Run',
    createdAt: new Date('2026-05-04T12:00:00Z'),
    updatedAt: new Date('2026-05-04T12:00:00Z'),
    status: 'idle',
    providerSessionId: 'provider-run-1',
    cwd: '/home/jmagar/workspace/lab',
  }
}

function message(id: string, text: string, minute: string): ACPMessage {
  return {
    id,
    runId: 'run-1',
    role: 'user',
    text,
    createdAt: new Date(`2026-05-04T12:${minute}:00Z`),
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
  }
}

test('touching a message reveals only that message timestamp', async () => {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)

  await act(async () => {
    root.render(<MessageThread run={run()} messages={[message('m1', 'First', '10'), message('m2', 'Second', '20')]} />)
  })

  const first = host.querySelector('[data-message-id="m1"]') as HTMLElement
  const second = host.querySelector('[data-message-id="m2"]') as HTMLElement

  await act(async () => {
    first.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })

  assert.match(first.outerHTML, /opacity-100/)
  assert.doesNotMatch(second.outerHTML, /opacity-100/)

  await act(async () => {
    second.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })

  assert.doesNotMatch(first.outerHTML, /opacity-100/)
  assert.match(second.outerHTML, /opacity-100/)

  root.unmount()
  host.remove()
})
```

- [ ] **Step 2: Run the new test and verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-thread.test.tsx
```

Expected: FAIL because `MessageThread` has no selected-message state and bubbles have no `data-message-id`.

- [ ] **Step 3: Implement selection wiring**

In `MessageBubble`, add wrapper attributes and handler:

```tsx
function MessageBubbleComponent({ message, selected = false, onSelect }: MessageBubbleProps) {
  ...
  return (
    <div
      data-message-id={message.id}
      className={cn('group/bubble flex min-w-0 gap-3', isUser && 'flex-row-reverse')}
      onPointerDown={(event) => {
        const target = event.target as HTMLElement
        if (target.closest('button,a,input,textarea,select,[role="button"]')) return
        onSelect?.(message.id)
      }}
    >
      ...
    </div>
  )
}
```

In `MessageThread`, add:

```tsx
const [selectedMessageId, setSelectedMessageId] = React.useState<string | null>(null)

React.useEffect(() => {
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') setSelectedMessageId(null)
  }
  window.addEventListener('keydown', onKeyDown)
  return () => window.removeEventListener('keydown', onKeyDown)
}, [])
```

Pass props:

```tsx
<MessageBubble
  key={message.id}
  message={message}
  selected={selectedMessageId === message.id}
  onSelect={setSelectedMessageId}
/>
```

- [ ] **Step 4: Add outside-click dismissal test**

Extend `message-thread.test.tsx`:

```tsx
test('outside pointer clears selected timestamp', async () => {
  ...
  await act(async () => {
    first.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.match(first.outerHTML, /opacity-100/)

  await act(async () => {
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })

  assert.doesNotMatch(first.outerHTML, /opacity-100/)
})
```

- [ ] **Step 5: Implement outside-click dismissal**

In `MessageThread`, wrap the list in a ref and register a document pointer handler:

```tsx
const threadRef = React.useRef<HTMLDivElement>(null)

React.useEffect(() => {
  const onPointerDown = (event: PointerEvent) => {
    if (!threadRef.current?.contains(event.target as Node)) {
      setSelectedMessageId(null)
    }
  }
  document.addEventListener('pointerdown', onPointerDown)
  return () => document.removeEventListener('pointerdown', onPointerDown)
}, [])
```

Attach `ref={threadRef}` to the inner transcript container.

- [ ] **Step 6: Run thread tests and verify they pass**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-thread.test.tsx
```

Expected: PASS.

### Task 4: Preserve Memoization Correctness

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx`

- [ ] **Step 1: Write failing comparator test**

Export the comparator only if acceptable for local tests:

```tsx
export function areMessageBubblePropsEqual(...)
```

Add:

```tsx
test('message bubble memo comparison includes timestamp and selected state', () => {
  const base = userMessage({ createdAt: new Date('2026-05-04T12:00:00Z') })
  const changedTimestamp = { ...base, createdAt: new Date('2026-05-04T12:01:00Z') }

  assert.equal(
    areMessageBubblePropsEqual({ message: base, selected: false }, { message: changedTimestamp, selected: false }),
    false,
  )
  assert.equal(
    areMessageBubblePropsEqual({ message: base, selected: false }, { message: base, selected: true }),
    false,
  )
})
```

- [ ] **Step 2: Run focused test and verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: FAIL because comparator currently compares message fields but not `createdAt` or `selected`.

- [ ] **Step 3: Update comparator**

Update comparator input type to `Readonly<MessageBubbleProps>` and include:

```tsx
previous.selected === next.selected &&
prev.createdAt.getTime() === current.createdAt.getTime()
```

Keep existing checks for id, role, text, streaming, version, thoughts length, and tool call length.

- [ ] **Step 4: Run focused test and verify it passes**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: PASS.

### Task 5: Verify Full Gateway-Admin Unit Coverage

**Files:**
- No code files beyond Tasks 1-4.

- [ ] **Step 1: Run focused chat tests**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx components/chat/message-thread.test.tsx
```

Expected: PASS.

- [ ] **Step 2: Run full gateway-admin unit suite**

Run:

```bash
pnpm --dir apps/gateway-admin test:unit
```

Expected: PASS.

- [ ] **Step 3: Run lint if class/prop changes are non-trivial**

Run:

```bash
pnpm --dir apps/gateway-admin lint
```

Expected: PASS.

### Task 6: Optional Browser Layout Verification

Only do this if component tests leave overlap risk unresolved or the implementation changes transcript layout beyond the row described above.

**Files:**
- Optional modify: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Add desktop hover/focus visual assertions**

Use the existing Playwright setup in `chat-shell.browser.test.ts`. Add a test that loads `/chat/`, injects a session event with `createdAt`, hovers/focuses a message, and asserts the timestamp is visible.

Expected checks:
- Desktop viewport `1360x960`.
- Timestamp is hidden before hover.
- Timestamp is visible after hover.
- Timestamp bounding box does not intersect message text bounding box or copy button bounding box.

- [ ] **Step 2: Add mobile touch visual assertions**

Use a mobile viewport such as `390x844`.

Expected checks:
- Timestamp is hidden before touch.
- Tapping the first message reveals the first timestamp.
- Tapping the second message hides the first timestamp and reveals the second.
- Timestamp bounding box stays below the bubble and within viewport width.

- [ ] **Step 3: Run browser test**

Run:

```bash
pnpm --dir apps/gateway-admin test:browser
```

Expected: PASS. Note that this command builds the static Next export before serving `out/`.

## Final Verification

- [ ] `bd show lab-80qg --json`
  - Expected: bead remains open, has `plan-reviewed` and `research-reviewed` labels, and design notes reference this plan file.
- [ ] `test -f docs/superpowers/plans/2026-05-05-chat-interaction-timestamps.md`
  - Expected: exit code 0.
- [ ] `git diff -- docs/superpowers/plans/2026-05-05-chat-interaction-timestamps.md`
  - Expected: only this plan file content for this bead.

## Not In Scope

- Product-code implementation during planning.
- Committing changes.
- Editing `.gitignore`.
- Backend/API work.
- Message grouping/date separators.
- Always-visible timestamps.
- Replacing the current copy button or implementing the larger message action plan.
