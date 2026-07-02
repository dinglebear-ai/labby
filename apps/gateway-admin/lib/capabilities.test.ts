import assert from 'node:assert/strict'
import test from 'node:test'

import { capabilityAvailable, deriveCapabilities } from './capabilities'

const ALL = [
  { name: 'gateway' },
  { name: 'acp' },
  { name: 'device' },
  { name: 'marketplace' },
]

test('all capabilities available when their services are present', () => {
  const caps = deriveCapabilities(ALL, false)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
  assert.equal(caps.ready, true)
})

test('gateway-only catalog hides gated capabilities', () => {
  const caps = deriveCapabilities([{ name: 'gateway' }, { name: 'doctor' }], false)
  assert.equal(caps.acp, false)
  assert.equal(caps.nodes, false)
  assert.equal(caps.marketplace, false)
  assert.equal(caps.ready, true)
})

test('nodes capability is backed by the "device" service, not "nodes"', () => {
  assert.equal(deriveCapabilities([{ name: 'device' }], false).nodes, true)
  assert.equal(deriveCapabilities([{ name: 'nodes' }], false).nodes, false)
})

test('empty catalog → not ready, fail-open', () => {
  // The catalog SWR hook sets `fallbackData: []`, so on the first render no data
  // has arrived yet and `ready` must be false — `deriveCapabilities` keys off
  // `services.length`, not a loading flag. Guarded surfaces stay fail-open
  // (available) until the catalog resolves so nothing is hidden prematurely.
  const caps = deriveCapabilities([], false)
  assert.equal(caps.ready, false)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
})

test('ready is true once real data has arrived', () => {
  assert.equal(deriveCapabilities([{ name: 'gateway' }], false).ready, true)
})

test('ready is true on catalog error, but capabilities stay fail-open', () => {
  const caps = deriveCapabilities([{ name: 'gateway' }], true)
  assert.equal(caps.ready, true)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
})

test('capabilityAvailable requires the catalog to have confirmed the service', () => {
  const ready = deriveCapabilities(ALL, false)
  assert.equal(capabilityAvailable(ready, 'acp'), true)

  const gatewayOnly = deriveCapabilities([{ name: 'gateway' }], false)
  assert.equal(capabilityAvailable(gatewayOnly, 'acp'), false)

  // While UNRESOLVED (`!ready`) fail-open availability is NOT enough to fetch —
  // capabilityAvailable stays false so consumers hold off until the catalog answers.
  const unresolved = deriveCapabilities([], false)
  assert.equal(unresolved.acp, true)
  assert.equal(capabilityAvailable(unresolved, 'acp'), false)

  // On catalog ERROR the answer is definitive (`ready`) and fail-open, so
  // capabilityAvailable is true: attempt the fetch rather than disable a
  // possibly-present service over a transient catalog error.
  const errored = deriveCapabilities([{ name: 'gateway' }], true)
  assert.equal(errored.acp, true)
  assert.equal(capabilityAvailable(errored, 'acp'), true)
})
