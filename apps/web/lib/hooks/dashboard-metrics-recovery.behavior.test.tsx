import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { SWRConfig } from 'swr'
import { installTestDom, renderClient } from '../testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'
import { useDashboardMetrics } from './use-dashboard-metrics.ts'

installTestDom()
process.env.NEXT_PUBLIC_MOCK_DATA = 'false'

test('initial transient failures recover without focus, user action, or an existing metrics sample', async (t) => {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout', 'setInterval'], now: 1_800_000_000_000 })
  const savedTimeout = window.setTimeout
  const savedClear = window.clearTimeout
  window.setTimeout = ((handler: TimerHandler, delay?: number) => globalThis.setTimeout(handler as () => void, delay) as unknown as number) as typeof window.setTimeout
  window.clearTimeout = ((id: number) => globalThis.clearTimeout(id)) as typeof window.clearTimeout
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  let aggregates = 0
  let signals = 0
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'server_logs.query') return Response.json({ entries: [], truncated: false })
    if (action === 'gateway.usage.calls') {
      signals += 1
      return signals === 1 ? Response.json({ message: 'temporary failure' }, { status: 503 }) : Response.json({ latest_ingested_call_id: 1, calls: [{ id: 1, ts_unix: 1_800_000_000 }] })
    }
    assert.equal(action, 'gateway.usage.metrics')
    aggregates += 1
    if (aggregates === 1) return Response.json({ message: 'temporary failure' }, { status: 503 })
    return Response.json({
      window_total_calls: 4, total_calls: 4, error_calls: 0,
      avg_elapsed_ms: 12, p50_elapsed_ms: 8, p95_elapsed_ms: 20, p99_elapsed_ms: 20,
      distinct_tools: 0, distinct_actors: 0, peak_per_min: 0,
      top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [], upstreams: [],
      hourly: [], timeseries: [], facets: { tools: [], actors: [], upstreams: [], outcomes: [] },
    })
  }
  function Harness() {
    const result = useDashboardMetrics('1h')
    return <span>{result.data?.tool_calls.total ?? (result.error ? 'error' : 'loading')}</span>
  }
  const view = await renderClient(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness /></SWRConfig>)
  try {
    assert.equal(view.container.textContent, 'error')
    assert.equal(aggregates, 1)
    assert.equal(signals, 1)
    await act(async () => { t.mock.timers.tick(30_000); await Promise.resolve() })
    assert.ok(signals >= 2, 'change detector retries a transient initial failure')
    assert.ok(signals <= 4, 'change detector recovery is bounded')
    await act(async () => { t.mock.timers.tick(30_000); await Promise.resolve() })
    assert.equal(aggregates, 2, 'minute recovery works even without any successful initial data')
    assert.equal(view.container.textContent, '4')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    window.setTimeout = savedTimeout
    window.clearTimeout = savedClear
    t.mock.timers.reset()
  }
})


test('explicit Refresh bypasses the automatic throttle without rescanning recent logs', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  let aggregates = 0
  let logQueries = 0
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'server_logs.query') { logQueries += 1; return Response.json({ entries: [], truncated: false }) }
    if (action === 'gateway.usage.calls') return Response.json({ latest_ingested_call_id: 1, calls: [{ id: 1, ts_unix: 1_800_000_000 }] })
    aggregates += 1
    return Response.json({
      window_total_calls: aggregates, total_calls: aggregates, error_calls: 0,
      avg_elapsed_ms: 12, p50_elapsed_ms: 8, p95_elapsed_ms: 20, p99_elapsed_ms: 20,
      distinct_tools: 0, distinct_actors: 0, peak_per_min: 0,
      top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [], upstreams: [],
      hourly: [], timeseries: [], facets: { tools: [], actors: [], upstreams: [], outcomes: [] },
    })
  }
  let refresh!: () => Promise<unknown>
  function Harness() {
    const result = useDashboardMetrics('1h')
    refresh = result.refresh
    return <span>{result.data?.tool_calls.total ?? 'loading'}</span>
  }
  const view = await renderClient(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness /></SWRConfig>)
  try {
    assert.equal(view.container.textContent, '1')
    await act(async () => { await refresh() })
    assert.equal(aggregates, 2, 'manual refresh did not return the cached aggregate')
    assert.equal(view.container.textContent, '2')
    assert.equal(logQueries, 1, 'manual counter refresh did not bypass log retention bounds')
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})
