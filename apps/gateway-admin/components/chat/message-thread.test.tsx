import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

import { MessageThread, shouldShowWorkingAssistantBubble } from './message-thread'
import { installChatTestDom, renderClient } from './test-utils'
import type { ACPMessage, ACPRun } from './types'

installChatTestDom()

const RUN_TIMESTAMP = new Date('2026-05-05T00:00:00Z')
const MESSAGE_TIMESTAMP = new Date('2026-05-05T00:00:01Z')

function run(status: ACPRun['status'] = 'running'): ACPRun {
  return {
    id: 'run-1',
    projectId: 'workspace',
    agentId: 'codex',
    provider: 'codex-acp',
    title: 'Run',
    createdAt: RUN_TIMESTAMP,
    updatedAt: RUN_TIMESTAMP,
    status,
    providerSessionId: 'provider-run-1',
    cwd: '/home/jmagar/workspace/lab',
  }
}

function message(overrides: Partial<ACPMessage> = {}): ACPMessage {
  return {
    id: 'message-1',
    runId: 'run-1',
    role: 'user',
    text: 'Please continue.',
    createdAt: MESSAGE_TIMESTAMP,
    isStreaming: false,
    thoughts: [],
    toolCalls: [],
    version: 1,
    ...overrides,
  }
}

test('shows working assistant bubble while run is running and no assistant stream exists', () => {
  assert.equal(shouldShowWorkingAssistantBubble(run('running'), [message()], 'open'), true)
})

test('shows working assistant bubble during initial connecting state for a running run', () => {
  assert.equal(shouldShowWorkingAssistantBubble(run('running'), [message()], 'connecting'), true)
})

test('does not show working assistant bubble when an assistant stream already exists', () => {
  assert.equal(
    shouldShowWorkingAssistantBubble(
      run('running'),
      [
        message(),
        message({
          id: 'assistant-stream',
          role: 'assistant',
          text: 'Working on it',
          isStreaming: true,
        }),
      ],
      'open',
    ),
    false,
  )
})

test('does not show working assistant bubble for waiting-for-permission', () => {
  assert.equal(shouldShowWorkingAssistantBubble(run('waiting_for_permission'), [message()], 'open'), false)
})

test('does not show working assistant bubble for idle runs or errored streams', () => {
  assert.equal(shouldShowWorkingAssistantBubble(run('idle'), [message()], 'open'), false)
  assert.equal(shouldShowWorkingAssistantBubble(run('running'), [message()], 'error'), false)
})

test('touch selection shows actions for one message and selecting another moves the row', async () => {
  const view = await renderClient(
    <MessageThread
      run={run()}
      messages={[
        message({ id: 'm1', text: 'first' }),
        message({ id: 'm2', text: 'second' }),
      ]}
      canRetryMessages
      canEditMessages
    />,
  )

  const bubbles = view.container.querySelectorAll('[data-message-id]')
  await act(async () => {
    bubbles[0]!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(
    view.container.querySelector('[data-message-id="m1"] [aria-label="Message actions"]')?.getAttribute('data-selected'),
    'true',
  )

  await act(async () => {
    bubbles[1]!.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(
    view.container.querySelector('[data-message-id="m1"] [aria-label="Message actions"]')?.getAttribute('data-selected'),
    'false',
  )
  assert.equal(
    view.container.querySelector('[data-message-id="m2"] [aria-label="Message actions"]')?.getAttribute('data-selected'),
    'true',
  )

  await view.unmount()
})

test('escape dismisses selected mobile message actions', async () => {
  const view = await renderClient(
    <MessageThread run={run()} messages={[message({ id: 'm1', text: 'first' })]} canRetryMessages canEditMessages />,
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

test('outside pointer dismisses selected mobile message actions', async () => {
  const view = await renderClient(
    <MessageThread run={run()} messages={[message({ id: 'm1', text: 'first' })]} canRetryMessages canEditMessages />,
  )

  const bubble = view.container.querySelector('[data-message-id="m1"]')!
  await act(async () => {
    bubble.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'true')

  await act(async () => {
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType: 'touch' }))
  })
  assert.equal(view.container.querySelector('[aria-label="Message actions"]')?.getAttribute('data-selected'), 'false')

  await view.unmount()
})
