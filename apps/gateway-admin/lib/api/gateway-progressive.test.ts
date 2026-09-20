import test from 'node:test'
import assert from 'node:assert/strict'

import { loadGatewayConfiguration, loadGatewayRuntime, loadGatewayToolInventory } from './gateway-progressive.ts'

test('gateway configuration can resolve without waiting for runtime hydration', async () => {
  let runtimeRequested = false
  const api = {
    list: async () => [{ id: 'one' }],
    hydrateRuntime: async () => {
      runtimeRequested = true
      return [{ id: 'one', status: 'connected' }]
    },
    hydrateToolInventory: async () => [{ id: 'one', tools: ['search'] }],
  }

  const configured = await loadGatewayConfiguration(api)

  assert.deepEqual(configured, [{ id: 'one' }])
  assert.equal(runtimeRequested, false)
  assert.deepEqual(await loadGatewayRuntime(api, configured), [
    { id: 'one', status: 'connected' },
  ])
  assert.deepEqual(await loadGatewayToolInventory(api, configured), [
    { id: 'one', tools: ['search'] },
  ])
})
