import test from 'node:test'
import assert from 'node:assert/strict'

import { OwnerBootstrapError, bootstrapOwner, describeOwnerBootstrapError } from './owner-bootstrap.ts'
import { __setBrowserSessionStateForTests, getBrowserSessionState, getSessionAuthority } from './session-store.ts'

type FetchMock = typeof globalThis.fetch

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

function pendingOwnerSession() {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'owner-subject', email: 'owner@example.com' },
    expiresAt: 124,
    csrfToken: 'csrf-owner',
    authorityState: 'transport',
  })
}

const readySession = {
  authenticated: true,
  authority_state: 'ready',
  authority: { principal_id: 'bootstrap-owner', organization_id: 'bootstrap-local', authority_generation: 3 },
  user: { sub: 'owner-subject', email: 'owner@example.com' },
  owner: { kind: 'personal', id: 'bootstrap-owner' },
  organization_id: 'bootstrap-local',
  teams: [],
  projects: [],
  capabilities: ['platform.manage'],
  authority_generation: 3,
  expires_at: 125,
  csrf_token: 'csrf-owner-2',
}

test('bootstrapOwner posts trimmed names with the session CSRF token, then reloads a ready session', async () => {
  pendingOwnerSession()
  const calls: Array<{ url: string; init?: RequestInit }> = []
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    if (String(url) === '/v1/access/bootstrap-owner') {
      return new Response(JSON.stringify({ status: 'already_applied' }), { status: 200 })
    }
    return new Response(JSON.stringify(readySession), { status: 200 })
  }) as FetchMock

  const outcome = await bootstrapOwner({ organizationName: '  Unraid ', projectName: 'Team-Skills' })

  assert.equal(outcome, 'already_applied')
  assert.equal(calls[0].url, '/v1/access/bootstrap-owner')
  assert.equal(calls[0].init?.method, 'POST')
  assert.equal(calls[0].init?.credentials, 'include')
  assert.equal(new Headers(calls[0].init?.headers).get('x-csrf-token'), 'csrf-owner')
  assert.deepEqual(JSON.parse(String(calls[0].init?.body)), { organization_name: 'Unraid', project_name: 'Team-Skills' })
  assert.equal(calls[1].url, '/auth/session')
  const state = getBrowserSessionState()
  assert.equal(state.status === 'authenticated' ? state.authorityState : undefined, 'ready')
  assert.equal(getSessionAuthority()?.principalId, 'bootstrap-owner')
})

const pendingSessionBody = {
  authenticated: true,
  authority_state: 'transport',
  authority: null,
  remediation: 'Complete owner bootstrap to enable multi-user authority.',
  user: { sub: 'owner-subject', email: 'owner@example.com' },
  owner: null,
  organization_id: null,
  teams: [],
  projects: [],
  capabilities: [],
  authority_generation: null,
  expires_at: 125,
  csrf_token: 'csrf-owner-2',
}

function conflictThenSession(sessionBody: unknown) {
  const calls: string[] = []
  globalThis.fetch = (async (url: string | URL | Request) => {
    calls.push(String(url))
    if (String(url) === '/v1/access/bootstrap-owner') {
      return new Response(
        JSON.stringify({ kind: 'conflict', message: 'access owner bootstrap conflicts with existing state' }),
        { status: 409 },
      )
    }
    return new Response(JSON.stringify(sessionBody), { status: 200 })
  }) as FetchMock
  return calls
}

test('bootstrapOwner surfaces a conflict as an actionable error without retrying', async () => {
  pendingOwnerSession()
  const calls = conflictThenSession(pendingSessionBody)

  await assert.rejects(
    bootstrapOwner({ organizationName: 'Local', projectName: 'Default' }),
    (error: unknown) => error instanceof OwnerBootstrapError && error.status === 409 && error.kind === 'conflict',
  )
  assert.deepEqual(calls, ['/v1/access/bootstrap-owner', '/auth/session'])
  const state = getBrowserSessionState()
  assert.equal(state.status === 'authenticated' ? state.authorityState : undefined, 'transport')
})

test('a conflict for an owner linked through another identity reloads into a ready session', async () => {
  pendingOwnerSession()
  conflictThenSession(readySession)

  await assert.rejects(bootstrapOwner({ organizationName: 'Unraid', projectName: 'Team-Skills' }), OwnerBootstrapError)
  const state = getBrowserSessionState()
  assert.equal(state.status === 'authenticated' ? state.authorityState : undefined, 'ready')
  assert.equal(getSessionAuthority()?.principalId, 'bootstrap-owner')
})

test('bootstrapOwner refuses to post without a CSRF token', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  let called = false
  globalThis.fetch = (async () => {
    called = true
    return new Response('{}', { status: 200 })
  }) as FetchMock

  await assert.rejects(bootstrapOwner({ organizationName: 'Local', projectName: 'Default' }), OwnerBootstrapError)
  assert.equal(called, false)
})

test('bootstrapOwner rejects an unexpected success body', async () => {
  pendingOwnerSession()
  globalThis.fetch = (async () => new Response(JSON.stringify({ status: 'mystery' }), { status: 200 })) as FetchMock

  await assert.rejects(bootstrapOwner({ organizationName: 'Local', projectName: 'Default' }), /unexpected response/)
})

test('describeOwnerBootstrapError maps server kinds to recovery guidance', () => {
  assert.match(describeOwnerBootstrapError(new OwnerBootstrapError(409, 'x', 'conflict')), /names from the original setup/)
  assert.match(describeOwnerBootstrapError(new OwnerBootstrapError(403, 'x', 'forbidden')), /configured Labby admin account/)
  assert.match(describeOwnerBootstrapError(new OwnerBootstrapError(422, 'x', 'validation_failed')), /128 characters/)
  assert.equal(describeOwnerBootstrapError(new OwnerBootstrapError(500, 'store unavailable', 'internal_error')), 'store unavailable')
  assert.match(describeOwnerBootstrapError(new TypeError('fetch failed')), /could not reach the server/)
})

test('bootstrapOwner retries an edge timeout because owner bootstrap is idempotent', async () => {
  pendingOwnerSession()
  let posts = 0
  globalThis.fetch = (async (url: string | URL | Request) => {
    if (String(url) === '/v1/access/bootstrap-owner') {
      posts += 1
      return posts === 1
        ? new Response('<html>522</html>', { status: 522 })
        : new Response(JSON.stringify({ status: 'already_applied' }), { status: 200 })
    }
    return new Response(JSON.stringify(readySession), { status: 200 })
  }) as FetchMock

  const outcome = await bootstrapOwner({ organizationName: 'Unraid', projectName: 'Team-Skills' }, { retryDelaysMs: [0] })
  assert.equal(outcome, 'already_applied')
  assert.equal(posts, 2)
})

test('bootstrapOwner stops after its retry budget and explains an edge failure', async () => {
  pendingOwnerSession()
  let posts = 0
  globalThis.fetch = (async () => {
    posts += 1
    return new Response('<html>522</html>', { status: 522 })
  }) as FetchMock

  let caught: unknown
  await bootstrapOwner({ organizationName: 'Unraid', projectName: 'Team-Skills' }, { retryDelaysMs: [0, 0] }).catch((error: unknown) => {
    caught = error
  })
  assert.ok(caught instanceof OwnerBootstrapError && caught.status === 522)
  assert.equal(posts, 3)
  assert.match(describeOwnerBootstrapError(caught), /could not reach the server \(HTTP 522\).*repeating owner bootstrap is safe/)
})
