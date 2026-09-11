import test from 'node:test'
import assert from 'node:assert/strict'
import type { Gateway } from '@/lib/types/gateway'
import { runGatewayBatch } from './gateway-selection-model'

const make = (id: string, enabled = true) => ({ id, name: id, enabled, transport: 'http' }) as Gateway
test('sequential batch revalidates state and identity after each awaited operation', async () => {
  let current = [make('a'), make('b'), make('c')]
  const sent: string[] = []
  const reports = await runGatewayBatch('disable', current, () => current, { onBatchSetEnabled: async (gateway, enabled) => {
    assert.equal(enabled, false)
    sent.push(gateway.id)
    current = [make('a', false), make('b', false)]
    return { ok: true }
  } }, () => true)
  assert.deepEqual(sent, ['a'])
  assert.deepEqual(reports.map(report => report.outcome), ['completed', 'skipped', 'skipped'])
})
test('failures remain per-target and session changes prevent subsequent sends', async () => {
  const current = [make('a'), make('b'), make('c')]
  let valid = true
  const sent: string[] = []
  const reports = await runGatewayBatch('reload', current, () => current, { onBatchReload: async gateway => {
    sent.push(gateway.id)
    if (gateway.id === 'a') throw new Error('denied')
    valid = false
    return { ok: false, error: 'reload rejected' }
  } }, () => valid)
  assert.deepEqual(sent, ['a', 'b'])
  assert.deepEqual(reports.map(report => report.outcome), ['failed', 'failed', 'failed'])
  assert.match(reports[2].detail!, /Session changed/)
})
test('renamed targets and unsupported reloads never dispatch', async () => {
  const targets = [make('a'), make('b')]
  const current = [{ ...make('a'), name: 'replacement' }, { ...make('b'), transport: 'in_process' }] as Gateway[]
  let calls = 0
  const reports = await runGatewayBatch('reload', targets, () => current, { onBatchReload: async () => { calls++; return { ok: true } } }, () => true)
  assert.equal(calls, 0)
  assert.ok(reports.every(report => report.outcome === 'skipped'))
})
