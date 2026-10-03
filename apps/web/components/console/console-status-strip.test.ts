import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { GatewayApiError } from '@/lib/api/gateway-client'
import { Window } from 'happy-dom'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { classifyStatusFailure, deriveConsoleAttention, deriveConsoleCapabilityAlerts, deriveConsoleStatus, loadConsoleStatus, upstreamMetricColor, useConsoleStatus } from './console-status-strip'

test('partial warning inventory failure preserves the missing snapshot and still reports runtime alerts', async () => {
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_input, init) => {
    const { action } = JSON.parse(String(init?.body))
    if (action === 'gateway.mcp.list') return new Response(JSON.stringify([{ name: 'partial', connected: true, likely_stale_count: 1 }]), { status: 200 })
    if (action === 'gateway.clients.list') return new Response('[]', { status: 200 })
    return new Response('Inventory temporarily unavailable', { status: 503 })
  }) as typeof fetch
  try {
    const state = await loadConsoleStatus(new AbortController().signal)
    assert.equal(state.kind, 'ready')
    if (state.kind !== 'ready') return
    assert.equal(state.alerts, undefined, 'missing warning inventory must not clear warning incidents')
    assert.equal(state.runtimeAlerts?.[0].key, 'gateway:partial:stale')
  } finally { globalThis.fetch = originalFetch }
})

test('connected capability failures alert without claiming disconnection', () => {
  const runtime = [{ name: 'connected', connected: true, capability_observation: {
    scope: 'credential' as const,
    tools: { state: 'known' as const, discovered: 1, exposed: 1 },
    resources: { state: 'failed' as const, discovered: 1, exposed: 0, error: 'Resources refresh failed' },
    prompts: { state: 'unknown' as const, discovered: null, exposed: null },
    skills: { state: 'unknown' as const, discovered: null, exposed: null },
  } }]
  assert.deepEqual(deriveConsoleAttention(runtime), [])
  const alerts = deriveConsoleCapabilityAlerts(runtime)
  assert.equal(alerts.length, 1)
  assert.equal(alerts[0].message, 'Resources refresh failed')
  assert.equal(alerts[0].key, 'gateway:connected:capability:resources')
})

test('console attention excludes idle credential caches but retains actual failures', () => {
  assert.deepEqual(deriveConsoleAttention([
    { name: 'unchecked', connected: false, capability_observation: { scope: 'credential', tools: { state: 'unknown', discovered: null, exposed: null }, resources: { state: 'unknown', discovered: null, exposed: null }, prompts: { state: 'unknown', discovered: null, exposed: null }, skills: { state: 'unknown', discovered: null, exposed: null } } },
    { name: 'down', connected: false },
  ]), ['down'])
})

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

test('disabled console status never polls protected gateway APIs', async () => {
  installTestDom()
  const originalFetch = globalThis.fetch
  let fetchCount = 0
  globalThis.fetch = (async () => {
    fetchCount += 1
    return new Response('{}', { status: 200 })
  }) as typeof fetch

  function Probe() {
    const state = useConsoleStatus(false)
    return React.createElement('span', { 'data-kind': state.kind }, state.kind)
  }

  const view = await renderClient(React.createElement(Probe))
  try {
    assert.equal(view.container.querySelector('[data-kind]')?.getAttribute('data-kind'), 'unauthorized')
    assert.equal(fetchCount, 0)
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
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
