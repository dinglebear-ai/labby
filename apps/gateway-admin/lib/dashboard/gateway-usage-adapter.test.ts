import test from 'node:test'
import assert from 'node:assert/strict'

import { aggregateGatewayUsage } from './gateway-usage-adapter.ts'

test('adapts persisted gateway usage into truthful dashboard metrics', () => {
  const now = 1_800_000_000_000
  const metrics = aggregateGatewayUsage(
    '24h',
    now,
    {
      total_calls: 3,
      error_calls: 1,
      avg_elapsed_ms: 20,
      top_tools: [
        { upstream: 'github', tool: 'search', calls: 3 },
      ],
      top_actors: [
        { actor: 'codex', calls: 2 },
        { actor: 'unattributed', calls: 1 },
      ],
    },
    {
      calls: [
        { ts_unix: 1_800_000_000, upstream: 'github', tool: 'search', actor: 'codex', outcome: 'ok', elapsed_ms: 10 },
        { ts_unix: 1_799_999_940, upstream: 'github', tool: 'search', actor: 'codex', outcome: 'timeout', elapsed_ms: 40 },
        { ts_unix: 1_799_999_880, upstream: 'github', tool: 'search', actor: 'unattributed', outcome: 'ok', elapsed_ms: 10 },
      ],
      total_matching: 3,
      next_cursor: null,
    },
  )

  assert.deepEqual(metrics.tool_calls, { total: 3, failed: 1, succeeded: 2 })
  assert.equal(metrics.tools.top[0].name, 'github::search')
  assert.equal(metrics.tools.top[0].failed, 1)
  assert.equal(metrics.latency.avg, 20)
  assert.deepEqual(metrics.errors.by_kind, [{ kind: 'timeout', count: 1 }])
  assert.deepEqual(metrics.upstreams, [{ name: 'github', calls: 3, failed: 1 }])
  assert.equal(metrics.actors.agent.top[0].id, 'codex')
  assert.deepEqual(metrics.collected, {
    tokens: false,
    surfaces: false,
    fan_out: false,
    actor_kinds: false,
    complete_call_rows: true,
  })
})

test('keeps dimensional top-tool rows distinct and attributes failures to the matching dimension', () => {
  const metrics = aggregateGatewayUsage(
    '24h',
    1_800_000_000_000,
    {
      total_calls: 3,
      error_calls: 1,
      avg_elapsed_ms: 10,
      top_tools: [
        { upstream: 'github', tool: 'search', capability: 'tools', operation: 'tool.call', subject_scoped: false, calls: 2 },
        { upstream: 'github', tool: 'search', capability: 'tools', operation: 'tool.call', subject_scoped: true, calls: 1 },
      ],
      top_actors: [],
    },
    {
      calls: [
        { ts_unix: 1_800_000_000, upstream: 'github', tool: 'search', capability: 'tools', operation: 'tool.call', subject_scoped: false, actor: 'codex', outcome: 'timeout', elapsed_ms: 10 },
        { ts_unix: 1_799_999_990, upstream: 'github', tool: 'search', capability: 'tools', operation: 'tool.call', subject_scoped: false, actor: 'codex', outcome: 'ok', elapsed_ms: 10 },
        { ts_unix: 1_799_999_980, upstream: 'github', tool: 'search', capability: 'tools', operation: 'tool.call', subject_scoped: true, actor: 'codex', outcome: 'ok', elapsed_ms: 10 },
      ],
      total_matching: 3,
      next_cursor: null,
    },
  )

  assert.deepEqual(metrics.tools.top, [
    { name: 'github::search', calls: 2, failed: 1 },
    { name: 'github::search · OAuth', calls: 1, failed: 0 },
  ])
})

test('marks row-derived panels sampled when the call page has more rows', () => {
  const metrics = aggregateGatewayUsage(
    '7d',
    1_800_000_000_000,
    {
      total_calls: 5_000,
      error_calls: 2,
      avg_elapsed_ms: 4,
      top_tools: [],
      top_actors: [],
    },
    { calls: [], total_matching: 5_000, next_cursor: 'cursor' },
  )

  assert.equal(metrics.collected.complete_call_rows, false)
  assert.equal(metrics.tool_calls.total, 5_000)
})
