import test from 'node:test'
import assert from 'node:assert/strict'

import { agentOwnerChoices, listTeamNames } from './owners'
import type { AuthoritySnapshot } from '@/lib/auth/authority'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'

const authority: AuthoritySnapshot = {
  schemaVersion: 1,
  compatibilityGeneration: 1,
  principalId: 'person-1',
  organizationId: 'org-1',
  activeOwner: { kind: 'personal', id: 'person-1' },
  teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }],
  projects: [
    { id: 'project-1', role: 'member' },
    { id: 'project-2', role: 'viewer' },
  ],
  capabilities: ['scope.read', 'scope.create'],
  generation: 1,
}

test('Agent owners use readable labels and omit viewer-only projects', () => {
  assert.deepEqual(agentOwnerChoices(
    authority,
    new Map([['team-1', 'Engineering']]),
    new Map([['project-1', 'Gateway']]),
  ), [
    { kind: 'personal', id: 'person-1', label: 'Personal workspace', key: 'personal' },
    { kind: 'team', id: 'team-1', label: 'Engineering', key: 'team:team-1' },
    { kind: 'project', id: 'project-1', label: 'Gateway', key: 'project:project-1' },
  ])
  assert.deepEqual(agentOwnerChoices({ ...authority, capabilities: ['scope.read'] }, new Map(), new Map()), [])
})

test('team names come from the authenticated access projection', async () => {
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'person-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority })
  let request: Request | undefined
  globalThis.fetch = async (input, init) => {
    request = new Request(new URL(String(input), 'http://labby.test'), init)
    return Response.json({ teams: [{ team_id: 'team-1', name: 'Engineering' }] })
  }
  try {
    assert.equal((await listTeamNames()).get('team-1'), 'Engineering')
    assert.equal(new URL(request!.url).pathname, '/v1/access/admin/')
    assert.deepEqual(JSON.parse(await request!.text()), { action: 'access.team.list', params: {} })
  } finally {
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
