import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { GatewayApiError } from '../../lib/api/gateway-client-core.ts'
import { __setBrowserSessionStateForTests, loadBrowserSession } from '../../lib/auth/session-store.ts'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { ToolBrowser, toolBrowserError } from './tool-browser.tsx'

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)) })
  }
  throw lastError
}

test('tool browser presents auth, availability, and request-id failures distinctly', () => {
  assert.deepEqual(
    toolBrowserError(new GatewayApiError('expired', 401, 'auth_failed'), 'fallback'),
    { message: 'Sign in to search tools.', status: 401, requestId: undefined },
  )
  assert.deepEqual(
    toolBrowserError(new GatewayApiError('denied', 403, 'forbidden'), 'fallback'),
    { message: 'Administrator access is required.', status: 403, requestId: undefined },
  )
  assert.deepEqual(
    toolBrowserError(new GatewayApiError('boom', 503, 'backend_unreachable', 'req-7'), 'fallback'),
    { message: 'Tools are temporarily unavailable.', status: 503, requestId: 'req-7' },
  )
})

test('tool browser renders hostile catalog text literally and clears it on session change', async () => {
  installTestDom()
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'admin', email: 'admin@example.com' },
    expiresAt: 100,
    csrfToken: 'csrf',
    isAdmin: true,
  })
  globalThis.fetch = async (input) => {
    const path = String(input)
    if (path === '/auth/session') return new Response(JSON.stringify({ authenticated: false }), { status: 200 })
    if (path.endsWith('/search')) return new Response(JSON.stringify({
      results: [{ path: 'github.hostile', id: 'github::hostile', kind: 'tool', namespace: 'github', name: 'hostile', description: '<script>alert(1)</script>', signature: '(input: string)', tags: [], score: 10 }],
      total: 1,
      truncated: false,
    }), { status: 200, headers: { 'content-type': 'application/json' } })
    return new Response(JSON.stringify({
      path: 'github.hostile', id: 'github::hostile', namespace: 'github', name: 'hostile',
      description: '<script>alert(1)</script>', helper: 'codemode.github.hostile', signature: '(input: string)', tags: [],
      typescript: '<script>alert(2)</script>',
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  const view = await renderClient(<ToolBrowser initialQuery="hostile" />)
  try {
    const form = view.container.querySelector('form')
    assert.ok(form)
    await act(async () => {
      form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))
    })
    await waitFor(() => assert.match(view.container.textContent ?? '', /github\.hostile/))
    const result = [...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('github.hostile'))
    assert.ok(result)
    await act(async () => { result.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /alert\(2\)/))
    assert.equal(view.container.querySelectorAll('script').length, 0)

    await act(async () => { await loadBrowserSession() })
    assert.doesNotMatch(view.container.textContent ?? '', /github\.hostile|alert\(2\)/)
  } finally {
    await view.unmount()
  }
})
