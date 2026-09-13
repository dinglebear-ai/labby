import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act, StrictMode } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import type { TaskSchedule } from '@/lib/agent-tasks/schedules'
installTestDom()
let TaskScheduleRows: typeof import('./task-schedules-page').TaskScheduleRows
let TaskWorkspace: typeof import('./task-schedules-page').TaskWorkspace
let newScheduleDraft: typeof import('./task-schedule-form').newScheduleDraft
let validateRetryPolicy: typeof import('./task-schedule-form').validateRetryPolicy
let validateScheduleDraft: typeof import('./task-schedule-form').validateScheduleDraft
test.before(async () => { ({ TaskScheduleRows, TaskWorkspace } = await import('./task-schedules-page')); ({ newScheduleDraft, validateScheduleDraft, validateRetryPolicy } = await import('./task-schedule-form')) })
const row: TaskSchedule = { schedule_id: 'daily', name: 'Daily review', owner_kind: 'personal', owner_id: 'principal', agent_id: 'agent', schedule: { kind: 'daily', hour: 2, minute: 0, timezone: 'America/New_York' }, armed: true, next_run_at: 1893456000000, revision: 1, last_task_id: null, last_error_kind: null, missed_run_policy: 'coalesce_one', dst_policy: 'skip_nonexistent_repeat_ambiguous' }
const agent = { agent_id: 'agent', owner_kind: 'personal', owner_id: 'principal', state: 'active', version: 1, catalog_generation: 'generation', content_digest: 'sha256:content', repository_digest: 'sha256:repository', image_digest: 'sha256:image', harness_digest: 'sha256:harness', loadout_digest: 'sha256:loadout' }
const original = globalThis.fetch
function authenticate() { __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal' }, expiresAt: Date.now() + 100000, csrfToken: 'csrf', authority: { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal', organizationId: 'org', activeOwner: { kind: 'personal', id: 'principal' }, teams: [], projects: [], capabilities: ['scope.read', 'scope.create', 'scope.operate', 'scope.delete'], generation: 1 } }) }
test.afterEach(() => { globalThis.fetch = original; __setBrowserSessionStateForTests({ status: 'unauthenticated' }) })
async function waitFor(check: () => void) { let error: unknown; for (let i = 0; i < 100; i++) { try { check(); return } catch (reason) { error = reason } await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) }) } throw error }
test('cadence form preserves timezone and validates useful bounded input', () => {
  const draft = { ...newScheduleDraft(), name: 'Weekly review', agentId: 'agent', input: 'Review', kind: 'weekly' as const, weekdays: [4, 1], time: '07:00', timezone: 'America/New_York' }
  assert.deepEqual(validateScheduleDraft(draft), { kind: 'weekly', weekdays: [1, 4], hour: 7, minute: 0, timezone: 'America/New_York' })
  assert.throws(() => validateScheduleDraft({ ...draft, weekdays: [] }), /at least one weekday/)
  assert.throws(() => validateScheduleDraft({ ...draft, timezone: 'Invented/Zone' }), /IANA timezone/)
  assert.throws(() => validateScheduleDraft({ ...draft, kind: 'once', date: '2000-01-01T00:00' }), /future date/)
  assert.throws(() => validateScheduleDraft({ ...draft, kind: 'interval', intervalMinutes: '0' }), /interval/)
})
test('schedule rows expose real action callbacks without optimistic switch success', async () => {
  const operations: string[] = []
  const view = await renderClient(<TaskScheduleRows rows={[row]} agents={[agent]} states={{}} canOperate canDelete onToggle={() => operations.push('pause')} onRun={() => operations.push('run')} onEdit={() => operations.push('edit')} onDelete={() => operations.push('delete')} />)
  try {
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Pause Daily review"]')!.click())
    assert.equal(document.querySelector('[aria-label="Pause Daily review"]')!.getAttribute('aria-checked'), 'true')
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Run Daily review now"]')!.click())
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Inspect Daily review"]')!.click())
    assert.match(view.container.textContent ?? '', /America\/New_York/)
    assert.match(view.container.textContent ?? '', /No automatic retries/)
    assert.doesNotMatch(view.container.textContent ?? '', /platform-base|passed|2 · backoff/)
    await act(async () => [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Edit')!.click())
    await act(async () => [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Delete')!.click())
    assert.deepEqual(operations, ['pause', 'run', 'edit', 'delete'])
  } finally { await view.unmount() }
})
test('task workspace survives strict effect restart and retains denied mutation state', async () => {
  authenticate()
  let pauseCalls = 0
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body))
    if (body.action === 'agents.list') return Response.json({ agents: [agent] })
    if (body.action === 'tasks.schedule_list') return Response.json({ schedules: [row] })
    if (body.action === 'tasks.schedule_pause') { pauseCalls++; return Response.json({ kind: 'access_denied', message: 'Only the schedule creator may pause it.' }, { status: 403 }) }
    throw new Error(`Unexpected action ${body.action}`)
  }
  const view = await renderClient(<StrictMode><TaskWorkspace owner={{ kind: 'personal', id: 'principal' }} capabilities={['scope.read', 'scope.create', 'scope.operate']} /></StrictMode>)
  try {
    await waitFor(() => assert.ok(view.container.querySelector('[aria-label="Pause Daily review"]')))
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Pause Daily review"]')!.click())
    await waitFor(() => assert.match(view.container.textContent ?? '', /Only the schedule creator/))
    assert.equal(pauseCalls, 1)
    assert.equal(view.container.querySelector('[aria-label="Pause Daily review"]')!.getAttribute('aria-checked'), 'true')
  } finally { await view.unmount() }
})

test('uncertain Run Now retry preserves its idempotency key and reports pending truthfully', async () => {
  authenticate()
  const keys: string[] = []
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body))
    if (body.action === 'agents.list') return Response.json({ agents: [agent] })
    if (body.action === 'tasks.schedule_list') return Response.json({ schedules: [row] })
    if (body.action === 'tasks.schedule_run_now') { keys.push(body.params.idempotency_key); if (keys.length === 1) throw new Error('Connection interrupted'); return Response.json({ state: 'pending' }) }
    throw new Error(`Unexpected action ${body.action}`)
  }
  const view = await renderClient(<TaskWorkspace owner={{ kind: 'personal', id: 'principal' }} capabilities={['scope.read', 'scope.operate']} />)
  try {
    await waitFor(() => assert.ok(view.container.querySelector('[aria-label="Run Daily review now"]')))
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Run Daily review now"]')!.click())
    await waitFor(() => assert.match(view.container.textContent ?? '', /Connection interrupted/))
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Run Daily review now"]')!.click())
    await waitFor(() => assert.match(view.container.textContent ?? '', /Run accepted by the scheduler/))
    assert.equal(keys.length, 2)
    assert.equal(keys[0], keys[1])
    assert.doesNotMatch(view.container.textContent ?? '', /Task is running|Task completed/)
  } finally { await view.unmount() }
})

