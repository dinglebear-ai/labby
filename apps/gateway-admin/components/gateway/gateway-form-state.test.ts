import test from 'node:test'
import assert from 'node:assert/strict'

import { gatewayFormUiReducer, initialGatewayFormUiState } from './gateway-form-state'

test('reducer bails out for fresh but shallow-equal reset objects', () => {
  const next = gatewayFormUiReducer(initialGatewayFormUiState, {
    type: 'set',
    key: 'errors',
    value: {},
  })
  assert.equal(next, initialGatewayFormUiState)
})
