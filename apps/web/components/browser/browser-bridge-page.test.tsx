import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { Window } from 'happy-dom'

import { browserApi } from '@/lib/api/browser-client'
import type { BrowserIdentity } from '@/lib/types/browser'

function installDom() {
  const window = new Window()
  for (const key of ['document', 'navigator', 'HTMLElement', 'HTMLButtonElement', 'Node', 'Event', 'MouseEvent', 'PointerEvent', 'KeyboardEvent', 'CustomEvent', 'DOMException', 'MutationObserver'] as const) {
    Object.defineProperty(globalThis, key, { configurable: true, value: window[key] })
  }
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'getComputedStyle', { configurable: true, value: window.getComputedStyle.bind(window) })
  Object.defineProperty(globalThis, 'requestAnimationFrame', { configurable: true, value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 0) })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', { configurable: true, value: window.clearTimeout.bind(window) })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
  return window
}

const identity: BrowserIdentity = {
  id: 'browser-1', display_name: 'Operator Chrome', extension_id: 'abcdefghijklmnopabcdefghijklmnop',
  paired_at: 1, last_seen_at: null, revoked_at: null, connected: true,
}

function button(label: string, root: ParentNode = document.body) {
  const found = [...root.querySelectorAll('button')].find((entry) => entry.textContent?.trim() === label)
  assert.ok(found, `missing ${label} button`)
  return found
}

test('failed revocation preserves confirmation for retry; successful retry closes it', async () => {
  const window = installDom()
  const { BrowserBridgePage } = await import('./browser-bridge-page')
  const { createRoot } = await import('react-dom/client')
  const original = { ...browserApi }
  let calls = 0
  let rejectFirst!: (error: Error) => void
  const firstRevoke = new Promise<BrowserIdentity>((_resolve, reject) => { rejectFirst = reject })
  browserApi.list = async () => [identity]
  browserApi.pairings = async () => []
  browserApi.sessions = async () => ({ sessions: [], next_cursor: null, detail_warnings: [] })
  browserApi.revoke = async () => {
    calls += 1
    if (calls === 1) return firstRevoke
    return { ...identity, revoked_at: 2 }
  }
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  try {
    await act(async () => root.render(<BrowserBridgePage />))
    assert.match(container.textContent ?? '', /1 connected/)
    assert.equal(container.querySelectorAll('[data-console-hero-stat-icon]').length, 0)
    assert.ok(container.querySelector('[data-browser-connection-dot]'))
    await act(async () => button('Revoke').click())
    assert.ok(document.querySelector('[data-slot="alert-dialog-content"]'))
    await act(async () => {
      const confirm = button('Revoke browser')
      confirm.click()
      confirm.click()
    })
    assert.equal(calls, 1, 'overlapping confirmations must send only one revocation')
    await act(async () => rejectFirst(new Error('Revocation denied; sign in again.')))
    assert.equal(calls, 1)
    assert.ok(document.querySelector('[data-slot="alert-dialog-content"]'), 'failed revocation must leave its confirmation open')
    await act(async () => button('Revoke browser').click())
    assert.equal(calls, 2)
    assert.equal(document.querySelector('[data-slot="alert-dialog-content"]'), null)
  } finally {
    await act(async () => root.unmount())
    container.remove()
    Object.assign(browserApi, original)
    await window.happyDOM.close()
  }
})



test('partial browser session detail failures stay visible without blanking the bridge', async () => {
  const window = installDom()
  const { BrowserBridgePage } = await import('./browser-bridge-page')
  const { createRoot } = await import('react-dom/client')
  const original = { ...browserApi }
  browserApi.list = async () => [identity]
  browserApi.pairings = async () => []
  browserApi.sessions = async () => ({
    sessions: [],
    next_cursor: null,
    detail_warnings: ['Session stale-session details unavailable: session disappeared'],
  })
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  try {
    await act(async () => root.render(<BrowserBridgePage />))
    assert.ok(document.body.textContent?.includes('Browser bridge is partially degraded'))
    assert.ok(document.body.textContent?.includes('stale-session'))
    assert.ok(document.body.textContent?.includes('Operator Chrome'))
    assert.equal(document.body.textContent?.includes('Browser bridge unavailable'), false)
  } finally {
    await act(async () => root.unmount())
    container.remove()
    Object.assign(browserApi, original)
    await window.happyDOM.close()
  }
})

test('session pagination exposes older bounded pages instead of hiding them', async () => {
  const window = installDom()
  const { BrowserBridgePage } = await import('./browser-bridge-page')
  const { createRoot } = await import('react-dom/client')
  const original = { ...browserApi }
  const cursors: Array<string | undefined> = []
  browserApi.list = async () => []
  browserApi.pairings = async () => []
  browserApi.sessions = async (_signal, cursor) => {
    cursors.push(cursor)
    return cursor === 'older-page'
      ? { sessions: [], next_cursor: null, detail_warnings: [] }
      : { sessions: [], next_cursor: 'older-page', detail_warnings: [] }
  }
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  try {
    await act(async () => root.render(<BrowserBridgePage />))
    assert.deepEqual(cursors, [undefined])
    assert.match(container.textContent ?? '', /0 connected/)
    assert.ok(document.body.textContent?.includes('Session page 1'))
    await act(async () => button('Next pages').click())
    assert.deepEqual(cursors, [undefined, 'older-page'])
    assert.ok(document.body.textContent?.includes('Session page 2'))
    assert.ok(document.body.textContent?.includes('No active WebMCP pages on this session page'))
    await act(async () => button('Previous pages').click())
    assert.deepEqual(cursors, [undefined, 'older-page', undefined])
    assert.ok(document.body.textContent?.includes('Session page 1'))
  } finally {
    await act(async () => root.unmount())
    container.remove()
    Object.assign(browserApi, original)
    await window.happyDOM.close()
  }
})
