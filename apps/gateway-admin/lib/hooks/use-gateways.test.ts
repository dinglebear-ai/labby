import test from 'node:test'
import assert from 'node:assert/strict'

import { GATEWAYS_KEY, gatewaysRequestKey } from './use-gateways'

test('gateway loading can be disabled for closed demand-driven dialogs', () => {
  assert.equal(gatewaysRequestKey(false), null)
  assert.equal(gatewaysRequestKey(true), GATEWAYS_KEY)
})
