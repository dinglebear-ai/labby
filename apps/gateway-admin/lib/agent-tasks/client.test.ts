import assert from 'node:assert/strict'
import test from 'node:test'

import {
  AgentTaskClientError,
  createAgentFromHarness,
  getAgentTranscript,
  listAgentHarnesses,
  listAgentSessions,
  listAgents,
  listTasks,
  resumeAgentSession,
  runAgent,
  stopAgentSession,
} from './client.ts'
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
  assert.deepEqual(JSON.parse(await requests[0]!.text()), { action: 'agents.list', params: { limit: '100' } })
  assert.deepEqual(JSON.parse(await requests[1]!.text()), { action: 'tasks.list', params: {} })
})

test('agent and session lists follow bounded string-cursor pages', async () => {
  authenticate()
  const requests: Array<{ action: string; params: Record<string, unknown> }> = []
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params: Record<string, unknown> }
    requests.push(body)
    if (body.action === 'agents.list') {
      return Response.json(body.params.cursor
        ? { agents: [{ agent_id: 'agent-2' }], next_cursor: null }
        : { agents: [{ agent_id: 'agent-1' }], next_cursor: 'agent-1' })
    }
    return Response.json(body.params.cursor
      ? { sessions: [{ session_id: 'session-2' }], next_cursor: null }
      : { sessions: [{ session_id: 'session-1' }], next_cursor: 'session-1' })
  }

  assert.deepEqual((await listAgents()).map(agent => agent.agent_id), ['agent-1', 'agent-2'])
  assert.deepEqual((await listAgentSessions('agent-1')).map(session => session.session_id), ['session-1', 'session-2'])
  assert.deepEqual(requests, [
    { action: 'agents.list', params: { limit: '100' } },
    { action: 'agents.list', params: { limit: '100', cursor: 'agent-1' } },
    { action: 'agents.sessions.list', params: { agent_id: 'agent-1', limit: '100' } },
    { action: 'agents.sessions.list', params: { agent_id: 'agent-1', limit: '100', cursor: 'session-1' } },
  ])
})

test('denials do not become empty authoritative lists', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ kind: 'access_denied', message: 'access denied' }, { status: 403 })
  await assert.rejects(listAgents(), (error: unknown) => error instanceof AgentTaskClientError && error.status === 403 && error.code === 'access_denied' && error.message === 'access denied')
  await assert.rejects(listTasks(), (error: unknown) => error instanceof AgentTaskClientError && error.status === 403 && error.code === 'access_denied' && error.message === 'access denied')
})

test('agent creation copies the exact approved harness bundle into the active owner', async () => {
  authenticate()
  let request: Request | undefined
  const harness = {
    id: 'codex-approved',
    digest: 'sha256:harness',
    available: true,
    content_digest: 'sha256:content',
    repository_digest: 'sha256:repository',
    image_digest: 'sha256:image',
    loadout_digest: 'sha256:loadout',
    catalog_generation: 'catalog-4',
  }
  globalThis.fetch = async (input, init) => {
    request = new Request(new URL(String(input), 'http://labby.test'), init)
    return Response.json({ agent_id: 'first-agent', owner_kind: 'team', owner_id: 'team-1', version: 1, state: 'active', harness_id: harness.id, harness_digest: harness.digest, ...harness })
  }

  const created = await createAgentFromHarness('first-agent', harness)

  assert.equal(created.agent_id, 'first-agent')
  assert.deepEqual(JSON.parse(await request!.text()), {
    action: 'agents.create',
    params: {
      agent_id: 'first-agent',
      owner_kind: 'team',
      owner_id: 'team-1',
      content_digest: 'sha256:content',
      repository_digest: 'sha256:repository',
      image_digest: 'sha256:image',
      harness_digest: 'sha256:harness',
      loadout_digest: 'sha256:loadout',
      catalog_generation: 'catalog-4',
    },
  })
})

test('agent session helpers preserve exact service actions and identifiers', async () => {
  authenticate()
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    const body = JSON.parse(await request.clone().text()) as { action: string }
    if (body.action === 'agents.harnesses') return Response.json({ harnesses: [{ id: 'codex', available: true }] })
    if (body.action === 'agents.sessions.list') return Response.json({ sessions: [{ agent_id: 'agent-1', session_id: 'session-1' }] })
    if (body.action === 'agents.session.transcript') return Response.json({ agent_id: 'agent-1', session_id: 'session-1', input: 'inspect', transcript: 'done', truncated: false })
    return Response.json({ agent_id: 'agent-1', agent_version: 2, session_id: body.action === 'agents.session.resume' ? 'session-2' : 'session-1', status: 'admitted', input_digest: 'sha256:input', authority_expires_at: 10 })
  }

  await listAgentHarnesses()
  await listAgentSessions('agent-1')
  await runAgent('agent-1', 'inspect', 'run-request')
  await getAgentTranscript('agent-1', 'session-1')
  await stopAgentSession('agent-1', 'session-1')
  assert.equal((await resumeAgentSession('agent-1', 'session-1')).session_id, 'session-2')

  const bodies = await Promise.all(requests.map(async request => JSON.parse(await request.text())))
  assert.deepEqual(bodies.slice(0, 5), [
    { action: 'agents.harnesses', params: {} },
    { action: 'agents.sessions.list', params: { agent_id: 'agent-1', limit: '100' } },
    { action: 'agents.run', params: { agent_id: 'agent-1', input: 'inspect', idempotency_key: 'run-request' } },
    { action: 'agents.session.transcript', params: { agent_id: 'agent-1', session_id: 'session-1' } },
    { action: 'agents.session.stop', params: { agent_id: 'agent-1', session_id: 'session-1' } },
  ])
  assert.equal(bodies[5].action, 'agents.session.resume')
  assert.equal(bodies[5].params.agent_id, 'agent-1')
  assert.equal(bodies[5].params.session_id, 'session-1')
  assert.match(bodies[5].params.idempotency_key, /^[0-9a-f-]{36}$/)
})
