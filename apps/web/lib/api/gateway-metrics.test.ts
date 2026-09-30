import test from 'node:test'
import assert from 'node:assert/strict'

import { buildGatewayMetricItems } from './gateway-metrics.ts'

test('buildGatewayMetricItems returns discovered and exposed counts by default', () => {
  assert.deepEqual(buildGatewayMetricItems(30, 30, false), [
    { key: 'discovered', label: 'tools', value: 30, tone: 'muted' },
    { key: 'exposed', label: 'exposed', value: 30, tone: 'success' },
  ])
})

test('buildGatewayMetricItems includes filtered count when tools are hidden by policy', () => {
  assert.deepEqual(buildGatewayMetricItems(30, 12, true), [
    { key: 'discovered', label: 'tools', value: 30, tone: 'muted' },
    { key: 'exposed', label: 'exposed', value: 12, tone: 'success' },
    { key: 'filtered', label: 'filtered', value: 18, tone: 'muted' },
  ])
})
