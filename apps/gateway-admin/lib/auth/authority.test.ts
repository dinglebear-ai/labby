import test from 'node:test'
import assert from 'node:assert/strict'

import { authorityCacheKey, authorityIdentity, parseAuthoritySnapshot, resetAuthorityOpaqueValues, selectAuthorityWorkspace } from './authority.ts'
import { __authorityContextStatsForTests, __resetAuthorityContextForTests, beginAuthorityRequest, invalidateAuthorityRequests } from './authority-context.ts'

const payload = {
  owner: { kind: 'personal', id: 'principal-1' },
  organization_id: 'org-1',
  teams: [{ id: 'team-a', role: 'owner', membership_epoch: 2, policy_epoch: 4 }],
  projects: [{ id: 'project-a', role: 'manager' }],
  capabilities: ['scope.read', 'scope.read', 'scope.operate'],
  authority_generation: 7,
}

test.afterEach(() => {
  __resetAuthorityContextForTests()
  resetAuthorityOpaqueValues()
})

test('parses server authority and produces an opaque context-qualified cache key', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  assert.equal(snapshot.principalId, 'principal-1')
  assert.deepEqual(snapshot.capabilities, ['scope.operate', 'scope.read'])
  const key = authorityCacheKey(snapshot, 'depot-east')
  assert.equal(key.includes('principal-1'), false)
  assert.equal(key.includes('depot-east'), false)
  assert.deepEqual(authorityCacheKey(snapshot, 'depot-east'), key, 'the same subject maps to the same opaque token within a session')
})

test('logout forgets opaque subject tokens so a later session cannot be correlated', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  const before = authorityCacheKey(snapshot)
  resetAuthorityOpaqueValues()
  const after = authorityCacheKey(snapshot)
  assert.notEqual(before[3], after[3], 'principal token must be re-minted after logout')
  assert.notEqual(before[5], after[5], 'owner token must be re-minted after logout')
})

test('capabilities and selectors are trimmed and de-duplicated', () => {
  const snapshot = parseAuthoritySnapshot({ ...payload, capabilities: [' scope.read ', 'scope.read', 'scope.operate\n'], active_team_id: ' team-a ' })
  assert.deepEqual(snapshot.capabilities, ['scope.operate', 'scope.read'])
  assert.equal(snapshot.activeTeamId, 'team-a')
})

test('project display names are optional and never required for authorization', () => {
  const named = parseAuthoritySnapshot({ ...payload, projects: [{ id: 'project-a', role: 'manager', name: 'Project Alpha' }] })
  assert.deepEqual(named.projects, [{ id: 'project-a', role: 'manager', name: 'Project Alpha' }])
  assert.deepEqual(parseAuthoritySnapshot(payload).projects, [{ id: 'project-a', role: 'manager' }])
  assert.throws(() => parseAuthoritySnapshot({ ...payload, projects: [{ id: 'project-a', role: 'manager', name: '   ' }] }), /project\.name must be a non-empty string/)
})

test('workspace selectors only accept server-projected teams and projects', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  assert.equal(selectAuthorityWorkspace(snapshot, { teamId: 'team-a' }).activeOwner.kind, 'team')
  assert.equal(selectAuthorityWorkspace(snapshot, { teamId: 'team-a', projectId: 'project-a' }).activeOwner.kind, 'project')
  assert.equal(selectAuthorityWorkspace(snapshot, {}).activeOwner.kind, 'personal')
  assert.throws(() => selectAuthorityWorkspace(snapshot, { teamId: 'team-b' }), /not available/)
  assert.throws(() => selectAuthorityWorkspace(snapshot, { projectId: 'project-b' }), /not available/)
})

test('malformed authority projections are rejected instead of partially trusted', () => {
  for (const broken of [
    { ...payload, organization_id: '' },
    { ...payload, authority_generation: -1 },
    { ...payload, capabilities: ['scope.read', 7] },
    { ...payload, teams: [{ id: 'team-a', role: 'owner' }] },
    { ...payload, owner: { kind: 'unknown', id: 'x' } },
  ]) assert.throws(() => parseAuthoritySnapshot(broken), /Malformed authority response/)
})

test('optional selector errors name the offending field', () => {
  assert.throws(() => parseAuthoritySnapshot({ ...payload, active_team_id: '   ' }), /active_team_id must be a non-empty string/)
  assert.throws(() => parseAuthoritySnapshot({ ...payload, active_project_id: '' }), /active_project_id must be a non-empty string/)
})

test('authority identity covers every authorization-relevant field and nothing else', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  assert.equal(authorityIdentity(undefined), 'authority-unavailable')
  assert.equal(authorityIdentity(snapshot), authorityIdentity({ ...snapshot }))
  assert.notEqual(authorityIdentity(snapshot), authorityIdentity(undefined), 'gaining or losing a projection is a change')
  assert.notEqual(authorityIdentity(snapshot), authorityIdentity({ ...snapshot, generation: 8 }))
  assert.notEqual(authorityIdentity(snapshot), authorityIdentity({ ...snapshot, principalId: 'principal-2' }))
  assert.notEqual(authorityIdentity(snapshot), authorityIdentity({ ...snapshot, capabilities: ['scope.read'] }))
  assert.notEqual(authorityIdentity(snapshot), authorityIdentity(selectAuthorityWorkspace(snapshot, { teamId: 'team-a' })))
  assert.notEqual(authorityIdentity(selectAuthorityWorkspace(snapshot, { teamId: 'team-a' })), authorityIdentity(selectAuthorityWorkspace(snapshot, { teamId: 'team-a', projectId: 'project-a' })))
  // Ordering of capabilities is not an authority change.
  assert.equal(authorityIdentity(snapshot), authorityIdentity({ ...snapshot, capabilities: ['scope.read', 'scope.operate'] }))
})

test('context invalidation aborts old requests but preserves the current generation', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  const old = beginAuthorityRequest(snapshot, 10)
  const current = beginAuthorityRequest(snapshot, 11)
  invalidateAuthorityRequests(11)
  assert.equal(old.signal.aborted, true)
  assert.equal(current.signal.aborted, false)
  old.finish()
  current.finish()
})

test('invalidation drops every superseded generation so the controller map cannot grow', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  const started: Array<ReturnType<typeof beginAuthorityRequest>> = []
  for (let generation = 1; generation <= 20; generation += 1) {
    started.push(beginAuthorityRequest(snapshot, generation))
    invalidateAuthorityRequests(generation)
  }
  const stats = __authorityContextStatsForTests()
  assert.equal(stats.activeGenerations, 1, 'only the current generation may retain controllers')
  assert.equal(stats.inFlight, 1)
  assert.deepEqual(started.slice(0, -1).map((request) => request.signal.aborted), Array(19).fill(true))
  assert.equal(started.at(-1)?.signal.aborted, false)
  started.at(-1)?.finish()
  assert.deepEqual(__authorityContextStatsForTests(), { activeGenerations: 0, inFlight: 0 }, 'finishing the last request releases its generation bucket')
})

test('a caller signal aborts the authority-scoped request without disturbing its siblings', () => {
  const snapshot = parseAuthoritySnapshot(payload)
  const caller = new AbortController()
  const scoped = beginAuthorityRequest(snapshot, 3, 'local', caller.signal)
  const sibling = beginAuthorityRequest(snapshot, 3)
  caller.abort()
  assert.equal(scoped.signal.aborted, true)
  assert.equal(sibling.signal.aborted, false)
  scoped.finish()
  sibling.finish()
})
