import test from 'node:test'
import assert from 'node:assert/strict'

import {
  GatewaySaveCompensationError,
  runGatewaySaveTransaction,
} from './gateway-save-transaction'

test('route failure compensates the completed gateway write', async () => {
  const events: string[] = []
  await assert.rejects(
    runGatewaySaveTransaction(
      async () => async () => { events.push('rollback') },
      async () => { events.push('route'); throw new Error('route failed') },
    ),
    /route failed/,
  )
  assert.deepEqual(events, ['route', 'rollback'])
})

test('rollback failure is surfaced distinctly for operator recovery', async () => {
  await assert.rejects(
    runGatewaySaveTransaction(
      async () => async () => { throw new Error('rollback failed') },
      async () => { throw new Error('route failed') },
    ),
    GatewaySaveCompensationError,
  )
})

test('identity navigation commits only after the protected route succeeds', async () => {
  const events: string[] = []
  await runGatewaySaveTransaction(
    async () => ({ rollback: async () => { events.push('rollback new-id') }, commit: () => { events.push('navigate new-id') } }),
    async () => { events.push('route') },
  )
  assert.deepEqual(events, ['route', 'navigate new-id'])
  events.length = 0
  await assert.rejects(runGatewaySaveTransaction(
    async () => ({ rollback: async () => { events.push('rollback new-id') }, commit: () => { events.push('navigate new-id') } }),
    async () => { throw new Error('route failed') },
  ), /route failed/)
  assert.deepEqual(events, ['rollback new-id'])
})
