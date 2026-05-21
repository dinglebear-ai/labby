# Chat Message Actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add contextual copy, retry, and edit actions under chat message bubbles with desktop hover/focus behavior and mobile touch selection behavior.

**Architecture:** Keep message action rendering colocated with the chat bubble, but keep transport behavior above it. `MessageBubble` renders a right-aligned action row from pure availability data and callbacks; `MessageThread` owns selected-message state and passes action callbacks from `ChatShell`; `ChatShell` reuses existing chat-session actions for retry/edit instead of duplicating ACP fetch logic.

**Tech Stack:** Next.js 16, React 19, TypeScript, lucide-react icons, existing Aurora/shadcn-style `Button`, `node:test`, `tsx --test`, `react-dom/server`, `react-dom/client`.

---

## Research And Repo Facts

- Current bubble rendering lives in `apps/gateway-admin/components/chat/message-bubble.tsx`.
- The existing copy button is `CopyButton` in `message-bubble.tsx:25` and is rendered absolutely inside the bubble at `message-bubble.tsx:264`.
- `MessageThread` maps each message directly to `MessageBubble` in `apps/gateway-admin/components/chat/message-thread.tsx:87`.
- Existing send/retry-reusable prompt flow is `sendPromptForSelectedProvider` in `apps/gateway-admin/lib/chat/use-chat-session-controller.ts:141`.
- Current message type `ACPMessage` stores `text`, `role`, streaming state, thoughts, tool calls, and version, but not original prompt attachments.
- Existing component tests are `node:test` files run by `pnpm --dir apps/gateway-admin test:unit`.
- W3C WCAG 1.4.13 requires hover/focus-triggered additional content to be dismissible, hoverable, and persistent: https://w3c.github.io/wcag/understanding/content-on-hover-or-focus.html
- MDN documents `navigator.clipboard.writeText` as secure-context-only, Promise-returning, and able to throw `NotAllowedError`: https://developer.mozilla.org/en-US/docs/Web/API/Clipboard/writeText
- WAI-ARIA APG menu-button guidance only becomes necessary if these actions move into an overflow action menu; the initial scoped row should use plain buttons: https://www.w3.org/WAI/ARIA/apg/patterns/menu-button/

## File Structure

- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx`
  - Remove the current absolute in-bubble copy affordance.
  - Add pure action availability helpers.
  - Add `MessageActionToolbar` with Copy, Retry, and Edit buttons.
  - Render the toolbar under the bubble, aligned to the right.
  - Keep desktop reveal via hover/focus classes and selected state.
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx`
  - Add server-render tests for action availability, accessible labels, right-aligned under-bubble placement, and invalid action omission.
  - Add client interaction tests for copy success/failure and action callback dispatch.
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx`
  - Add `selectedMessageId` state for touch/mobile selection.
  - Pass selection and action callbacks into `MessageBubble`.
  - Add Escape/outside-click dismissal only while a message is selected.
- Create: `apps/gateway-admin/components/chat/message-thread.test.tsx`
  - Test mobile/touch selection, selecting another message, and Escape dismissal.
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx`
  - Wire retry/edit callbacks from chat shell into `MessageThread`.
  - Retry should call the existing prompt send path with the selected user message text.
  - Edit should populate the composer draft before showing the action; if no draft setter exists yet, add the setter in the same task before exposing Edit.
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
  - Only if needed for edit: accept a controlled draft value or imperative draft setter through `ChatShell`.
- Modify: `apps/gateway-admin/components/chat/chat-shell.test.tsx`
  - Add pure callback tests for retry/edit wiring if `ChatShell` exports helper functions.

## Behavior Contract

