import test from 'node:test'
import assert from 'node:assert/strict'
import { enrichDashboardWithObservability } from './dashboard-observability.ts'
import { aggregateGatewayUsage } from '../dashboard/gateway-usage-adapter.ts'
import type { ServerLogEntry } from '../types/traces.ts'

const now = Date.parse('2026-08-17T12:00:00.000Z')
const base = aggregateGatewayUsage('24h', now, {
  window_total_calls: 2,
  total_calls: 2,
  error_calls: 0,
  avg_elapsed_ms: 10,
  p50_elapsed_ms: 10,
  p95_elapsed_ms: 10,
  p99_elapsed_ms: 10,
  distinct_tools: 0,
  distinct_actors: 0,
  peak_per_min: 0,
  top_tools: [],
  least_tools: [],
  top_actors: [],
  slowest_tools: [],
  errors: [],
  upstreams: [],
  hourly: [],
  timeseries: [],
  facets: { tools: [], actors: [], upstreams: [], outcomes: [] },
})

function entry(message: string, fields: Record<string, unknown>, timestamp = now - 1000): ServerLogEntry {
  return {
    timestamp: new Date(timestamp).toISOString(),
    level: 'INFO',
    target: 'labby',
    message,
    service: typeof fields.service === 'string' ? fields.service : null,
    action: typeof fields.action === 'string' ? fields.action : null,
    kind: typeof fields.kind === 'string' ? fields.kind : null,
    file: 'labby.jsonl',
    fields,
  }
}

test('derives surfaces and estimated token attribution from terminal dispatch events', () => {
  const result = enrichDashboardWithObservability(base, [
    entry('dispatch start', { surface: 'api', service: 'gateway', action: 'gateway.list', input_tokens: 999 }),
    entry('dispatch ok', { surface: 'api', service: 'gateway', action: 'gateway.list', input_tokens: 10, output_tokens: 20 }),
    entry('upstream dispatch ok', { surface: 'mcp', service: 'github', action: 'search', tool: 'github::search', input_tokens: 4, output_tokens: 6 }),
    entry('dispatch ok', { surface: 'api', service: 'server_logs', action: 'server_logs.query', input_tokens: 100, output_tokens: 100 }),
    entry('dispatch ok', { surface: 'api', service: 'old', action: 'ignored', input_tokens: 500 }, now - 25 * 60 * 60 * 1000),
  ], '24h', now)

  assert.deepEqual(result.surfaces, [{ surface: 'api', calls: 1 }, { surface: 'mcp', calls: 1 }])
  assert.deepEqual(result.tokens, { input: 14, output: 26, total: 40, avg_per_call: 20 })
  assert.deepEqual(result.tokens_by_tool, [
    { name: 'gateway::gateway.list', tokens: 30 },
    { name: 'github::search', tokens: 10 },
  ])
  assert.equal(result.collected.surfaces, true)
  assert.equal(result.collected.tokens, true)
})

test('derives Code Mode fan-out rates and artifacts from completion events', () => {
  const result = enrichDashboardWithObservability(base, [
    entry('gateway codemode ok', { call_count: 3, artifact_writes: 2, truncated: true }),
    entry('gateway codemode failed', { call_count: 1, artifact_writes: 0, kind: 'timeout' }),
    entry('gateway codemode start', { call_count: 99 }),
  ], '24h', now)

  assert.deepEqual(result.fan_out, {
    runs: 2,
    total_calls: 4,
    avg_calls_per_run: 2,
    max_calls_in_run: 3,
    timeout_rate: 0.5,
    truncation_rate: 0.5,
    artifact_writes: 2,
  })
  assert.equal(result.collected.fan_out, true)
})

test('a successful empty retained sample is collected with zero-valued panels', () => {
  const result = enrichDashboardWithObservability(base, [], '24h', now)
  assert.equal(result.collected.surfaces, true)
  assert.equal(result.collected.tokens, true)
  assert.equal(result.collected.fan_out, true)
  assert.deepEqual(result.surfaces, [])
  assert.deepEqual(result.tokens_by_tool, [])
  assert.equal(result.fan_out.runs, 0)
})
