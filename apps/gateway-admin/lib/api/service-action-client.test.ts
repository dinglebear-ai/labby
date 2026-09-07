import test from 'node:test'
import assert from 'node:assert/strict'

import {
  __setBrowserSessionStateForTests,
  getBrowserSessionState,
  logoutBrowserSession,
} from '../auth/session-store.ts'
import { safeFanout } from './service-action-client'
import { performServiceAction, type ServiceActionError } from './service-action-client'

class TestActionError extends Error implements ServiceActionError {
  constructor(
    message: string,
    public status: number,
    public code?: string,
  ) {
    super(message)
  }
}

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

const authority = { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'one', organizationId: 'org', activeOwner: { kind: 'personal' as const, id: 'one' }, activeTeamId: undefined, activeProjectId: undefined, teams: [{ id: 'team-a', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read'], generation: 1 } as const

function blockedFetch() {
  let release: (() => void) | undefined
  const blocked = new Promise<void>((resolve) => { release = resolve })
  globalThis.fetch = (async () => { await blocked; return new Response(JSON.stringify({ ok: true }), { status: 200 }) }) as typeof fetch
  return () => release?.()
}

const run = () => performServiceAction({ action: 'artifacts.list', params: {}, serviceLabel: 'Library', url: '/v1/artifacts', createError: (message, status, code) => new TestActionError(message, status, code) })
const isAbort = (error: unknown) => error instanceof DOMException && error.name === 'AbortError'

test('performServiceAction rejects a response from a superseded authority context', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one', authority })
  const release = blockedFetch()
  const pending = run()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'two' }, expiresAt: 2, csrfToken: 'two', authority: { ...authority, principalId: 'two', activeOwner: { kind: 'personal', id: 'two' }, generation: 2 } })
  release()
  await assert.rejects(pending, isAbort)
})

test('performServiceAction treats a team or project switch under the same generation as a change', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one', authority })
  const release = blockedFetch()
  const pending = run()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one', authority: { ...authority, activeOwner: { kind: 'team', id: 'team-a' }, activeTeamId: 'team-a' } })
  release()
  await assert.rejects(pending, isAbort)
})

test('performServiceAction treats gaining or losing the authority projection as a change', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one' })
  let release = blockedFetch()
  let pending = run()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one', authority })
  release()
  await assert.rejects(pending, isAbort, 'a projection arriving mid-request supersedes the unprojected request')

  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one', authority })
  release = blockedFetch()
  pending = run()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one' })
  release()
  await assert.rejects(pending, isAbort, 'losing the projection mid-request supersedes the projected request')
})

test('performServiceAction accepts a response when only transport fields changed in flight', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'one', authority })
  const release = blockedFetch()
  const pending = run()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 99, csrfToken: 'rotated', authority: { ...authority } })
  release()
  assert.deepEqual(await pending, { ok: true })
})

