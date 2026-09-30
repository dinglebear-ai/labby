import assert from 'node:assert/strict'
import test from 'node:test'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import { createTaskSchedule, listScheduleAgents, listTaskSchedules, pauseTaskSchedule, runTaskSchedule } from './schedules'
const authority = { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal', organizationId: 'org', activeOwner: { kind: 'project' as const, id: 'project' }, activeTeamId: 'team', activeProjectId: 'project', teams: [{ id: 'team', role: 'owner', membershipEpoch: 1, policyEpoch: 1 }], projects: [{ id: 'project', role: 'member' }], capabilities: ['scope.read', 'scope.create', 'scope.operate'], generation: 1 } as const
const original = globalThis.fetch
function authenticate() { __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal' }, expiresAt: Date.now() + 10000, csrfToken: 'csrf', projectId: 'project', authority }) }
test.afterEach(() => { globalThis.fetch = original; __setBrowserSessionStateForTests({ status: 'unauthenticated' }) })
test('schedule client sends real scoped cadence, prompt, and stable run idempotency', async () => {
  authenticate()
  const requests: Array<{ url: string; headers: Headers; body: { action: string; params: Record<string, unknown> } }> = []
  globalThis.fetch = async (input, init) => { requests.push({ url: String(input), headers: new Headers(init?.headers), body: JSON.parse(String(init?.body)) }); return Response.json({ state: 'pending' }) }
  await createTaskSchedule({ schedule_id: 'schedule', name: 'Daily', owner_kind: 'project', owner_id: 'project', project_id: 'project', agent_id: 'agent', input: 'Review errors', armed: true, schedule: { kind: 'weekly', weekdays: [1, 4], hour: 7, minute: 0, timezone: 'America/New_York' } })
  await runTaskSchedule('schedule', 'logical-click')
  assert.deepEqual(requests.map(request => request.url), ['/v1/tasks', '/v1/tasks'])
  assert.equal(requests[0].headers.get('x-labby-project-id'), 'project')
  assert.equal(requests[0].headers.get('x-csrf-token'), 'csrf')
  assert.equal(requests[0].body.action, 'tasks.schedule_create')
  assert.deepEqual(requests[0].body.params.schedule, { kind: 'weekly', weekdays: [1, 4], hour: 7, minute: 0, timezone: 'America/New_York' })
  assert.deepEqual(requests[1].body, { action: 'tasks.schedule_run_now', params: { schedule_id: 'schedule', idempotency_key: 'logical-click' } })
})
test('schedule listing follows sparse authorized pages and does not invent an empty result', async () => {
  authenticate()
  let count = 0
  globalThis.fetch = async () => Response.json(++count === 1 ? { schedules: [], next_cursor: 'hidden-page' } : { schedules: [{ schedule_id: 'visible' }], next_cursor: null })
  assert.deepEqual(await listTaskSchedules(), [{ schedule_id: 'visible' }])
  assert.equal(count, 2)
  globalThis.fetch = async () => Response.json({ message: 'Current membership does not permit this action', kind: 'access_denied' }, { status: 403 })
  await assert.rejects(pauseTaskSchedule('visible'), /Current membership/)
})
test('schedule agent inventory uses the agents dispatch string limit and follows cursors', async () => {
  authenticate()
  const urls: string[] = []
  const bodies: Array<{ action: string; params: Record<string, unknown> }> = []
  globalThis.fetch = async (input, init) => {
    urls.push(String(input))
    const body = JSON.parse(String(init?.body)) as { action: string; params: Record<string, unknown> }
    bodies.push(body)
    return Response.json(body.params.cursor
      ? { agents: [{ agent_id: 'agent-2' }], next_cursor: null }
      : { agents: [{ agent_id: 'agent-1' }], next_cursor: 'agent-1' })
  }
  assert.deepEqual((await listScheduleAgents()).map(agent => agent.agent_id), ['agent-1', 'agent-2'])
  assert.deepEqual(urls, ['/v1/agents', '/v1/agents'])
  assert.deepEqual(bodies, [
    { action: 'agents.list', params: { limit: '100' } },
    { action: 'agents.list', params: { cursor: 'agent-1', limit: '100' } },
  ])
})
test('late schedule result cannot cross a changed authority', async () => {
  authenticate()
  let finish!: (value: Response) => void
  globalThis.fetch = () => new Promise(resolve => { finish = resolve })
  const pending = listTaskSchedules()
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  finish(Response.json({ schedules: [{ schedule_id: 'old-owner' }], next_cursor: null }))
  await assert.rejects(pending, { name: 'AbortError' })
})
