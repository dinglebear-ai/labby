import test from 'node:test'
import assert from 'node:assert/strict'

import { buildTraceSummary } from './trace-model.ts'
import type { ServerLogEntry } from '@/lib/types/traces'

function entry(
  offsetMs: number,
  requestId: string | null,
  event: string,
  fields: Record<string, unknown> = {},
  overrides: Partial<ServerLogEntry> = {},
): ServerLogEntry {
  return {
    timestamp: new Date(1_800_000_000_000 + offsetMs).toISOString(),
    level: event === 'error' ? 'WARN' : 'INFO',
    target: 'labby::observability',
    message: event,
    service: 'gateway',
    action: 'tool.call',
    kind: event === 'error' ? 'timeout' : null,
    file: 'labby.jsonl',
    fields: {
      event,
      ...(requestId ? { request_id: requestId } : {}),
      ...fields,
    },
    ...overrides,
  }
}

test('correlates root request events with child upstream spans', () => {
  const summary = buildTraceSummary([
    entry(0, 'req-ok', 'start', { surface: 'mcp', actor_key: 'actor-1' }),
    entry(40, 'req-ok', 'finish', {
      surface: 'dispatch',
      upstream: 'github',
      elapsed_ms: 40,
      response_bytes: 2048,
    }, { service: 'upstream.pool', action: 'upstream.request' }),
    entry(60, 'req-ok', 'finish', { surface: 'mcp', elapsed_ms: 60 }),
    entry(100, 'req-slow', 'start', { surface: 'api' }),
    entry(250, 'req-slow', 'error', {
      surface: 'dispatch',
      upstream: 'slack',
      elapsed_ms: 150,
      kind: 'timeout',
    }, { service: 'upstream.pool', action: 'upstream.request' }),
    entry(300, 'req-slow', 'error', {
      surface: 'api',
      elapsed_ms: 200,
      input_tokens: 10,
      output_tokens: 30,
      kind: 'timeout',
    }),
    entry(400, null, 'start', { surface: 'internal' }),
  ])

  assert.equal(summary.total, 3)
  assert.equal(summary.failed, 1)
  assert.equal(summary.incomplete, 1)
  assert.equal(summary.p50_ms, 60)
  assert.equal(summary.p95_ms, 200)
  assert.deepEqual(summary.upstreams, [
    { name: 'github', count: 1 },
    { name: 'slack', count: 1 },
  ])

  const ok = summary.traces.find((trace) => trace.id === 'req-ok')
  assert.equal(ok?.events.length, 3)
  assert.equal(ok?.response_bytes, 2048)
  assert.equal(ok?.actor_key, 'actor-1')
  assert.equal(ok?.outcome, 'ok')

  const failed = summary.traces.find((trace) => trace.id === 'req-slow')
  assert.equal(failed?.outcome, 'failed')
  assert.equal(failed?.error_kind, 'timeout')
  assert.equal((failed?.input_tokens ?? 0) + (failed?.output_tokens ?? 0), 40)
})

test('child completion cannot complete a request without a root terminal event', () => {
  const summary = buildTraceSummary([
    entry(0, 'req-partial', 'start', { surface: 'mcp' }),
    entry(30, 'req-partial', 'finish', { surface: 'dispatch', upstream: 'github', elapsed_ms: 30 }, {
      service: 'upstream.pool',
      action: 'upstream.request',
    }),
  ])
  assert.equal(summary.traces[0]?.outcome, 'incomplete')
})

test('recoverable child warning does not fail a successful root request', () => {
  const summary = buildTraceSummary([
    entry(0, 'req-recovered', 'start', { surface: 'mcp' }),
    entry(20, 'req-recovered', 'error', { surface: 'dispatch', upstream: 'github', kind: 'timeout' }, {
      service: 'upstream.pool',
      action: 'upstream.request',
      level: 'WARN',
    }),
    entry(50, 'req-recovered', 'finish', { surface: 'mcp', elapsed_ms: 50 }),
  ])
  assert.equal(summary.traces[0]?.outcome, 'ok')
  assert.equal(summary.traces[0]?.error_kind, null)
})

test('missing timestamps do not inflate request latency from the unix epoch', () => {
  const summary = buildTraceSummary([
    entry(0, 'req-time', 'start', { surface: 'api' }),
    entry(10, 'req-time', 'finish', { surface: 'dispatch', upstream: 'github', elapsed_ms: 10 }, {
      service: 'upstream.pool',
      action: 'upstream.request',
      timestamp: null,
    }),
    entry(75, 'req-time', 'finish', { surface: 'api' }),
  ])
  assert.equal(summary.traces[0]?.elapsed_ms, 75)
})

test('drops the oldest correlation group when a retained sample is truncated', () => {
  const summary = buildTraceSummary([
    entry(100, 'req-new', 'finish', { surface: 'api' }),
    entry(50, 'req-old', 'finish', { surface: 'api' }),
    entry(0, 'req-old', 'start', { surface: 'api' }),
  ], { truncated: true })
  assert.deepEqual(summary.traces.map((trace) => trace.id), ['req-new'])
})
