import test from 'node:test'
import assert from 'node:assert/strict'
import { normalizeGatewayColumns, visibleGatewayColumns, moveGatewayColumn } from './gateway-column-model'

test('persisted column order rejects duplicates unknown and invalid IDs and appends missing', () => {
  assert.deepEqual(normalizeGatewayColumns(['exposed', 'exposed', 1, null, 'unknown']), ['exposed', 'endpoint', 'uptime'])
  for (const value of [null, {}]) assert.deepEqual(normalizeGatewayColumns(value), ['exposed', 'endpoint', 'uptime'])
  assert.deepEqual(normalizeGatewayColumns('endpoint'), ['exposed', 'endpoint', 'uptime'])
  assert.deepEqual(normalizeGatewayColumns(['endpoint']), ['endpoint', 'exposed', 'uptime'])
})
test('responsive hiding never discards persisted column identities', () => {
  const order = normalizeGatewayColumns(['uptime', 'endpoint', 'exposed'])
  assert.deepEqual(visibleGatewayColumns(order, 1179), ['endpoint', 'exposed'])
  assert.deepEqual(visibleGatewayColumns(order, 1180), ['endpoint', 'exposed'])
  assert.deepEqual(visibleGatewayColumns(order, 1400), order)
  const moved = moveGatewayColumn(order, 'exposed', 'endpoint')
  assert.deepEqual(moved, ['uptime', 'exposed', 'endpoint'])
  assert.deepEqual(order, ['uptime', 'endpoint', 'exposed'])
})
