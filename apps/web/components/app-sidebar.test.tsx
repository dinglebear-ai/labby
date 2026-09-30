import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

import {
  BrowserSignOutButton,
  primarySidebarNavigation,
  secondarySidebarNavigation,
} from './app-sidebar'
import {
  __setBrowserSessionStateForTests,
  getBrowserSessionState,
} from '../lib/auth/session-store.ts'
import { installTestDom, renderClient } from '../lib/testing/dom-test-utils.tsx'

test('app sidebar navigation excludes design system route', () => {
  const labels = [
    ...primarySidebarNavigation.map((item) => item.title),
    ...secondarySidebarNavigation.map((item) => item.title),
  ]

  assert.equal(labels.includes('Gateway'), true)
  assert.equal(labels.includes('Servers'), false)
  assert.equal(labels.includes('Snippets'), true)
  assert.equal(labels.includes('Design System'), false)

  // Removed surfaces (no backing service): assert each is gone, not just Chat.
  for (const removed of ['Nodes', 'Marketplace', 'Chat', 'Setup', 'Activity', 'Logs']) {
    assert.equal(labels.includes(removed), false, `expected "${removed}" to be removed from nav`)
  }
})

test('snippets is a high-level primary navigation item', () => {
  const snippets = primarySidebarNavigation.find((item) => item.title === 'Snippets')

  assert.ok(snippets)
  assert.equal(snippets.url, '/snippets')
  assert.equal(primarySidebarNavigation.indexOf(snippets), 2)
})

test('BrowserSignOutButton signs out locally and surfaces a failed server revocation separately', async () => {
  installTestDom()
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-123',
  })
  globalThis.fetch = (async () => new Response('boom', { status: 500 })) as typeof fetch

  const view = await renderClient(React.createElement(BrowserSignOutButton))
  try {
    const button = view.container.querySelector('button')
    assert.ok(button)
    await act(async () => {
      button.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    // Local authority state is cleared even though the server did not confirm
    // the revocation; the failure is reported so the operator knows the server
    // session may linger.
    const alert = view.container.querySelector('[role="alert"]')
    assert.ok(alert)
    assert.match(alert.textContent ?? '', /did not confirm sign-out \(HTTP 500\)/)
    assert.match(alert.textContent ?? '', /server session may remain active/)
    assert.doesNotMatch(alert.textContent ?? '', /still active\./)
    assert.deepEqual(getBrowserSessionState(), { status: 'unauthenticated' })
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