- Copy appears for messages with non-empty `text`.
- Retry appears only for user messages with non-empty `text` and a run/session state that can accept a prompt.
- Edit appears only for user messages with non-empty `text` and a real composer draft path.
- Invalid retry/edit actions are omitted, not disabled, for the initial scope.
- Desktop action row is visually hidden until bubble hover or focus-within, then remains usable while the row itself is hovered or focused.
- Mobile action row appears only after the bubble is touched/selected and can be dismissed by Escape, outside click, or selecting another bubble.
- Copy writes raw message text, not rendered markdown text.
- Copy failure is visible through button state or accessible label, not silent.

## TDD Tasks

### Task 1: Add Pure Message Action Availability

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx:23`
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx:190`

- [ ] **Step 1: Write failing availability tests**

Add tests near the existing copy text test:

```tsx
test('derives message actions from role, text, and callback availability', () => {
  assert.deepEqual(
    getMessageActionAvailability(userMessage({ text: 'Retry me.' }), {
      canRetry: true,
      canEdit: true,
    }),
    { copy: true, retry: true, edit: true },
  )

  assert.deepEqual(
    getMessageActionAvailability(assistantMessage({ text: 'Assistant.' }), {
      canRetry: true,
      canEdit: true,
    }),
    { copy: true, retry: false, edit: false },
  )

  assert.deepEqual(
    getMessageActionAvailability(userMessage({ text: '   ' }), {
      canRetry: true,
      canEdit: true,
    }),
    { copy: false, retry: false, edit: false },
  )
})
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: FAIL with `getMessageActionAvailability` not exported or not defined.

- [ ] **Step 3: Implement the pure helper**

Add this export after `getMessageCopyText`:

```tsx
export type MessageActionAvailabilityInput = {
  canRetry?: boolean
  canEdit?: boolean
}

export type MessageActionAvailability = {
  copy: boolean
  retry: boolean
  edit: boolean
}

export function getMessageActionAvailability(
  message: Pick<ACPMessage, 'role' | 'text' | 'isStreaming'>,
  input: MessageActionAvailabilityInput = {},
): MessageActionAvailability {
  const hasText = message.text.trim().length > 0
  const isUser = message.role === 'user'
  const isStable = !message.isStreaming

  return {
    copy: hasText,
    retry: hasText && isUser && isStable && Boolean(input.canRetry),
    edit: hasText && isUser && isStable && Boolean(input.canEdit),
  }
}
```

Update the test import:

```tsx
import { MessageBubble, getMessageActionAvailability, getMessageCopyText } from './message-bubble'
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: PASS for all `message-bubble.test.tsx` tests.

### Task 2: Render Under-Bubble Action Toolbar

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx:4`
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx:190`

- [ ] **Step 1: Write failing render tests**

Add these tests:

```tsx
test('renders message actions under the bubble and right aligned', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={userMessage({ text: 'Can you retry this?' })}
      actionState={{ selected: false, canRetry: true, canEdit: true }}
    />,
  )

  assert.match(markup, /aria-label="Message actions"/)
  assert.match(markup, /Copy message/)
  assert.match(markup, /Retry message/)
  assert.match(markup, /Edit message/)
  assert.match(markup, /justify-end/)
  assert.ok(
    markup.indexOf('Can you retry this?') < markup.indexOf('aria-label="Message actions"'),
    'actions should render after the message content',
  )
})

test('omits retry and edit for assistant messages', () => {
  const markup = renderToStaticMarkup(
    <MessageBubble
      message={assistantMessage({ isStreaming: false, thoughts: [], toolCalls: [] })}
      actionState={{ selected: false, canRetry: true, canEdit: true }}
    />,
  )

  assert.match(markup, /Copy message/)
  assert.doesNotMatch(markup, /Retry message/)
  assert.doesNotMatch(markup, /Edit message/)
})
```

- [ ] **Step 2: Run and verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: FAIL because `MessageBubble` does not accept `actionState` and no toolbar exists.

- [ ] **Step 3: Add toolbar props and icons**

Change the import:

```tsx
import { Bot, Check, Copy, Pencil, RotateCcw, ChevronDown, ListChecks, UserRound } from 'lucide-react'
```

