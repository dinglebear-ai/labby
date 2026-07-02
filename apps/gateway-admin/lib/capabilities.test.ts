import assert from 'node:assert/strict'
import test from 'node:test'

import { deriveCapabilities } from './capabilities'

const ALL = [
  { name: 'gateway' },
  { name: 'acp' },
  { name: 'device' },
  { name: 'marketplace' },
]

test('all capabilities available when their services are present', () => {
  const caps = deriveCapabilities(ALL, false, false)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
  assert.equal(caps.ready, true)
})

test('gateway-only catalog hides gated capabilities', () => {
  const caps = deriveCapabilities([{ name: 'gateway' }, { name: 'doctor' }], false, false)
  assert.equal(caps.acp, false)
  assert.equal(caps.nodes, false)
  assert.equal(caps.marketplace, false)
  assert.equal(caps.ready, true)
})

test('nodes capability is backed by the "device" service, not "nodes"', () => {
  assert.equal(deriveCapabilities([{ name: 'device' }], false, false).nodes, true)
  assert.equal(deriveCapabilities([{ name: 'nodes' }], false, false).nodes, false)
})

test('ready is false while the catalog is loading', () => {
  const caps = deriveCapabilities([], true, false)
  assert.equal(caps.ready, false)
  // Fail-open while unresolved.
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
})

test('ready is true once real data has arrived', () => {
  assert.equal(deriveCapabilities([{ name: 'gateway' }], false, false).ready, true)
})

test('ready is true on catalog error, but capabilities stay fail-open', () => {
  const caps = deriveCapabilities([{ name: 'gateway' }], false, true)
  assert.equal(caps.ready, true)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
})

test('fallbackData first render: empty services + isLoading false + no error → not ready, fail-open', () => {
  // The catalog SWR hook sets `fallbackData: []`, so on the very first render
  // `isLoading` is already false while no data has arrived. `ready` must be
  // false here so guarded pages do not render children (and fire /v1 fetches)
  // before the catalog resolves.
  const caps = deriveCapabilities([], false, false)
  assert.equal(caps.ready, false)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
})
