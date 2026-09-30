import test from 'node:test'
import assert from 'node:assert/strict'

import {
  getMockGatewayFallback,
  getMockGatewaysFallback,
  getMockServiceActionsFallback,
  getMockServiceConfigFallback,
  getMockSupportedServicesFallback,
} from './mock-fallback.ts'

test('mock gateway fallback seeds the UI with populated gateway data', () => {
  const gateways = getMockGatewaysFallback()

  assert.equal(gateways.length, 5)
  assert.equal(gateways[0]?.name, 'filesystem-server')

  gateways[0]!.name = 'mutated'

  const freshGateways = getMockGatewaysFallback()
  assert.equal(freshGateways[0]?.name, 'filesystem-server')
})

test('mock gateway fallback can resolve a single gateway by id', () => {
  const gateway = getMockGatewayFallback('gw-2')

  assert.ok(gateway)
  assert.equal(gateway?.name, 'github-server')
})

test('mock service fallbacks expose config and actions for the add gateway flow', () => {
  const services = getMockSupportedServicesFallback()
  const config = getMockServiceConfigFallback('github')
  const actions = getMockServiceActionsFallback('github')

  assert.equal(services.length, 4)
  assert.equal(config.service, 'github')
  assert.ok(config.fields.length > 0)
  assert.ok(actions.some((action) => action.name === 'list_issues'))
})
