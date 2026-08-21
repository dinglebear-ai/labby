import test from 'node:test'
import assert from 'node:assert/strict'

import { aggregateGatewayUsage, type GatewayUsageMetrics } from './gateway-usage-adapter.ts'

function summary(overrides: Partial<GatewayUsageMetrics> = {}): GatewayUsageMetrics {
  return {
    window_total_calls: 3,
    total_calls: 3,
    error_calls: 1,
    avg_elapsed_ms: 20,
    p50_elapsed_ms: 10,
    p95_elapsed_ms: 40,
    p99_elapsed_ms: 40,
    distinct_tools: 1,
    distinct_actors: 2,
    peak_per_min: 2,
    top_tools: [{ upstream: 'github', tool: 'search', calls: 3, failed: 1 }],
    least_tools: [{ upstream: 'github', tool: 'search', calls: 3, failed: 1 }],
    top_actors: [
      { actor: 'codex', calls: 2 },
      { actor: 'unattributed', calls: 1 },
    ],
    slowest_tools: [
      { upstream: 'github', tool: 'search', avg_elapsed_ms: 20 },
    ],
    errors: [{ kind: 'timeout', calls: 1 }],
    upstreams: [{ upstream: 'github', calls: 3, failed: 1 }],
    hourly: Array.from({ length: 24 }, (_, hour) => ({ hour, calls: hour === 12 ? 3 : 0 })),
    timeseries: [
      { ts_unix: 1_799_996_400, calls: 1, failed: 0 },
      { ts_unix: 1_800_000_000, calls: 2, failed: 1 },
    ],
    facets: {
      tools: [{ upstream: 'github', tool: 'search' }],
      actors: ['codex', 'unattributed'],
      upstreams: ['github'],
      outcomes: ['ok', 'timeout'],
    },
    ...overrides,
  }
}

test('adapts complete-window gateway aggregates into truthful dashboard metrics', () => {
  const metrics = aggregateGatewayUsage('24h', 1_800_000_000_000, summary())

  assert.deepEqual(metrics.tool_calls, { total: 3, failed: 1, succeeded: 2 })
  assert.equal(metrics.tools.top[0].name, 'github::search')
  assert.equal(metrics.tools.top[0].failed, 1)
  assert.equal(metrics.tools.distinct, 1)
  assert.equal(metrics.actors.agent.active, 2)
  assert.equal(metrics.latency.p50, 10)
  assert.equal(metrics.latency.p95, 40)
  assert.equal(metrics.latency.avg, 20)
  assert.deepEqual(metrics.errors.by_kind, [{ kind: 'timeout', count: 1 }])
  assert.deepEqual(metrics.upstreams, [{ name: 'github', calls: 3, failed: 1 }])
  assert.equal(metrics.actors.agent.top[0].id, 'codex')
  assert.deepEqual(metrics.timeseries, [
    { ts: 1_799_996_400_000, calls: 1, failed: 0 },
    { ts: 1_800_000_000_000, calls: 2, failed: 1 },
  ])
  assert.equal(metrics.collected.complete_window_analytics, true)
})

test('keeps dimensional top-target rows distinct with exact failure counts', () => {
  const metrics = aggregateGatewayUsage('24h', 1_800_000_000_000, summary({
    top_tools: [
      { upstream: 'github', tool: 'search', capability: 'tools', operation: 'tool.call', subject_scoped: false, calls: 2, failed: 1 },
      { upstream: 'github', tool: 'search', capability: 'tools', operation: 'tool.call', subject_scoped: true, calls: 1, failed: 0 },
    ],
  }))

  assert.deepEqual(metrics.tools.top, [
    {
      name: 'github::search',
      id: 'github\u0000search\u0000tools\u0000tool.call\u0000shared',
      label: 'github::search',
      calls: 2,
      failed: 1,
    },
    {
      name: 'github::search',
      id: 'github\u0000search\u0000tools\u0000tool.call\u0000subject',
      label: 'github::search · OAuth',
      calls: 1,
      failed: 0,
    },
  ])
})

test('uses backend-provided full-window buckets rather than raw call-page sampling', () => {
  const buckets = Array.from({ length: 24 }, (_, index) => ({
    ts_unix: 1_800_000_000 - (23 - index) * 3600,
    calls: index === 0 ? 4_000 : index === 23 ? 1_000 : 0,
    failed: 0,
  }))
  const metrics = aggregateGatewayUsage('24h', 1_800_000_000_000, summary({
    window_total_calls: 5_000,
    total_calls: 5_000,
    error_calls: 0,
    timeseries: buckets,
  }))

  assert.equal(metrics.tool_calls.total, 5_000)
  assert.equal(metrics.timeseries.length, 24)
  assert.equal(metrics.timeseries[0].calls, 4_000)
  assert.equal(metrics.timeseries[23].calls, 1_000)
})
