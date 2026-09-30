import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import {
  AuthAdminApiError,
  authAdminApi,
  type AllowedEmailEntry,
} from './auth-admin-client.ts'

const CSRF = 'csrf-abc'

function setAuthenticatedSession() {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'admin-user', email: 'admin@example.com' },
    expiresAt: 9999,
    csrfToken: CSRF,
    isAdmin: true,
  })
}

const sampleEntry: AllowedEmailEntry = {
  email: 'alice@example.com',
  added_by: 'admin@example.com',
  created_at: '2026-04-26T10:00:00Z',
  role: 'member',
}

// ---------------------------------------------------------------------------
// listAllowedEmails
// ---------------------------------------------------------------------------

test('authAdminApi.listAllowedEmails sends GET with credentials and csrf', async () => {
  setAuthenticatedSession()

  let requestUrl = ''
  let requestInit: RequestInit | undefined

  globalThis.fetch = async (input, init) => {
    requestUrl = String(input)
    requestInit = init
    return new Response(
      JSON.stringify({ entries: [sampleEntry] }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    )
  }

  const entries = await authAdminApi.listAllowedEmails()

  assert.equal(requestUrl, '/v1/auth/allowed-emails')
  assert.equal(requestInit?.credentials, 'include')
  const headers = new Headers(requestInit?.headers)
  assert.equal(headers.get('x-csrf-token'), CSRF)
  assert.deepEqual(entries, [sampleEntry])
})

test('authAdminApi.listAllowedEmails returns empty array when entries is empty', async () => {
  setAuthenticatedSession()

  globalThis.fetch = async () =>
    new Response(
      JSON.stringify({ entries: [] }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    )

  const entries = await authAdminApi.listAllowedEmails()
  assert.deepEqual(entries, [])
})

test('authAdminApi.listAllowedEmails surfaces backend errors as AuthAdminApiError', async () => {
  setAuthenticatedSession()

  globalThis.fetch = async () =>
    new Response(
      JSON.stringify({ kind: 'auth_failed', message: 'admin only' }),
      { status: 403, headers: { 'Content-Type': 'application/json' } },
    )

  await assert.rejects(
    authAdminApi.listAllowedEmails(),
    (error: unknown) =>
      error instanceof AuthAdminApiError &&
      error.status === 403 &&
      error.kind === 'auth_failed' &&
      error.message === 'admin only',
  )
})

// ---------------------------------------------------------------------------
// addAllowedEmail
// ---------------------------------------------------------------------------

test('authAdminApi.addAllowedEmail sends POST with email body and csrf', async () => {
  setAuthenticatedSession()

  let requestUrl = ''
  let requestInit: RequestInit | undefined

  globalThis.fetch = async (input, init) => {
    requestUrl = String(input)
    requestInit = init
    return new Response(
      JSON.stringify({ entry: sampleEntry }),
      { status: 201, headers: { 'Content-Type': 'application/json' } },
    )
  }

  const result = await authAdminApi.addAllowedEmail('alice@example.com', 'member')

  assert.equal(requestUrl, '/v1/auth/allowed-emails')
  assert.equal(requestInit?.method, 'POST')
  assert.equal(requestInit?.credentials, 'include')
  assert.deepEqual(JSON.parse(String(requestInit?.body)), { email: 'alice@example.com', role: 'member' })
  const headers = new Headers(requestInit?.headers)
  assert.equal(headers.get('x-csrf-token'), CSRF)
  assert.deepEqual(result, sampleEntry)
})

test('addAllowedEmail posts the role', async () => {
  setAuthenticatedSession()

  let sentBody = ''
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    sentBody = String(init?.body)
    return new Response(JSON.stringify({ entry: { email: 'eli@example.com', added_by: 'owner', created_at: '1', role: 'admin' } }), { status: 201 })
  }) as typeof globalThis.fetch
  const entry = await authAdminApi.addAllowedEmail('eli@example.com', 'admin')
  assert.deepEqual(JSON.parse(sentBody), { email: 'eli@example.com', role: 'admin' })
  assert.equal(entry.role, 'admin')
})

test('authAdminApi.addAllowedEmail surfaces 422 duplicate as AuthAdminApiError', async () => {
  setAuthenticatedSession()

  globalThis.fetch = async () =>
    new Response(
      JSON.stringify({ kind: 'already_exists', message: 'email already in allowlist' }),
      { status: 422, headers: { 'Content-Type': 'application/json' } },
    )

  await assert.rejects(
    authAdminApi.addAllowedEmail('alice@example.com', 'member'),
    (error: unknown) =>
      error instanceof AuthAdminApiError &&
      error.status === 422 &&
      error.kind === 'already_exists',
  )
})

// ---------------------------------------------------------------------------
// removeAllowedEmail
// ---------------------------------------------------------------------------

test('authAdminApi.removeAllowedEmail sends DELETE to url-encoded path', async () => {
  setAuthenticatedSession()

  let requestUrl = ''
  let requestInit: RequestInit | undefined

  globalThis.fetch = async (input, init) => {
    requestUrl = String(input)
    requestInit = init
    return new Response(null, { status: 204 })
  }

  await authAdminApi.removeAllowedEmail('alice+alias@example.com')

  assert.equal(requestUrl, '/v1/auth/allowed-emails/alice%2Balias%40example.com')
  assert.equal(requestInit?.method, 'DELETE')
  assert.equal(requestInit?.credentials, 'include')
  const headers = new Headers(requestInit?.headers)
  assert.equal(headers.get('x-csrf-token'), CSRF)
})

test('authAdminApi.removeAllowedEmail surfaces non-204 errors as AuthAdminApiError', async () => {
  setAuthenticatedSession()

  globalThis.fetch = async () =>
    new Response(
      JSON.stringify({ kind: 'not_found', message: 'email not in allowlist' }),
      { status: 404, headers: { 'Content-Type': 'application/json' } },
    )

  await assert.rejects(
    authAdminApi.removeAllowedEmail('alice@example.com'),
    (error: unknown) =>
      error instanceof AuthAdminApiError &&
      error.status === 404 &&
      error.kind === 'not_found',
  )
})

test('authAdminApi.removeAllowedEmail rethrows AbortError unchanged', async () => {
  setAuthenticatedSession()

  globalThis.fetch = async () => {
    throw new DOMException('request aborted', 'AbortError')
  }

  await assert.rejects(
    authAdminApi.removeAllowedEmail('alice@example.com'),
    (error: unknown) => error instanceof DOMException && error.name === 'AbortError',
  )
})
