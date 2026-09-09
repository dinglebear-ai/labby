import assert from 'node:assert/strict'
import test from 'node:test'

import { listAgents, listTasks } from './agent-tasks/client.ts'
import { listProjects } from './projects/client.ts'
import { __resetAuthorityContextForTests } from './auth/authority-context.ts'
import { __setBrowserSessionStateForTests, loadBrowserSession, selectSessionWorkspace } from './auth/session-store.ts'

const authority = {
  schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1',
  activeOwner: { kind: 'team' as const, id: 'team-owner' }, activeTeamId: 'team-owner',
  teams: [
    { id: 'team-owner', role: 'owner', membershipEpoch: 1, policyEpoch: 1 },
    { id: 'team-admin', role: 'admin', membershipEpoch: 1, policyEpoch: 1 },
    { id: 'team-member', role: 'member', membershipEpoch: 1, policyEpoch: 1 },
  ], projects: [], capabilities: ['scope.read'], generation: 7,
} as const

function authenticate() {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority })
}

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __resetAuthorityContextForTests()
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

function pendingUntilAborted() {
  globalThis.fetch = async (_input, init) => new Promise<Response>((_resolve, reject) => {
    const signal = init?.signal
    signal?.addEventListener('abort', () => reject(signal.reason), { once: true })
  })
}

test('project, agent, and task requests carry the active workspace and are bound to the session authority', async () => {
  authenticate()
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    return Response.json(request.url.includes('/projects/') ? [] : { agents: [], tasks: [] })
  }
  await listProjects()
  await listAgents()
  await listTasks()
  assert.deepEqual(requests.map(request => new URL(request.url).pathname), ['/v1/projects/', '/v1/agents/', '/v1/tasks/'])
  for (const request of requests) {
    assert.equal(request.credentials, 'include')
    assert.equal(new URL(request.url).search, '', 'workspace selection must not be carried in the URL')
    assert.ok(request.signal instanceof AbortSignal && !request.signal.aborted, 'every request must be abortable by the authority context')
  }
})

test('personal and outsider denials remain errors across project, agent, and task surfaces', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ kind: 'not_found' }, { status: 404 })
  await assert.rejects(listProjects(), /failed \(404\)/)
  await assert.rejects(listAgents(), /failed \(404\)/)
  await assert.rejects(listTasks(), /failed \(404\)/)
})

test('workspace switching aborts stale project, agent, and task requests', async () => {
  for (const load of [listProjects, listAgents, listTasks]) {
    authenticate()
    pendingUntilAborted()
    const pending = load()
    selectSessionWorkspace({ teamId: 'team-admin' })
    await assert.rejects(pending, error => error instanceof DOMException && error.name === 'AbortError')
  }
})

test('a server-side authority change observed on session refresh aborts stale requests', async () => {
  authenticate()
  pendingUntilAborted()
  const pending = listProjects()
  globalThis.fetch = async () => Response.json({
    authenticated: true, user: { sub: 'principal-1' }, expires_at: 999, csrf_token: 'csrf-2',
    principal_id: 'principal-1', organization_id: 'org-1',
    active_owner: { kind: 'team', id: 'team-owner' }, active_team_id: 'team-owner',
    teams: [{ id: 'team-owner', role: 'member', membership_epoch: 2, policy_epoch: 1 }], projects: [],
    capabilities: ['scope.read'], authority_generation: 8,
  })
  await loadBrowserSession()
  await assert.rejects(pending, error => error instanceof DOMException && error.name === 'AbortError')
})

test('a transport-only session refresh does not abort in-flight workspace requests', async () => {
  authenticate()
  let release: ((response: Response) => void) | undefined
  globalThis.fetch = async (input, init) => {
    if (input === '/auth/session') {
      return Response.json({
        authenticated: true, user: { sub: 'principal-1' }, expires_at: 999, csrf_token: 'csrf-rotated',
        principal_id: 'principal-1', organization_id: 'org-1',
        active_owner: { kind: 'team', id: 'team-owner' }, active_team_id: 'team-owner',
        teams: authority.teams.map(team => ({ id: team.id, role: team.role, membership_epoch: team.membershipEpoch, policy_epoch: team.policyEpoch })), projects: [],
        capabilities: ['scope.read'], authority_generation: 7,
      })
    }
    return new Promise<Response>((resolve, reject) => {
      release = resolve
      init?.signal?.addEventListener('abort', () => reject(init.signal?.reason), { once: true })
    })
  }
  const pending = listProjects()
  await loadBrowserSession()
  release?.(Response.json([{ project_id: 'p', team_id: 'team-owner', name: 'P', status: 'active', role: 'owner', policy_epoch: 1, can_manage: true }]))
  assert.deepEqual((await pending).map(row => row.project_id), ['p'])
})

test('runtime unavailable responses remain explicit failures', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ kind: 'runtime_unavailable' }, { status: 503 })
  await assert.rejects(listAgents(), /failed \(503\)/)
  await assert.rejects(listTasks(), /failed \(503\)/)
})
