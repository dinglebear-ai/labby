import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { accessApi } from './access-client.ts'

const originalFetch = globalThis.fetch

test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

function signedInUnprovisioned() {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'invitee-subject', email: 'invitee@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-invitee',
    authorityState: 'unprovisioned',
  })
}

test('acceptTeamInvitation sends only the opaque invitation token through authenticated access dispatch', async () => {
  signedInUnprovisioned()
  const token = 'ab'.repeat(32)
  let captured: { url: string; init?: RequestInit } | undefined
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    captured = { url: String(url), init }
    return new Response(JSON.stringify({
      team_id: 'engineering',
      principal_id: 'invitee',
      role: 'member',
      status: 'active',
      membership_epoch: 2,
    }), { status: 200 })
  }) as typeof fetch

  const outcome = await accessApi.acceptTeamInvitation('  ' + token + '  ')

  assert.equal(captured?.url, '/v1/access/admin')
  assert.equal(captured?.init?.method, 'POST')
  assert.equal(new Headers(captured?.init?.headers).get('x-csrf-token'), 'csrf-invitee')
  assert.deepEqual(JSON.parse(String(captured?.init?.body)), {
    action: 'access.team_invitation.accept',
    params: { token },
  })
  assert.equal(outcome.team_id, 'engineering')
})

test('createTeamInvitation is email-first and never accepts a caller-supplied invitation token', async () => {
  signedInUnprovisioned()
  let body: { action: string; params: Record<string, unknown> } | undefined
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    body = JSON.parse(String(init?.body)) as typeof body
    return new Response(JSON.stringify({
      team_id: 'engineering',
      role: 'member',
      status: 'pending',
      team_membership_epoch: 4,
      expires_at: 999,
      token: 'cd'.repeat(32),
    }), { status: 200 })
  }) as typeof fetch

  const outcome = await accessApi.createTeamInvitation('engineering', ' New.User@Example.COM ', 'member')

  assert.equal(body?.action, 'access.team_invitation.create')
  assert.deepEqual(body?.params, {
    team_id: 'engineering',
    email: 'New.User@Example.COM',
    role: 'member',
    ttl_seconds: 7 * 24 * 60 * 60,
  })
  assert.equal(Object.hasOwn(body?.params ?? {}, 'principal_id'), false)
  assert.equal(Object.hasOwn(body?.params ?? {}, 'token'), false)
  assert.equal(outcome.token, 'cd'.repeat(32))
})