test('performServiceAction retries a CSRF failure under a stable authority projection and re-captures it for the retry', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'one' }, expiresAt: 1, csrfToken: 'expired-csrf', authority })
  const actionCsrfTokens: Array<string | undefined> = []
  globalThis.fetch = (async (input, init) => {
    if (input === '/auth/session') {
      return new Response(JSON.stringify({
        authenticated: true,
        user: { sub: 'one' },
        expires_at: 456,
        csrf_token: 'fresh-csrf',
        principal_id: 'one',
        organization_id: 'org',
        active_owner: { kind: 'personal', id: 'one' },
        teams: [{ id: 'team-a', role: 'member', membership_epoch: 1, policy_epoch: 1 }],
        projects: [],
        capabilities: ['scope.read'],
        authority_generation: 1,
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    actionCsrfTokens.push((init?.headers as Record<string, string>)['x-csrf-token'])
    if (actionCsrfTokens.length === 1) {
      return new Response(JSON.stringify({ kind: 'validation_failed', message: 'CSRF token mismatch' }), { status: 403, headers: { 'content-type': 'application/json' } })
    }
    return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch

  assert.deepEqual(await run(), { ok: true })
  assert.deepEqual(actionCsrfTokens, ['expired-csrf', 'fresh-csrf'])
})

test('safeFanout returns per-item failures without rejecting the whole fan-out', async () => {
  const results = await safeFanout([1, 2, 3], async (item) => {
    if (item === 2) {
      throw new Error('bad item')
    }
    return item * 10
  })

  assert.deepEqual(
    results.map((result) => result.ok),
    [true, false, true],
  )
  assert.deepEqual(
    results.map((result) => result.item),
    [1, 2, 3],
  )
  assert.equal(results[0].ok && results[0].value, 10)
  assert.equal(!results[1].ok && results[1].error instanceof Error, true)
  assert.equal(results[2].ok && results[2].value, 30)
})

test('performServiceAction reloads an expired browser session and retries once with the fresh csrf token', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'expired-csrf',
  })
  const actionCsrfTokens: Array<string | undefined> = []
  let sessionLoads = 0

  globalThis.fetch = (async (input, init) => {
    if (input === '/auth/session') {
      sessionLoads += 1
      return new Response(JSON.stringify({
        authenticated: true,
        user: { sub: 'browser-user', email: 'browser@example.com' },
        expires_at: 456,
        csrf_token: 'fresh-csrf',
        is_admin: true,
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }

    const headers = init?.headers as Record<string, string>
    actionCsrfTokens.push(headers['x-csrf-token'])
    if (actionCsrfTokens.length === 1) {
      return new Response(JSON.stringify({ kind: 'auth_failed', message: 'session expired' }), {
        status: 401,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  try {
    const result = await performServiceAction<{ ok: boolean }, TestActionError>({
      action: 'gateway.list',
      params: {},
      serviceLabel: 'Gateway',
      url: '/v1/gateway',
      createError: (message, status, code) => new TestActionError(message, status, code),
    })

    assert.deepEqual(result, { ok: true })
    assert.equal(sessionLoads, 1)
    assert.deepEqual(actionCsrfTokens, ['expired-csrf', 'fresh-csrf'])
    assert.equal(getBrowserSessionState().status, 'authenticated')
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('performServiceAction does not retry when session reload confirms logout', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 123,
    csrfToken: 'expired-csrf',
  })
  let actionCalls = 0

  globalThis.fetch = (async (input) => {
    if (input === '/auth/session') {
      return new Response(JSON.stringify({ authenticated: false }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    actionCalls += 1
    return new Response(JSON.stringify({ kind: 'auth_failed', message: 'session expired' }), {
      status: 401,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  try {
    await assert.rejects(
      performServiceAction({
        action: 'gateway.list',
        params: {},
        serviceLabel: 'Gateway',
        url: '/v1/gateway',
        createError: (message, status, code) => new TestActionError(message, status, code),
      }),
      (error: unknown) => error instanceof TestActionError && error.status === 401,
    )
    assert.equal(actionCalls, 1)
    assert.deepEqual(getBrowserSessionState(), { status: 'unauthenticated' })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('performServiceAction coalesces concurrent browser session refreshes', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 123,
    csrfToken: 'expired-csrf',
  })
  let sessionLoads = 0
  const actionAttempts = new Map<string, number>()

  globalThis.fetch = (async (input, init) => {
    if (input === '/auth/session') {
      sessionLoads += 1
      await new Promise((resolve) => setTimeout(resolve, 10))
      return new Response(JSON.stringify({
        authenticated: true,
        user: { sub: 'browser-user' },
        expires_at: 456,
        csrf_token: 'fresh-csrf',
        is_admin: true,
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }

    const body = JSON.parse(String(init?.body)) as { params: { request: string } }
    const request = body.params.request
    const attempts = (actionAttempts.get(request) ?? 0) + 1
    actionAttempts.set(request, attempts)
    if (attempts === 1) {
      return new Response(JSON.stringify({ kind: 'auth_failed', message: 'session expired' }), {
        status: 401,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ request }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  try {
    const run = (request: string) => performServiceAction<{ request: string }, TestActionError>({
      action: 'gateway.list',
      params: { request },
      serviceLabel: 'Gateway',
      url: '/v1/gateway',
      createError: (message, status, code) => new TestActionError(message, status, code),
    })
    const results = await Promise.all([run('one'), run('two')])

    assert.deepEqual(results, [{ request: 'one' }, { request: 'two' }])
    assert.equal(sessionLoads, 1)
    assert.deepEqual([...actionAttempts.entries()], [['one', 2], ['two', 2]])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('performServiceAction reuses a refresh completed while a slow request was in flight', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 123,
    csrfToken: 'expired-csrf',
  })
  let sessionLoads = 0
  const actionAttempts = new Map<string, number>()

  globalThis.fetch = (async (input, init) => {
    if (input === '/auth/session') {
      sessionLoads += 1
      return new Response(JSON.stringify({
        authenticated: true,
        user: { sub: 'browser-user' },
        expires_at: 456,
        csrf_token: 'fresh-csrf',
        is_admin: true,
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }

    const body = JSON.parse(String(init?.body)) as { params: { request: string } }
    const request = body.params.request
    const attempts = (actionAttempts.get(request) ?? 0) + 1
    actionAttempts.set(request, attempts)
    if (attempts === 1) {
      if (request === 'slow') {
        await new Promise((resolve) => setTimeout(resolve, 20))
      }
      return new Response(JSON.stringify({ kind: 'auth_failed', message: 'session expired' }), {
        status: 401,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ request }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  try {
    const run = (request: string) => performServiceAction<{ request: string }, TestActionError>({
      action: 'gateway.list',
      params: { request },
      serviceLabel: 'Gateway',
      url: '/v1/gateway',
      createError: (message, status, code) => new TestActionError(message, status, code),
    })
    const results = await Promise.all([run('fast'), run('slow')])

    assert.deepEqual(results, [{ request: 'fast' }, { request: 'slow' }])
    assert.equal(sessionLoads, 1)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('performServiceAction does not retry from a refresh invalidated by successful logout', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user' },
    expiresAt: 123,
    csrfToken: 'csrf-123',
  })
  let releaseSessionLoad: (() => void) | undefined
  const sessionLoadBlocked = new Promise<void>((resolve) => {
    releaseSessionLoad = resolve
  })
  let actionCalls = 0

  globalThis.fetch = (async (input) => {
    if (input === '/auth/session') {
      await sessionLoadBlocked
      return new Response(JSON.stringify({
        authenticated: true,
        user: { sub: 'browser-user' },
        expires_at: 456,
        csrf_token: 'stale-csrf',
        is_admin: true,
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    if (input === '/auth/logout') {
      return new Response(null, { status: 204 })
    }
    actionCalls += 1
    return new Response(JSON.stringify({ kind: 'auth_failed', message: 'session expired' }), {
      status: 401,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch

  try {
    const action = performServiceAction({
      action: 'gateway.list',
      params: {},
      serviceLabel: 'Gateway',
      url: '/v1/gateway',
      createError: (message, status, code) => new TestActionError(message, status, code),
    })
    await new Promise((resolve) => setTimeout(resolve, 0))
    await logoutBrowserSession()
    releaseSessionLoad?.()

    await assert.rejects(action, (error: unknown) =>
      error instanceof TestActionError && error.status === 401,
    )
    assert.equal(actionCalls, 1)
    assert.deepEqual(getBrowserSessionState(), { status: 'unauthenticated' })
  } finally {
    globalThis.fetch = originalFetch
  }
})