Add types:

```tsx
export type MessageBubbleActionState = {
  selected?: boolean
  canRetry?: boolean
  canEdit?: boolean
}

export type MessageBubbleActionHandlers = {
  onSelect?: (messageId: string) => void
  onDismiss?: () => void
  onRetry?: (message: ACPMessage) => void
  onEdit?: (message: ACPMessage) => void
}
```

Update the component signature:

```tsx
function MessageBubbleComponent({
  message,
  actionState = {},
  actionHandlers = {},
}: {
  message: ACPMessage
  actionState?: MessageBubbleActionState
  actionHandlers?: MessageBubbleActionHandlers
}) {
```

- [ ] **Step 4: Replace in-bubble copy with toolbar**

Remove the absolute copy block:

```tsx
<div className="absolute right-2 top-2">
  <CopyButton text={getMessageCopyText(message)} />
</div>
```

Render this after the message bubble `</div>` and before the containing column closes:

```tsx
<MessageActionToolbar
  message={message}
  availability={getMessageActionAvailability(message, actionState)}
  selected={Boolean(actionState.selected)}
  onRetry={actionHandlers.onRetry}
  onEdit={actionHandlers.onEdit}
/>
```

Add the toolbar component:

```tsx
function MessageActionToolbar({
  message,
  availability,
  selected,
  onRetry,
  onEdit,
}: {
  message: ACPMessage
  availability: MessageActionAvailability
  selected: boolean
  onRetry?: (message: ACPMessage) => void
  onEdit?: (message: ACPMessage) => void
}) {
  if (!availability.copy && !availability.retry && !availability.edit) {
    return null
  }

  return (
    <div
      aria-label="Message actions"
      className={cn(
        'flex w-full justify-end gap-1 pr-1 transition-opacity',
        selected
          ? 'opacity-100'
          : 'opacity-0 group-hover/bubble:opacity-100 group-focus-within/bubble:opacity-100',
      )}
    >
      {availability.copy ? <CopyButton text={getMessageCopyText(message)} /> : null}
      {availability.retry ? (
        <Button variant="ghost" size="icon" aria-label="Retry message" className="size-7 rounded" onClick={() => onRetry?.(message)}>
          <RotateCcw className="size-3.5" />
        </Button>
      ) : null}
      {availability.edit ? (
        <Button variant="ghost" size="icon" aria-label="Edit message" className="size-7 rounded" onClick={() => onEdit?.(message)}>
          <Pencil className="size-3.5" />
        </Button>
      ) : null}
    </div>
  )
}
```

Update `CopyButton` classes so it is usable in the row:

```tsx
className="size-7 shrink-0 rounded text-aurora-text-muted/70 hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
```

- [ ] **Step 5: Update memo comparator**

Extend `areMessageBubblePropsEqual` to include action state booleans. The callback props must be stable from parents or intentionally ignored only if they never affect rendered availability.

```tsx
previous.actionState?.selected === next.actionState?.selected &&
previous.actionState?.canRetry === next.actionState?.canRetry &&
previous.actionState?.canEdit === next.actionState?.canEdit
```

- [ ] **Step 6: Run and verify pass**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: PASS.

### Task 3: Add Copy Success And Failure Interaction Tests

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-bubble.test.tsx:1`
- Modify: `apps/gateway-admin/components/chat/message-bubble.tsx:25`

- [ ] **Step 1: Add client render helpers**

Add imports:

```tsx
import { act } from 'react'
import { createRoot } from 'react-dom/client'
```

Add a helper:

```tsx
async function renderClient(element: React.ReactElement) {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)

  await act(async () => {
    root.render(element)
  })

  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}
