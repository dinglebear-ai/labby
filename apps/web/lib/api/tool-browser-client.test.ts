import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { describeCodeModeTool, searchCodeModeTools } from './tool-browser-client.ts'

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
  let releaseBody: (() => void) | undefined
  const encoded = new TextEncoder().encode(JSON.stringify({ results: [], total: 0, truncated: false }))
  const response = new Response(new ReadableStream({
    start(controller) {
      releaseBody = () => {
        controller.enqueue(encoded)
        controller.close()
      }
    },
  }), { status: 200 })
  globalThis.fetch = async () => response

  const pending = searchCodeModeTools('gateway')
  await Promise.resolve()
  setAuthenticatedSession('next-admin')
  releaseBody?.()

  await assert.rejects(
    pending,
    (error: unknown) => error instanceof DOMException && error.name === 'AbortError',
  )
})

test('tool search cancels a chunked response as soon as it exceeds the safety limit', async () => {
  setAuthenticatedSession()
  let cancelled = false
  globalThis.fetch = async () => new Response(new ReadableStream({
    start(controller) {
      controller.enqueue(new Uint8Array(256 * 1024))
      controller.enqueue(new Uint8Array(1))
    },
    cancel() { cancelled = true },
  }), { status: 200, headers: { 'x-request-id': 'req-stream-large' } })

  await assert.rejects(
    searchCodeModeTools('gateway'),
    (error: unknown) => error instanceof Error &&
      'code' in error && error.code === 'response_too_large' &&
      'requestId' in error && error.requestId === 'req-stream-large',
  )
  assert.equal(cancelled, true)
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

test('tool search rejects malformed successful JSON with its request ID', async () => {
  setAuthenticatedSession()
  globalThis.fetch = async () => new Response('{broken', {
    status: 200,
    headers: { 'x-request-id': 'req-invalid' },
  })

  await assert.rejects(
    searchCodeModeTools('gateway'),
    (error: unknown) => error instanceof Error &&
      'code' in error && error.code === 'invalid_response' &&
      'requestId' in error && error.requestId === 'req-invalid',
  )
})

test('tool search rejects schema-invalid successful JSON with its request ID', async () => {
  setAuthenticatedSession()
  globalThis.fetch = async () => new Response(JSON.stringify({ results: null, total: 0, truncated: false }), {
    status: 200,
    headers: { 'x-request-id': 'req-invalid-shape' },
  })

  await assert.rejects(
    searchCodeModeTools('gateway'),
    (error: unknown) => error instanceof Error &&
      'code' in error && error.code === 'invalid_response' &&
      'requestId' in error && error.requestId === 'req-invalid-shape',
  )
})

test('tool search rejects malformed safety metadata', async () => {
  setAuthenticatedSession()
  globalThis.fetch = async () => new Response(JSON.stringify({
    results: [{
      path: 'alpha.ping', id: 'alpha::ping', kind: 'tool', namespace: 'alpha', name: 'ping',
      description: 'Ping', signature: '()', tags: [], score: 1, safety: { destructive: 'false' },
    }],
    total: 1,
    truncated: false,
  }), { status: 200 })

  await assert.rejects(
    searchCodeModeTools('ping'),
    (error: unknown) => error instanceof Error && 'code' in error && error.code === 'invalid_response',
  )
})

test('tool search normalizes unexpected error JSON and preserves correlation', async () => {
  setAuthenticatedSession()
  globalThis.fetch = async () => new Response('null', {
    status: 500,
    headers: { 'x-request-id': 'req-error-shape' },
  })

  await assert.rejects(
    searchCodeModeTools('ping'),
    (error: unknown) => error instanceof Error &&
      'status' in error && error.status === 500 &&
      'requestId' in error && error.requestId === 'req-error-shape' &&
      error.message === 'Tools unavailable',
  )
})

test('tool search retries unexpected 401 JSON after refreshing the session', async () => {
  setAuthenticatedSession()
  let call = 0
  globalThis.fetch = async () => {
    call += 1
    if (call === 1) return new Response('null', { status: 401 })
    if (call === 2) return new Response(JSON.stringify({
      authenticated: true,
      user: { sub: 'admin-user', email: 'admin-user@example.com' },
      expires_at: 10000,
      csrf_token: 'csrf-refreshed',
      is_admin: true,
    }), { status: 200 })
    return new Response(JSON.stringify({ results: [], total: 0, truncated: false }), { status: 200 })
  }

  await searchCodeModeTools('ping')
  assert.equal(call, 3)
})

test('tool describe uses its endpoint, CSRF token, and response bound', async () => {
  setAuthenticatedSession()
  let requestUrl = ''
  let requestInit: RequestInit | undefined
  globalThis.fetch = async (input, init) => {
    requestUrl = String(input); requestInit = init
    return new Response(JSON.stringify({
      path: 'alpha.ping', id: 'alpha::ping', namespace: 'alpha', name: 'ping',
      description: 'Ping', helper: 'codemode.alpha.ping', signature: '()', tags: [],
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  const result = await describeCodeModeTool('alpha::ping')
  assert.equal(result.id, 'alpha::ping')
  assert.equal(requestUrl, '/v1/gateway/codemode/tools/describe')
  assert.deepEqual(JSON.parse(String(requestInit?.body)), { target: 'alpha::ping' })
  assert.equal(new Headers(requestInit?.headers).get('x-csrf-token'), CSRF)
})

test('tool describe rejects oversized responses with request correlation', async () => {
  setAuthenticatedSession()
  globalThis.fetch = async () => new Response('x'.repeat(128 * 1024 + 1), {
    status: 200,
    headers: { 'x-request-id': 'req-describe-large' },
  })
  await assert.rejects(
    describeCodeModeTool('alpha::ping'),
    (error: unknown) => error instanceof Error &&
      'code' in error && error.code === 'response_too_large' &&
      'requestId' in error && error.requestId === 'req-describe-large',
  )
})

test('tool describe rejects malformed safety metadata', async () => {
  setAuthenticatedSession()
  globalThis.fetch = async () => new Response(JSON.stringify({
    path: 'alpha.ping', id: 'alpha::ping', namespace: 'alpha', name: 'ping',
    description: 'Ping', helper: 'codemode.alpha.ping', signature: '()', tags: [],
    safety: { read_only: 1 },
  }), { status: 200 })
  await assert.rejects(
    describeCodeModeTool('alpha::ping'),
    (error: unknown) => error instanceof Error && 'code' in error && error.code === 'invalid_response',
  )
})
