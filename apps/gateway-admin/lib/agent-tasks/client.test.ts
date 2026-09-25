import assert from 'node:assert/strict'
import test from 'node:test'

import { cancelAgentSession, listAgents, listTasks } from './client.ts'
import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'

const authority = { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1', activeOwner: { kind: 'team' as const, id: 'team-1' }, activeTeamId: 'team-1', teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read'], generation: 1 } as const
function authenticate() { __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority }) }
const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

test('agent and task lists use authenticated authoritative action endpoints', async () => {
  authenticate()
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    return Response.json(new URL(request.url).pathname === '/v1/agents' ? { agents: [{ agent_id: 'a-1' }] } : { tasks: [{ task_id: 't-1' }] })
  }
  assert.equal((await listAgents())[0]?.agent_id, 'a-1')
  assert.equal((await listTasks())[0]?.task_id, 't-1')
  assert.deepEqual(requests.map(request => new URL(request.url).pathname), ['/v1/agents', '/v1/tasks'])
  assert.ok(requests.every(request => request.method === 'POST' && request.credentials === 'include'))
  assert.deepEqual(JSON.parse(await requests[0]!.text()), { action: 'agents.list', params: {} })
  assert.deepEqual(JSON.parse(await requests[1]!.text()), { action: 'tasks.list', params: {} })
})

test('agent and task lists follow authoritative pagination cursors', async () => {
  authenticate()
  const requests: Array<{ action: string; params: Record<string, unknown> }> = []
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params: Record<string, unknown> }
    requests.push(body)
    const cursor = body.params.cursor
    if (body.action === 'agents.list') {
      return Response.json(cursor ? { agents: [{ agent_id: 'a-2' }], next_cursor: null } : { agents: [{ agent_id: 'a-1' }], next_cursor: 'a-1' })
    }
    return Response.json(cursor ? { tasks: [{ task_id: 't-2' }], next_cursor: null } : { tasks: [{ task_id: 't-1' }], next_cursor: 't-1' })
  }

  assert.deepEqual((await listAgents()).map(agent => agent.agent_id), ['a-1', 'a-2'])
  assert.deepEqual((await listTasks()).map(task => task.task_id), ['t-1', 't-2'])
  assert.deepEqual(requests.map(request => request.params), [{}, { cursor: 'a-1' }, {}, { cursor: 't-1' }])
})

test('agent session cancel posts the shared cancel action with the session binding', async () => {
  authenticate()
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    return Response.json({ agent_id: 'a-1', session_id: 's-1', status: 'cancelling', cancel_requested: true })
  }
  const result = await cancelAgentSession('a-1', 's-1')
  assert.equal(result.status, 'cancelling')
  assert.equal(result.cancel_requested, true)
  assert.equal(requests.length, 1)
  assert.equal(new URL(requests[0]!.url).pathname, '/v1/agents')
  assert.equal(requests[0]!.method, 'POST')
  assert.equal(requests[0]!.headers.get('x-csrf-token'), 'csrf')
  assert.deepEqual(JSON.parse(await requests[0]!.text()), { action: 'agents.session.cancel', params: { agent_id: 'a-1', session_id: 's-1' } })
})

test('repeated pagination cursors fail closed', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ agents: [{ agent_id: 'a-1' }], next_cursor: 'same' })
  await assert.rejects(listAgents(), /repeated pagination cursor/)
})

test('denials do not become empty authoritative lists', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ message: 'access denied' }, { status: 403 })
  await assert.rejects(listAgents(), /failed \(403\)/)
  await assert.rejects(listTasks(), /failed \(403\)/)
})
