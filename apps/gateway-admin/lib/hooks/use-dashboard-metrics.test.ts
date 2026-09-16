import test from 'node:test'
import assert from 'node:assert/strict'
import { dashboardMetricsKey } from './use-dashboard-metrics'

test('usage caches cannot share data across authority, API targets or windows', () => {
  const original = dashboardMetricsKey('24h', '/v1', 'principal-a', 7)
  assert.deepEqual(original, ['dashboard-metrics', '24h', '/v1', 'principal-a', 7])
  for (const changed of [
    dashboardMetricsKey('7d', '/v1', 'principal-a', 7),
    dashboardMetricsKey('24h', 'https://remote.example/v1', 'principal-a', 7),
    dashboardMetricsKey('24h', '/v1', 'principal-b', 7),
    dashboardMetricsKey('24h', '/v1', 'principal-a', 8),
  ]) assert.notDeepEqual(original, changed)
})
