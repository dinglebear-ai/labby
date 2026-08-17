import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { searchCodeModeTools } from './tool-browser-client.ts'

const CSRF = 'csrf-tool-browser'

function setAuthenticatedSession(subject = 'admin-user') {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: subject, email: `${subject}@example.com` },
    expiresAt: 9999,
    csrfToken: CSRF,
    isAdmin: true,
  })
}

test('tool search sends its cookie credentials and CSRF token', async () => {
  setAuthenticatedSession()
  let requestUrl = ''
  let requestInit: RequestInit | undefined

  globalThis.fetch = async (input, init) => {
    requestUrl = String(input)
    requestInit = init
    return new Response(JSON.stringify({ results: [], total: 0, truncated: false }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  }

  await searchCodeModeTools('logs')

  assert.equal(requestUrl, '/v1/gateway/codemode/tools/search')
  assert.equal(requestInit?.method, 'POST')
  assert.equal(requestInit?.credentials, 'include')
  assert.equal(requestInit?.cache, 'no-store')
  assert.deepEqual(JSON.parse(String(requestInit?.body)), { query: 'logs', limit: 50 })
  assert.equal(new Headers(requestInit?.headers).get('x-csrf-token'), CSRF)
})

test('tool search discards a response from an earlier authenticated session', async () => {
  setAuthenticatedSession('first-admin')
  let resolveResponse: ((response: Response) => void) | undefined
  globalThis.fetch = () => new Promise<Response>((resolve) => { resolveResponse = resolve })

  const pending = searchCodeModeTools('gateway')
  setAuthenticatedSession('second-admin')
  resolveResponse?.(new Response(JSON.stringify({ results: [], total: 0, truncated: false }), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  }))

  await assert.rejects(
    pending,
    (error: unknown) => error instanceof DOMException && error.name === 'AbortError',
  )
})

test('tool search rechecks the session after a delayed response body', async () => {
  setAuthenticatedSession('body-admin')
  let resolveBody: ((value: ArrayBuffer) => void) | undefined
  const response = new Response(null, { status: 200 })
  response.arrayBuffer = () => new Promise((resolve) => { resolveBody = resolve })
  globalThis.fetch = async () => response

  const pending = searchCodeModeTools('gateway')
  await Promise.resolve()
  setAuthenticatedSession('next-admin')
  const encoded = new TextEncoder().encode(JSON.stringify({ results: [], total: 0, truncated: false }))
  resolveBody?.(encoded.buffer.slice(encoded.byteOffset, encoded.byteOffset + encoded.byteLength) as ArrayBuffer)

  await assert.rejects(
    pending,
    (error: unknown) => error instanceof DOMException && error.name === 'AbortError',
  )
})

test('tool search refreshes a stale CSRF session and retries once', async () => {
  setAuthenticatedSession()
  const csrfHeaders: Array<string | null> = []
  let call = 0
  globalThis.fetch = async (_input, init) => {
    call += 1
    if (call === 1) {
      csrfHeaders.push(new Headers(init?.headers).get('x-csrf-token'))
      return new Response(JSON.stringify({ kind: 'validation_failed', message: 'invalid csrf token' }), {
        status: 422,
        headers: { 'Content-Type': 'application/json' },
      })
    }
    if (call === 2) {
      return new Response(JSON.stringify({
        authenticated: true,
        user: { sub: 'admin-user', email: 'admin-user@example.com' },
        expires_at: 10000,
        csrf_token: 'csrf-refreshed',
        is_admin: true,
      }), { status: 200, headers: { 'Content-Type': 'application/json' } })
    }
    csrfHeaders.push(new Headers(init?.headers).get('x-csrf-token'))
    return new Response(JSON.stringify({ results: [], total: 0, truncated: false }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  }

  await searchCodeModeTools('gateway')
  assert.deepEqual(csrfHeaders, [CSRF, 'csrf-refreshed'])
  assert.equal(call, 3)
})

test('tool search rejects an oversized response before JSON parsing', async () => {
  setAuthenticatedSession()
  globalThis.fetch = async () => new Response('x'.repeat(256 * 1024 + 1), {
    status: 200,
    headers: { 'x-request-id': 'req-large' },
  })

  await assert.rejects(
    searchCodeModeTools('gateway'),
    (error: unknown) => error instanceof Error &&
      'code' in error && error.code === 'response_too_large' &&
      'requestId' in error && error.requestId === 'req-large',
  )
})
