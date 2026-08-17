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
