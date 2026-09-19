import test from 'node:test'
import assert from 'node:assert/strict'

import {
  describeGatewayOperationalState,
  gatewayNeedsAttention,
} from './gateway-operational-state'

const base = {
  enabled: true,
  status: {
    connected: true,
    healthy: true,
    discovered_tool_count: 1,
    exposed_tool_count: 1,
    discovered_resource_count: 0,
    exposed_resource_count: 0,
    discovered_prompt_count: 0,
    exposed_prompt_count: 0,
  },
  warnings: [],
}

test('connected warning state stays connected while requiring attention', () => {
  const state = describeGatewayOperationalState({
    ...base,
    status: { ...base.status, healthy: false },
    warnings: [{ code: 'prompts_unavailable', message: 'Prompts timed out' }],
  })

  assert.equal(state.kind, 'degraded')
  assert.equal(state.connectionLabel, 'Connected')
  assert.equal(state.label, 'Needs attention')
  assert.equal(state.needsAttention, true)
  assert.equal(state.reason, 'Prompts timed out')
})

test('connected last error remains degraded even if a stale healthy flag is true', () => {
  const state = describeGatewayOperationalState({
    ...base,
    status: { ...base.status, healthy: true, last_error: 'Runtime probe failed' },
    warnings: [],
  })

  assert.equal(state.kind, 'degraded')
  assert.equal(state.connectionLabel, 'Connected')
  assert.equal(state.label, 'Needs attention')
  assert.equal(state.reason, 'Runtime probe failed')
})

test('catalog warming is connected discovery work, not operator attention', () => {
  const state = describeGatewayOperationalState({
    ...base,
    status: { ...base.status, healthy: false, catalog_warming: true },
  })

  assert.equal(state.kind, 'discovering')
  assert.equal(state.connectionLabel, 'Connected')
  assert.equal(state.label, 'Discovering')
  assert.equal(state.needsAttention, false)
})

test('stale runtime state is consistently treated as attention', () => {
  const gateway = {
    ...base,
    status: { ...base.status, likely_stale_count: 2 },
  }

  const state = describeGatewayOperationalState(gateway)
  assert.equal(state.kind, 'degraded')
  assert.equal(state.connectionLabel, 'Connected')
  assert.equal(state.needsAttention, true)
  assert.match(state.reason, /2 likely stale runtime processes/)
  assert.equal(gatewayNeedsAttention(gateway), true)
})

test('disconnected and disabled remain distinct from degraded connectivity', () => {
  const disconnected = describeGatewayOperationalState({
    ...base,
    status: { ...base.status, connected: false, healthy: false, last_error: 'SSH unavailable' },
  })
  assert.equal(disconnected.kind, 'disconnected')
  assert.equal(disconnected.connectionLabel, 'Disconnected')
  assert.equal(disconnected.needsAttention, true)
  assert.equal(disconnected.reason, 'SSH unavailable')

  const disabled = describeGatewayOperationalState({
    ...base,
    enabled: false,
    status: { ...base.status, connected: false, healthy: false },
  })
  assert.equal(disabled.kind, 'disabled')
  assert.equal(disabled.connectionLabel, 'Disabled')
  assert.equal(disabled.needsAttention, false)
})

test('healthy connected state remains clean', () => {
  const state = describeGatewayOperationalState(base)
  assert.equal(state.kind, 'healthy')
  assert.equal(state.connectionLabel, 'Connected')
  assert.equal(state.label, 'Healthy')
  assert.equal(state.needsAttention, false)
})
