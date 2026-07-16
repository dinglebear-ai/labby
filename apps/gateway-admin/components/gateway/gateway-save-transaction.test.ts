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
