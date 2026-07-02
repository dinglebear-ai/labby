import assert from 'node:assert/strict'
import test from 'node:test'

import { deriveCapabilities } from './capabilities'

const ALL = [
  { name: 'gateway' },
  { name: 'acp' },
  { name: 'device' },
  { name: 'marketplace' },
  { name: 'stash' },
]

test('all capabilities available when their services are present', () => {
  const caps = deriveCapabilities(ALL, false, false)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
  assert.equal(caps.stash, true)
  assert.equal(caps.isLoading, false)
})

test('gateway-only catalog hides gated capabilities', () => {
  const caps = deriveCapabilities([{ name: 'gateway' }, { name: 'doctor' }], false, false)
  assert.equal(caps.acp, false)
  assert.equal(caps.nodes, false)
  assert.equal(caps.marketplace, false)
  assert.equal(caps.stash, false)
})

test('nodes capability is backed by the "device" service, not "nodes"', () => {
  assert.equal(deriveCapabilities([{ name: 'device' }], false, false).nodes, true)
  assert.equal(deriveCapabilities([{ name: 'nodes' }], false, false).nodes, false)
})

test('fail-open while loading', () => {
  const caps = deriveCapabilities([], true, false)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
  assert.equal(caps.marketplace, true)
  assert.equal(caps.stash, true)
  assert.equal(caps.isLoading, true)
})

test('fail-open on catalog error', () => {
  const caps = deriveCapabilities([{ name: 'gateway' }], false, true)
  assert.equal(caps.acp, true)
  assert.equal(caps.marketplace, true)
})

test('fail-open on empty catalog (no confident data)', () => {
  const caps = deriveCapabilities([], false, false)
  assert.equal(caps.acp, true)
  assert.equal(caps.nodes, true)
})