test('delete waits for server confirmation and then reloads authoritative schedule inventory', async () => {
  authenticate()
  let deleted = false
  let deleteCalls = 0
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body))
    if (body.action === 'agents.list') return Response.json({ agents: [agent] })
    if (body.action === 'tasks.schedule_list') return Response.json({ schedules: deleted ? [] : [row] })
    if (body.action === 'tasks.schedule_delete') { assert.equal(body.params.schedule_id, 'daily'); deleteCalls++; deleted = true; return Response.json({ deleted: true }) }
    throw new Error(`Unexpected action ${body.action}`)
  }
  const view = await renderClient(<TaskWorkspace owner={{ kind: 'personal', id: 'principal' }} capabilities={['scope.read', 'scope.operate', 'scope.delete']} />)
  try {
    await waitFor(() => assert.ok(view.container.querySelector('[aria-label="Inspect Daily review"]')))
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Inspect Daily review"]')!.click())
    await act(async () => [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Delete')!.click())
    assert.equal(deleteCalls, 0)
    await act(async () => [...document.querySelectorAll('button')].find(button => button.textContent === 'Delete schedule')!.click())
    await waitFor(() => assert.match(view.container.textContent ?? '', /No scheduled tasks/))
    assert.equal(deleteCalls, 1)
  } finally { await view.unmount() }
})

test('retries require explicit opt-in and preserve bounded server policy', () => {
  const draft = newScheduleDraft()
  assert.deepEqual(validateRetryPolicy(draft), { max_retries: 0, backoff_ms: 300000 })
  assert.deepEqual(validateRetryPolicy({ ...draft, retryEnabled: true }), { max_retries: 2, backoff_ms: 300000 })
  assert.throws(() => validateRetryPolicy({ ...draft, retryEnabled: true, maxRetries: '11' }), /1–10/)
  assert.throws(() => validateRetryPolicy({ ...draft, retryEnabled: true, backoffMinutes: '0' }), /retry delay/)
  const existing = newScheduleDraft({ ...row, retry_policy: { max_retries: 3, backoff_ms: 120000 }, task_template: { owner_kind: 'personal', owner_id: 'principal', agent_id: 'agent', input: 'Review' } })
  assert.deepEqual(validateRetryPolicy(existing), { max_retries: 3, backoff_ms: 120000 })
})
