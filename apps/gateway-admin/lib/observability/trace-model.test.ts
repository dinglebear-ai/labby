import test from 'node:test'
import assert from 'node:assert/strict'

import { buildTraceSummary } from './trace-model.ts'
import type { ServerLogEntry } from '@/lib/types/traces'

function entry(
  offsetMs: number,
  requestId: string | null,
  event: string,
  fields: Record<string, unknown> = {},
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
  }
}

test('correlates structured events and derives trace metrics', () => {
  const summary = buildTraceSummary([
    entry(0, 'req-ok', 'start', { surface: 'mcp', actor_key: 'actor-1' }),
    entry(40, 'req-ok', 'finish', {
      surface: 'dispatch',
      upstream: 'github',
      elapsed_ms: 40,
      response_bytes: 2048,
    }),
    entry(100, 'req-slow', 'start', { surface: 'api' }),
    entry(300, 'req-slow', 'error', {
      surface: 'api',
      upstream: 'slack',
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
  assert.equal(summary.p50_ms, 40)
  assert.equal(summary.p95_ms, 200)
  assert.deepEqual(summary.upstreams, [
    { name: 'github', count: 1 },
    { name: 'slack', count: 1 },
  ])

  const ok = summary.traces.find((trace) => trace.id === 'req-ok')
  assert.equal(ok?.events.length, 2)
  assert.equal(ok?.response_bytes, 2048)
  assert.equal(ok?.actor_key, 'actor-1')

  const failed = summary.traces.find((trace) => trace.id === 'req-slow')
  assert.equal(failed?.outcome, 'failed')
  assert.equal(failed?.error_kind, 'timeout')
  assert.equal((failed?.input_tokens ?? 0) + (failed?.output_tokens ?? 0), 40)
})