```

- [ ] **Step 2: Write failing copy interaction tests**

```tsx
test('copy action writes raw message text and shows copied state', async () => {
  const writes: string[] = []
  Object.assign(navigator, {
    clipboard: {
      writeText: async (value: string) => {
        writes.push(value)
      },
    },
  })

  const view = await renderClient(
    <MessageBubble
      message={assistantMessage({ text: '**raw** markdown', isStreaming: false, thoughts: [], toolCalls: [] })}
      actionState={{ selected: true }}
    />,
  )

  const button = view.container.querySelector('button[aria-label="Copy message"]') as HTMLButtonElement
  await act(async () => {
    button.click()
  })

  assert.deepEqual(writes, ['**raw** markdown'])
  assert.match(view.container.textContent ?? '', /Copied/)
  await view.unmount()
})

test('copy action exposes failure state when clipboard write is denied', async () => {
  Object.assign(navigator, {
    clipboard: {
      writeText: async () => {
        throw new DOMException('Denied', 'NotAllowedError')
      },
    },
  })

  const view = await renderClient(
    <MessageBubble
      message={assistantMessage({ text: 'copy me', isStreaming: false, thoughts: [], toolCalls: [] })}
      actionState={{ selected: true }}
    />,
  )

  const button = view.container.querySelector('button[aria-label="Copy message"]') as HTMLButtonElement
  await act(async () => {
    button.click()
  })

  assert.match(button.getAttribute('aria-label') ?? '', /Copy failed/)
  await view.unmount()
})
```

- [ ] **Step 3: Run and verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: FAIL because `CopyButton` has no text success or failure label yet.

- [ ] **Step 4: Implement copy result states**

Change `CopyButton`:

```tsx
type CopyState = 'idle' | 'copied' | 'failed'

function CopyButton({ text }: { text: string }) {
  const [copyState, setCopyState] = React.useState<CopyState>('idle')

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopyState('copied')
      window.setTimeout(() => setCopyState('idle'), 2000)
    } catch {
      setCopyState('failed')
      window.setTimeout(() => setCopyState('idle'), 2000)
    }
  }

  const label =
    copyState === 'copied'
      ? 'Copied message'
      : copyState === 'failed'
        ? 'Copy failed'
        : 'Copy message'

  return (
    <Button variant="ghost" size="icon" onClick={handleCopy} aria-label={label} className="size-7 shrink-0 rounded text-aurora-text-muted/70 hover:bg-aurora-hover-bg hover:text-aurora-text-primary">
      {copyState === 'copied' ? <Check className="size-3.5 text-aurora-success" /> : <Copy className="size-3.5" />}
      <span className="sr-only">{label}</span>
    </Button>
  )
}
```

- [ ] **Step 5: Run and verify pass**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-bubble.test.tsx
```

Expected: PASS.

### Task 4: Add Mobile Selection And Dismissal In MessageThread

**Files:**
- Modify: `apps/gateway-admin/components/chat/message-thread.tsx:72`
- Create: `apps/gateway-admin/components/chat/message-thread.test.tsx`

- [ ] **Step 1: Write failing thread interaction tests**

Create `apps/gateway-admin/components/chat/message-thread.test.tsx`:

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
    projectId: 'workspace',
    agentId: 'codex',
    provider: 'codex-acp',
    title: 'Run',
    createdAt: new Date('2026-05-05T00:00:00Z'),
    updatedAt: new Date('2026-05-05T00:00:00Z'),
    status: 'idle',
    providerSessionId: 'provider-run-1',
    cwd: '/home/jmagar/workspace/lab',
  }
}

function message(id: string, text: string): ACPMessage {
  return {
    id,
    runId: 'run-1',
    role: 'user',
    text,
    createdAt: new Date('2026-05-05T00:00:00Z'),
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
  }
}

