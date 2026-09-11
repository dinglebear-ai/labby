import test from 'node:test'
import assert from 'node:assert/strict'

import { GatewayApiError } from '@/lib/api/gateway-client'
import { Window } from 'happy-dom'
import { classifyStatusFailure, deriveConsoleStatus, upstreamMetricColor } from './console-status-strip'

test('console status derives connected upstream, session, and exposed-tool counts', () => {
  const snapshot = deriveConsoleStatus(
    [
      { name: 'alpha', enabled: true, connected: true, exposed_tool_count: 7 },
      { name: 'beta', enabled: true, connected: false, exposed_tool_count: 0 },
      { name: 'gamma', enabled: true, connected: true, exposed_tool_count: 25 },
      { name: 'disabled', enabled: false, connected: true, exposed_tool_count: 999 },
    ],
    [
      { transport: 'http', connected_at: '2026-09-09T00:00:00Z' },
      { transport: 'stdio', connected_at: '2026-09-09T00:01:00Z' },
    ],
  )

  assert.deepEqual(snapshot, {
    connected: 2,
    total: 3,
    sessions: 2,
    tools: 32,
  })
})

test('console status omits the session count when client inventory is unavailable', () => {
  const snapshot = deriveConsoleStatus([
    { name: 'alpha', enabled: true, connected: true, exposed_tool_count: 4 },
  ])
  assert.deepEqual(snapshot, {
    connected: 1,
    total: 1,
    sessions: undefined,
    tools: 4,
  })
})

test('the up metric is healthy only when every enabled upstream is connected', () => {
  assert.equal(upstreamMetricColor({ connected: 3, total: 3 }), 'var(--aurora-success)')
  assert.equal(upstreamMetricColor({ connected: 2, total: 3 }), 'var(--aurora-warn)')
  assert.equal(upstreamMetricColor({ connected: 0, total: 0 }), 'var(--aurora-text-muted)')
})

test('missing admin scope hides the strip while other failures are surfaced with their reason', () => {
  assert.deepEqual(classifyStatusFailure(new GatewayApiError('forbidden', 403)), { kind: 'unauthorized' })
  assert.deepEqual(classifyStatusFailure(new GatewayApiError('unauthorized', 401)), { kind: 'unauthorized' })
  assert.deepEqual(classifyStatusFailure(new GatewayApiError('boom', 500)), { kind: 'unavailable', reason: 'boom' })
  assert.deepEqual(classifyStatusFailure(new Error('network down')), { kind: 'unavailable', reason: 'network down' })
  assert.deepEqual(classifyStatusFailure('offline'), { kind: 'unavailable', reason: 'offline' })
  // An abort (unmount or authority change) is not a failure and must not stick as "unavailable".
  const { DOMException: HappyDOMException } = new Window()
  assert.deepEqual(classifyStatusFailure(new HappyDOMException('Authority context changed', 'AbortError')), { kind: 'loading' })
  const abort = new Error('aborted'); abort.name = 'AbortError'
  assert.deepEqual(classifyStatusFailure(abort), { kind: 'loading' })
})
