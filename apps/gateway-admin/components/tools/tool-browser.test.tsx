import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { SWRConfig } from 'swr'

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

test('tool browser submits a blank query to browse the live catalog', async () => {
  installTestDom()
  __setBrowserSessionStateForTests({
    status: 'authenticated', user: { sub: 'admin', email: 'admin@example.com' },
    expiresAt: 100, csrfToken: 'csrf', isAdmin: true,
  })
  let requestBody: unknown
  globalThis.fetch = async (input, init) => {
    const path = String(input)
    if (path.endsWith('/search')) {
      requestBody = JSON.parse(String(init?.body))
      return new Response(JSON.stringify({
        results: [{ path: 'alpha.ping', id: 'alpha::ping', kind: 'tool', namespace: 'alpha', name: 'ping', description: 'Ping', signature: '()', tags: [], score: 0 }],
        total: 1, truncated: false,
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    return new Response('{}', { status: 404 })
  }

  const view = await renderClient(<ToolBrowser />)
  try {
    const form = view.container.querySelector('form'); assert.ok(form)
    await act(async () => { form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /alpha\.ping/))
    assert.deepEqual(requestBody, { query: '', limit: 50 })
  } finally { await view.unmount() }
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

test('tool browser clears stale search results and retries the failed detail operation', async () => {
  installTestDom()
  __setBrowserSessionStateForTests({
    status: 'authenticated', user: { sub: 'admin', email: 'admin@example.com' },
    expiresAt: 100, csrfToken: 'csrf', isAdmin: true,
  })
  let describeAttempts = 0
  let searchAttempts = 0
  globalThis.fetch = async (input) => {
    const path = String(input)
    if (path.endsWith('/search')) {
      searchAttempts += 1
      if (searchAttempts > 1) return new Response(JSON.stringify({ kind: 'internal_error', message: 'failed' }), { status: 500, headers: { 'content-type': 'application/json' } })
      return new Response(JSON.stringify({
        results: [{ path: 'alpha.ping', id: 'alpha::ping', kind: 'tool', namespace: 'alpha', name: 'ping', description: 'Ping', signature: '()', tags: [], score: 10 }],
        total: 1, truncated: false,
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    describeAttempts += 1
    if (describeAttempts === 1) return new Response(JSON.stringify({ kind: 'internal_error', message: 'failed' }), { status: 500, headers: { 'content-type': 'application/json' } })
    return new Response(JSON.stringify({ path: 'alpha.ping', id: 'alpha::ping', namespace: 'alpha', name: 'ping', description: 'Ping', helper: 'codemode.alpha.ping', signature: '()', tags: [] }), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  const view = await renderClient(<ToolBrowser initialQuery="ping" />)
  try {
    const form = view.container.querySelector('form'); assert.ok(form)
    await act(async () => { form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /alpha\.ping/))
    const result = [...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('alpha.ping')); assert.ok(result)
    await act(async () => { result.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /temporarily unavailable/))
    const retry = [...view.container.querySelectorAll('button')].find((button) => button.textContent === 'Retry'); assert.ok(retry)
    await act(async () => { retry.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /codemode\.alpha\.ping/))
    assert.equal(describeAttempts, 2)

    await act(async () => { form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /temporarily unavailable/))
    assert.doesNotMatch(view.container.textContent ?? '', /alpha\.ping/)
    assert.doesNotMatch(view.container.textContent ?? '', /No matching tools/)
  } finally { await view.unmount() }
})

test('a completed empty browse names Code Mode being disabled, instead of looking unchanged', async () => {
  // Previously a zero-result "Browse all" rendered the exact same placeholder
  // text as before any search ran — an operator clicking it with Code Mode
  // off saw no feedback that anything had happened at all.
  installTestDom()
  __setBrowserSessionStateForTests({
    status: 'authenticated', user: { sub: 'admin', email: 'admin@example.com' },
    expiresAt: 100, csrfToken: 'csrf', isAdmin: true,
  })
  globalThis.fetch = async (input, init) => {
    const path = String(input)
    if (path.endsWith('/search')) {
      return new Response(JSON.stringify({ results: [], total: 0, truncated: false }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    if (path.endsWith('/gateway')) {
      const body = JSON.parse(String(init?.body)) as { action?: string }
      if (body.action === 'gateway.code_mode.get') {
        return new Response(JSON.stringify({
          enabled: false, timeout_ms: 5000, max_tool_calls: 10, max_response_bytes: 1024, max_response_tokens: 1024,
        }), { status: 200, headers: { 'content-type': 'application/json' } })
      }
    }
    return new Response('{}', { status: 404 })
  }

  const view = await renderClient(
    // `ToolBrowser` also reads the code-mode config via SWR, whose cache is a
    // module-level singleton — an isolated provider keeps this test's mock
    // from reading whatever an earlier test in this file already cached
    // under the same key.
    <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
      <ToolBrowser />
    </SWRConfig>,
  )
  try {
    assert.match(view.container.textContent ?? '', /Search, or browse the live catalog without a query/)
    const form = view.container.querySelector('form'); assert.ok(form)
    await act(async () => { form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })) })
    await waitFor(() => assert.match(view.container.textContent ?? '', /Code Mode is disabled/))
    assert.doesNotMatch(view.container.textContent ?? '', /Search, or browse the live catalog without a query/)
  } finally { await view.unmount() }
})