async function renderClient(element: React.ReactElement) {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  await act(async () => root.render(element))
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

test('touch selection shows actions for one message and selecting another moves the row', async () => {
  const view = await renderClient(
    <MessageThread run={run()} messages={[message('m1', 'first'), message('m2', 'second')]} canRetryMessages canEditMessages />,
  )

  const bubbles = view.container.querySelectorAll('[data-message-id]')
  await act(async () => {
    bubbles[0]!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[data-message-id="m1"] [aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await act(async () => {
    bubbles[1]!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[data-message-id="m1"] [aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')
  assert.equal(view.container.querySelector('[data-message-id="m2"] [aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await view.unmount()
})

test('escape dismisses selected mobile message actions', async () => {
  const view = await renderClient(
    <MessageThread run={run()} messages={[message('m1', 'first')]} canRetryMessages canEditMessages />,
  )

  const bubble = view.container.querySelector('[data-message-id="m1"]')!
  await act(async () => {
    bubble.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await act(async () => {
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')

  await view.unmount()
})
```

- [ ] **Step 2: Run and verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-thread.test.tsx
```

Expected: FAIL because `MessageThread` lacks action props, selected state, and `data-message-id`.

- [ ] **Step 3: Add MessageThread action props and selection state**

Update props:

```tsx
interface MessageThreadProps {
  run: ACPRun | null
  messages: ACPMessage[]
  connectionState?: SessionEventConnectionState
  canRetryMessages?: boolean
  canEditMessages?: boolean
  onRetryMessage?: (message: ACPMessage) => void
  onEditMessage?: (message: ACPMessage) => void
}
```

Add state and Escape effect:

```tsx
const [selectedMessageId, setSelectedMessageId] = React.useState<string | null>(null)

React.useEffect(() => {
  if (!selectedMessageId) return
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === 'Escape') setSelectedMessageId(null)
  }
  document.addEventListener('keydown', onKeyDown)
  return () => document.removeEventListener('keydown', onKeyDown)
}, [selectedMessageId])
```

Pass props:

```tsx
<MessageBubble
  key={message.id}
  message={message}
  actionState={{
    selected: selectedMessageId === message.id,
    canRetry: canRetryMessages,
    canEdit: canEditMessages,
  }}
  actionHandlers={{
    onSelect: setSelectedMessageId,
    onDismiss: () => setSelectedMessageId(null),
    onRetry: onRetryMessage,
    onEdit: onEditMessage,
  }}
/>
```

Add this wrapper attribute in `MessageBubble` on the outer element:

```tsx
data-message-id={message.id}
onPointerDown={(event) => {
  if (event.pointerType === 'touch') {
    actionHandlers.onSelect?.(message.id)
  }
}}
```

Add `data-selected={selected ? 'true' : 'false'}` to the toolbar.

- [ ] **Step 4: Run and verify pass**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/message-thread.test.tsx
```

Expected: PASS.

### Task 5: Wire Retry And Edit From ChatShell

**Files:**
- Modify: `apps/gateway-admin/components/chat/chat-shell.tsx:47`
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
- Modify: `apps/gateway-admin/components/chat/chat-shell.test.tsx`

- [ ] **Step 1: Inspect ChatInput draft ownership**

Run:

```bash
sed -n '1,260p' apps/gateway-admin/components/chat/chat-input.tsx
```

Expected: identify whether the input is currently internally stateful or controlled by props.

- [ ] **Step 2: Write failing helper tests for retry/edit wiring**

If helper extraction is needed, add exported helpers to `chat-shell.tsx` and tests like:

```tsx
test('retry payload uses selected message text without inventing attachments', async () => {
  const sent: unknown[] = []
  await retryMessageText(
    {
      id: 'm1',
      runId: 'run-1',
      role: 'user',
      text: 'retry this',
      createdAt: new Date('2026-05-05T00:00:00Z'),
      isStreaming: false,
      thoughts: [],
      toolCalls: [],
      version: 1,
    },
    async (payload) => {
      sent.push(payload)
    },
  )

  assert.deepEqual(sent, [{ text: 'retry this', attachments: [] }])
})
```

- [ ] **Step 3: Run and verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/chat-shell.test.tsx
```

Expected: FAIL because `retryMessageText` or equivalent helper does not exist.

- [ ] **Step 4: Implement retry callback**

Add a helper:

```tsx
export async function retryMessageText(
  message: Pick<ACPMessage, 'text'>,
  send: (payload: ChatInputPayload) => Promise<void>,
) {
  await send({ text: message.text, attachments: [] })
}
```

In `ChatShell`, add:

```tsx
const handleRetryMessage = React.useCallback(
  async (message: ACPMessage) => {
    await retryMessageText(message, handleSendPrompt)
  },
  [handleSendPrompt],
)
```

- [ ] **Step 5: Implement edit only with a real draft path**

If `ChatInput` is internally stateful, add controlled draft props:

```tsx
draftText?: string
onDraftTextChange?: (value: string) => void
```

Then in `ChatShell`:

```tsx
const [draftText, setDraftText] = React.useState('')
const handleEditMessage = React.useCallback((message: ACPMessage) => {
  setDraftText(message.text)
}, [])
```

Pass to `ChatInput`:

```tsx
draftText={draftText}
onDraftTextChange={setDraftText}
```

Pass to `MessageThread`:

```tsx
canRetryMessages={providerReady}
canEditMessages
onRetryMessage={(message) => void handleRetryMessage(message)}
onEditMessage={handleEditMessage}
```

If the draft path cannot be added cleanly in this task, pass `canEditMessages={false}` and leave Edit hidden. Do not render a non-functional Edit button.

- [ ] **Step 6: Run focused tests**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/chat/chat-shell.test.tsx components/chat/message-thread.test.tsx components/chat/message-bubble.test.tsx
```

Expected: PASS.

### Task 6: Full Gateway Admin Verification

**Files:**
- No product files beyond Tasks 1-5.

- [ ] **Step 1: Run unit tests**

Run:

```bash
pnpm --dir apps/gateway-admin test:unit
```

Expected: PASS for all gateway-admin unit tests.

- [ ] **Step 2: Run lint**

Run:

```bash
pnpm --dir apps/gateway-admin lint
```

Expected: PASS with no ESLint errors.

- [ ] **Step 3: Run build**

Run:

```bash
pnpm --dir apps/gateway-admin build
```

Expected: PASS and Next.js build completes.

- [ ] **Step 4: Manual browser spot check**

Run:

```bash
pnpm --dir apps/gateway-admin dev
```

Expected:
- `/chat` renders.
- Desktop: hover a user message and see the right-aligned row under the bubble.
- Desktop: Tab into the row and actions remain visible.
- Mobile viewport: tap one bubble and see actions; tap another bubble and the selection moves; press Escape and actions hide.
- Copy writes raw text.
- Retry sends the user message text only.
- Edit populates the composer only if the draft wiring was implemented.

## Not In Scope

- Emoji reactions.
- Pin, share, delete, branch, or quote actions.
- Persisted edit history.
- Attachment replay for retry, because `ACPMessage` does not currently retain prompt attachments.
- Backend ACP API changes.
- Virtualized transcript changes.
- Analytics.

## Final Implementation Checklist

- [ ] `apps/gateway-admin/components/chat/message-bubble.tsx` renders actions under the bubble, not inside the bubble.
- [ ] Desktop hover and focus-within reveal actions without requiring click.
- [ ] Mobile touch selection reveals one action row at a time.
- [ ] Escape dismisses selected actions.
- [ ] Copy handles success and denied clipboard writes.
- [ ] Retry is hidden unless it can call the existing send path.
- [ ] Edit is hidden unless it can populate an actual composer draft.
- [ ] Focused component tests pass.
- [ ] `pnpm --dir apps/gateway-admin test:unit` passes.
- [ ] `pnpm --dir apps/gateway-admin lint` passes.
- [ ] `pnpm --dir apps/gateway-admin build` passes.
