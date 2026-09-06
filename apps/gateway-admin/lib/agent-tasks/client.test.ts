import assert from 'node:assert/strict'
import test from 'node:test'

import { listAgents, listTasks } from './client.ts'

test('agent and task lists use authenticated authoritative action endpoints', async () => {
  const requests: Request[] = []
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    return Response.json(request.url.includes('/agents/') ? { agents: [{ agent_id: 'a-1' }] } : { tasks: [{ task_id: 't-1' }] })
  }
  assert.equal((await listAgents())[0]?.agent_id, 'a-1')
  assert.equal((await listTasks())[0]?.task_id, 't-1')
  assert.deepEqual(requests.map(request => new URL(request.url).pathname), ['/v1/agents/', '/v1/tasks/'])
  assert.ok(requests.every(request => request.method === 'POST' && request.credentials === 'include'))
  assert.deepEqual(JSON.parse(await requests[0]!.text()), { action: 'agents.list', params: {} })
  assert.deepEqual(JSON.parse(await requests[1]!.text()), { action: 'tasks.list', params: {} })
})

test('denials do not become empty authoritative lists', async () => {
  globalThis.fetch = async () => Response.json({ message: 'access denied' }, { status: 403 })
  await assert.rejects(listAgents(), /failed \(403\)/)
  await assert.rejects(listTasks(), /failed \(403\)/)
})
