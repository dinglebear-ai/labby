import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import type { ToolCallRecord } from '@/lib/types/metrics'

const call: ToolCallRecord = {
  id: 'call-1', ts: Date.now() - 60_000, tool: 'github::search', action: 'search_issues',
  agent_id: 'agent-1', agent_label: 'Reviewer', agent_kind: 'agent', ip: '127.0.0.1',
  surface: 'mcp', outcome: 'ok', error_kind: null, input_tokens: 10, output_tokens: 20, elapsed_ms: 120,
}
const router = { push() {}, replace() {}, prefetch() {}, back() {}, forward() {}, refresh() {} }

test('clicking a call card selects it, but its trace link keeps navigating instead', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  const { UsageCallCards } = await import('./usage-call-cards')
  const selected: ToolCallRecord[] = []
  const view = await renderClient(<AppRouterContext.Provider value={router as never}>
    <UsageCallCards calls={[call]} isLoading={false} error={undefined} onRetry={() => {}} onSelectCall={record => selected.push(record)} />
  </AppRouterContext.Provider>)
  try {
    const card = view.container.querySelector<HTMLElement>('[role="button"]')
    assert.ok(card, 'a selectable card is exposed as a button')
    assert.equal(card.getAttribute('aria-label'), 'Inspect call github::search.search_issues')
    assert.equal(card.getAttribute('tabindex'), '0')
    const link = card.querySelector('a')
    assert.ok(link)
    assert.match(link.getAttribute('href') ?? '', /\/traces\/?\?search=github$/)
    await act(async () => { link.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true }) as unknown as Event) })
    assert.equal(selected.length, 0, 'the trace link must not also select the card')
    await act(async () => { card.dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event) })
    assert.deepEqual(selected, [call])
    await act(async () => { card.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true }) as unknown as Event) })
    await act(async () => { card.dispatchEvent(new window.KeyboardEvent('keydown', { key: ' ', bubbles: true }) as unknown as Event) })
    assert.equal(selected.length, 3, 'Enter and Space select the focused card')
  } finally {
    await view.unmount()
    await window.happyDOM.close()
  }
})

test('call cards are plain articles when no selection handler is supplied', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  const { UsageCallCards } = await import('./usage-call-cards')
  const view = await renderClient(<AppRouterContext.Provider value={router as never}>
    <UsageCallCards calls={[call]} isLoading={false} error={undefined} onRetry={() => {}} />
  </AppRouterContext.Provider>)
  try {
    assert.equal(view.container.querySelector('[role="button"]'), null)
    assert.equal(view.container.querySelector('article')?.getAttribute('tabindex'), null)
  } finally {
    await view.unmount()
    await window.happyDOM.close()
  }
})
