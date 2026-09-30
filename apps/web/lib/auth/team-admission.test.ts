import test from 'node:test'
import assert from 'node:assert/strict'

import { requestTeamAdmission } from './team-admission.ts'
import { __setBrowserSessionStateForTests, getBrowserSessionState } from './session-store.ts'

type FetchMock = typeof globalThis.fetch

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

function unprovisionedSession() {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'viewer-subject', email: 'viewer@example.com' },
    expiresAt: 124,
    csrfToken: 'csrf-viewer',
    authorityState: 'unprovisioned',
  })
}

const admittedSession = {
  authenticated: true,
  authority_state: 'ready',
  authority: { principal_id: 'viewer-principal', organization_id: 'bootstrap-local', authority_generation: 4 },
  user: { sub: 'viewer-subject', email: 'viewer@example.com' },
  owner: { kind: 'personal', id: 'viewer-principal' },
  organization_id: 'bootstrap-local',
  teams: [],
  projects: [],
  capabilities: [],
  authority_generation: 4,
  expires_at: 125,
  csrf_token: 'csrf-viewer-2',
}

test('requestTeamAdmission makes one read-only /v1 request, then reloads the session', async () => {
  unprovisionedSession()
  const calls: Array<{ url: string; init?: RequestInit }> = []
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init })
    if (String(url) === '/v1/catalog') return new Response('{}', { status: 200 })
    return new Response(JSON.stringify(admittedSession), { status: 200 })
  }) as FetchMock

  const result = await requestTeamAdmission()

  assert.equal(result.admissionError, undefined)
  assert.equal(calls[0]?.url, '/v1/catalog')
  assert.equal(calls[0]?.init?.method, 'GET')
  assert.equal(calls[0]?.init?.credentials, 'include')
  assert.ok(calls.some((call) => call.url.endsWith('/auth/session')), 'session is reloaded')
  const state = getBrowserSessionState()
  assert.equal(state.status, 'authenticated')
  assert.equal(state.status === 'authenticated' ? state.authorityState : undefined, 'ready')
})

test('requestTeamAdmission still reloads the session when the /v1 request fails', async () => {
  unprovisionedSession()
  const urls: string[] = []
  globalThis.fetch = (async (url: string | URL | Request) => {
    urls.push(String(url))
    if (String(url) === '/v1/catalog') throw new TypeError('network down')
    return new Response(JSON.stringify({ ...admittedSession, authority_state: 'unprovisioned', authority: null }), {
      status: 200,
    })
  }) as FetchMock

  const result = await requestTeamAdmission()

  assert.ok(urls.some((url) => url.endsWith('/auth/session')), 'session is reloaded')
  assert.match(result.admissionError ?? '', /network down/)
})
