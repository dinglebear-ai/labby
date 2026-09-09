import test from 'node:test'
import assert from 'node:assert/strict'
import type { DepotProviderOption } from '@/lib/api/depot-client'
import { discoverSourceSupport } from './discover-source-support'

const provider = (id: string, sourceOrigins?: DepotProviderOption['sourceOrigins'], enabled = true): DepotProviderOption => ({
  id, name: id, enabled, sourceOrigins, health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null },
})

test('unknown capability is distinct from a qualified unsupported filter', () => {
  for (const providers of [[], [provider('team')], [provider('team', null)]]) {
    assert.equal(discoverSourceSupport(providers, 'all', 'ard').state, 'unknown')
  }
  assert.equal(discoverSourceSupport([provider('team', [])], 'all', 'ard').state, 'unavailable')
  assert.equal(discoverSourceSupport([provider('team', ['ard'])], 'all', 'ard').state, 'available')
})

test('all backends use capability intersection, ignoring disabled backends', () => {
  const providers = [provider('team', ['ard']), provider('catalog', ['mcp-registry'])]
  assert.equal(discoverSourceSupport(providers, 'all', 'ard').state, 'unavailable')
  assert.equal(discoverSourceSupport(providers, 'team', 'ard').state, 'available')
  assert.equal(discoverSourceSupport([providers[0], provider('off', [], false)], 'all', 'ard').state, 'available')
  assert.equal(discoverSourceSupport([provider('off', ['ard'], false)], 'off', 'ard').state, 'unavailable')
  assert.equal(discoverSourceSupport(providers, 'missing', 'ard').state, 'unknown')
})
