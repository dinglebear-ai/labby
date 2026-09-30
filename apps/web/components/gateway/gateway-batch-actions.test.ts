import test from 'node:test'
import assert from 'node:assert/strict'
import { gatewayBatchActions } from './gateway-batch-actions'

test('batch state actions use the explicit desired state and exact server id', async () => {
  const calls: string[] = []
  const actions = gatewayBatchActions({ enable: async id => { calls.push(`enable:${id}`) }, disable: async id => { calls.push(`disable:${id}`) }, reload: async () => ({ success: true, message: '' }) })
  assert.deepEqual(await actions.setEnabled({ id: 'one' }, false), { ok: true })
  assert.deepEqual(await actions.setEnabled({ id: 'two' }, true), { ok: true })
  assert.deepEqual(calls, ['disable:one', 'enable:two'])
})
test('batch actions expose failures including an unsuccessful reload response', async () => {
  const fail = async () => { throw new Error('Unavailable') }
  const actions = gatewayBatchActions({ enable: fail, disable: fail, reload: async () => ({ success: false, message: 'Probe failed' }) })
  assert.deepEqual(await actions.setEnabled({ id: 'one' }, true), { ok: false, error: 'Unavailable' })
  assert.deepEqual(await actions.reload({ id: 'one' }), { ok: false, error: 'Probe failed' })
  assert.deepEqual(await gatewayBatchActions({ enable: fail, disable: fail, reload: fail }).reload({ id: 'one' }), { ok: false, error: 'Unavailable' })
})
