import assert from 'node:assert/strict'
import test from 'node:test'

import { gatewayDetailStatus } from './gateway-detail-status'

test('connected servers remain connected when capability discovery needs attention', () => {
  assert.deepEqual(
    gatewayDetailStatus({ enabled: true, connected: true, healthy: false }),
    { label: 'Connected', tone: 'connected' },
  )
})

test('disconnected and disabled servers keep distinct states', () => {
  assert.deepEqual(
    gatewayDetailStatus({ enabled: true, connected: false, healthy: false }),
    { label: 'Disconnected', tone: 'disconnected' },
  )
  assert.deepEqual(
    gatewayDetailStatus({ enabled: false, connected: false, healthy: false }),
    { label: 'Disabled', tone: 'disabled' },
  )
})
