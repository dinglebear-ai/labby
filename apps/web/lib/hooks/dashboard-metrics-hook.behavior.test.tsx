import test, { type TestContext } from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { SWRConfig, useSWRConfig } from 'swr'
import { installTestDom, renderClient } from '../testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests, getBrowserSessionContextIdentity, getBrowserSessionEpoch, selectSessionWorkspace } from '../auth/session-store.ts'
import type { AuthoritySnapshot } from '../auth/authority.ts'
import { useDashboardMetrics } from './use-dashboard-metrics.ts'

installTestDom()
process.env.NEXT_PUBLIC_MOCK_DATA = 'false'

function aggregate(total: number) {
  return {
    window_total_calls: total, total_calls: total, error_calls: 0,
    avg_elapsed_ms: 12, p50_elapsed_ms: 8, p95_elapsed_ms: 20, p99_elapsed_ms: 20,
    distinct_tools: 1, distinct_actors: 1, peak_per_min: 1,
    top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [], upstreams: [],
    hourly: [], timeseries: [], facets: { tools: [], actors: [], upstreams: [], outcomes: [] },
  }
}

const logs = {
  kind: 'server_logs', entries: [], matched: 0, scanned_lines: 0,
  malformed_lines: 0, scanned_bytes: 0, max_scan_bytes: 1024, truncated: false,
}
const settle = async () => { await act(async () => { await Promise.resolve(); await Promise.resolve() }) }
const deferred = <T,>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(done => { resolve = done })
  return { promise, resolve }
}

function installClock(t: TestContext) {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout', 'setInterval'], now: 1_800_000_000_000 })
  const previous = { timeout: window.setTimeout, clearTimeout: window.clearTimeout }
  window.setTimeout = ((callback: TimerHandler, delay?: number) => globalThis.setTimeout(callback as () => void, delay) as unknown as number) as typeof window.setTimeout
  window.clearTimeout = ((id: number) => globalThis.clearTimeout(id)) as typeof window.clearTimeout
  return () => {
    window.setTimeout = previous.timeout
    window.clearTimeout = previous.clearTimeout
    t.mock.timers.reset()
  }
}

test('Overview keeps an in-flight dirty row, retries failed aggregates, and bounds requests', async (t) => {
  const restoreClock = installClock(t)
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  const first = deferred<Response>()
  const slow = deferred<Response>()
  let latestId = 1
  let aggregates = 0
  let signals = 0
  let logQueries = 0
  let failNext = false
  let delayNext = false
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'gateway.usage.calls') {
      signals += 1
      return Response.json({ latest_ingested_call_id: latestId, calls: [{ id: latestId, ts_unix: 1_800_000_000 }] })
    }
    if (action === 'server_logs.query') {
      logQueries += 1
      return Response.json(logs)
    }
    assert.equal(action, 'gateway.usage.metrics')
    aggregates += 1
    if (aggregates === 1) return first.promise
    if (delayNext) { delayNext = false; return slow.promise }
    if (failNext) {
      failNext = false
      return Response.json({ message: 'temporary failure' }, { status: 503 })
    }
    return Response.json(aggregate(aggregates))
  }
  let refresh!: ReturnType<typeof useSWRConfig>['mutate']
  function Harness() {
    refresh = useSWRConfig().mutate
    const result = useDashboardMetrics('1h')
    return <span>{result.data?.tool_calls.total ?? 'loading'}</span>
  }
  const view = await renderClient(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness /></SWRConfig>)
  try {
    assert.equal(aggregates, 1)
    const changeKey = ['dashboard-change', '1h', '/v1', getBrowserSessionContextIdentity(), getBrowserSessionEpoch()]
    latestId = 2 // Same second, distinct durable row ID.
    await act(async () => { await refresh(changeKey) })
    first.resolve(Response.json(aggregate(1)))
    await settle()
    assert.equal(view.container.textContent, '1')
    await act(async () => { t.mock.timers.tick(10_000); await Promise.resolve() })
    assert.equal(aggregates, 2, 'initial in-flight change was not lost')
    assert.equal(view.container.textContent, '2')
    assert.equal(logQueries, 1, 'activity refresh reused the retained log sample')

    latestId = 3
    failNext = true
    await act(async () => { await refresh(changeKey) })
    await act(async () => { t.mock.timers.tick(10_000); await Promise.resolve() })
    assert.equal(aggregates, 3)
    assert.equal(view.container.textContent, '2', 'last successful sample remains visible')
    await act(async () => { t.mock.timers.tick(2_000); await Promise.resolve() })
    assert.equal(aggregates, 4, 'unchanged dirty token retried after failure')
    assert.equal(view.container.textContent, '4')
    assert.ok(signals <= 5, `unexpected signal requests: ${signals}`)
    assert.equal(logQueries, 1)

    latestId = 4
    delayNext = true
    await act(async () => { await refresh(changeKey) })
    await act(async () => { t.mock.timers.tick(10_000); await Promise.resolve() })
    assert.equal(aggregates, 5)
    latestId = 5
    await act(async () => { await refresh(changeKey) })
    await act(async () => { t.mock.timers.tick(10_000); await Promise.resolve() })
    assert.equal(aggregates, 5, 'slow aggregate stays single-flight')
    slow.resolve(Response.json(aggregate(5)))
    await settle()
    await act(async () => { t.mock.timers.tick(10_000); await Promise.resolve() })
    assert.equal(aggregates, 6, 'new row during slow aggregate stays dirty')
    assert.ok(signals <= 13, `unexpected signal requests: ${signals}`)
    assert.equal(logQueries, 1, 'no independent five-second log poll')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    restoreClock()
  }
})

test('authority and window changes abort stale work and never display prior metrics', async () => {
  const authority: AuthoritySnapshot = {
    schemaVersion: 1, compatibilityGeneration: 1, principalId: 'operator',
    organizationId: 'org-1', activeOwner: { kind: 'team' as const, id: 'team-a' },
    activeTeamId: 'team-a', teams: [
      { id: 'team-a', role: 'owner' as const, membershipEpoch: 1, policyEpoch: 1 },
      { id: 'team-b', role: 'owner' as const, membershipEpoch: 1, policyEpoch: 1 },
    ], projects: [], capabilities: ['scope.read'], generation: 1,
  }
  __setBrowserSessionStateForTests({
    status: 'authenticated', user: { sub: 'operator' }, expiresAt: 1_900_000_000,
    csrfToken: 'test-csrf', authority,
  })
  const originalFetch = globalThis.fetch
  const originalBase = process.env.NEXT_PUBLIC_API_URL
  const first = deferred<Response>()
  const last = deferred<Response>()
  let initialSignal: AbortSignal | undefined
  let lastSignal: AbortSignal | undefined
  let aggregates = 0
  globalThis.fetch = async (url, init) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'server_logs.query') return Response.json(logs)
    if (action === 'gateway.usage.calls') return Response.json({ latest_ingested_call_id: 1, calls: [{ id: 1, ts_unix: 1_800_000_000 }] })
    aggregates += 1
    if (aggregates === 1) { initialSignal = init?.signal ?? undefined; return first.promise }
    if (aggregates === 4) {
      assert.match(String(url), /^\/alternate\//)
      lastSignal = init?.signal ?? undefined
      return last.promise
    }
    return Response.json(aggregate(aggregates))
  }
  function Harness({ period }: { period: '1h' | '24h' }) {
    const result = useDashboardMetrics(period)
    return <span>{period}:{result.data?.tool_calls.total ?? 'loading'}</span>
  }
  const view = await renderClient(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness period="1h" /></SWRConfig>)
  let mounted = true
  try {
    assert.equal(view.container.textContent, '1h:loading')
    await act(async () => { selectSessionWorkspace({ teamId: 'team-b' }) })
    assert.equal(initialSignal?.aborted, true)
    assert.equal(view.container.textContent, '1h:2')
    await view.rerender(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness period="24h" /></SWRConfig>)
    assert.equal(view.container.textContent, '24h:3')
    first.resolve(Response.json(aggregate(999)))
    await settle()
    assert.equal(view.container.textContent, '24h:3')
    process.env.NEXT_PUBLIC_API_URL = '/alternate'
    await view.rerender(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness period="24h" /></SWRConfig>)
    assert.equal(view.container.textContent, '24h:loading', 'prior API base was not published')
    assert.equal(aggregates, 4)
    await view.unmount()
    mounted = false
    assert.equal(lastSignal?.aborted, true, 'unmount cancels only its own request')
    last.resolve(Response.json(aggregate(4)))
  } finally {
    if (mounted) await view.unmount()
    globalThis.fetch = originalFetch
    if (originalBase === undefined) delete process.env.NEXT_PUBLIC_API_URL
    else process.env.NEXT_PUBLIC_API_URL = originalBase
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})

test('hidden and offline pauses catch up on return; unsupported signal retains minute fallback', async (t) => {
  const restoreClock = installClock(t)
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  const visibility = Object.getOwnPropertyDescriptor(document, 'visibilityState')
  const online = Object.getOwnPropertyDescriptor(navigator, 'onLine')
  let signals = 0
  let aggregates = 0
  let unsupported = false
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'server_logs.query') return Response.json(logs)
    if (action === 'gateway.usage.calls') {
      signals += 1
      return unsupported
        ? Response.json({ message: 'unsupported', kind: 'usage_change_token_unsupported' }, { status: 409 })
        : Response.json({ latest_ingested_call_id: signals, calls: [{ id: signals, ts_unix: 1_800_000_000 }] })
    }
    aggregates += 1
    return Response.json(aggregate(aggregates))
  }
  function Harness() {
    const result = useDashboardMetrics('1h')
    return <span>{result.data?.tool_calls.total ?? 'loading'}</span>
  }
  const view = await renderClient(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness /></SWRConfig>)
  try {
    assert.equal(aggregates, 1)
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' })
    await act(async () => { t.mock.timers.tick(20_000); await Promise.resolve() })
    assert.equal(aggregates, 1)
    const beforeVisible = signals
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' })
    await act(async () => { document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve() })
    assert.equal(signals, beforeVisible + 1)
    assert.equal(aggregates, 2, 'visible catch-up applies the new row')

    Object.defineProperty(navigator, 'onLine', { configurable: true, value: false })
    await act(async () => { t.mock.timers.tick(10_000); await Promise.resolve() })
    const beforeOnline = signals
    Object.defineProperty(navigator, 'onLine', { configurable: true, value: true })
    unsupported = true
    await act(async () => { window.dispatchEvent(new Event('online')); await Promise.resolve() })
    assert.equal(signals, beforeOnline + 1)
    await act(async () => { t.mock.timers.tick(20_000); await Promise.resolve() })
    assert.equal(signals, beforeOnline + 1, 'unsupported signal stops polling')
    await act(async () => { t.mock.timers.tick(40_000); await Promise.resolve() })
    assert.ok(aggregates >= 3, 'minute fallback still refreshes metrics')
    assert.ok(aggregates <= 4, `unexpected aggregate requests: ${aggregates}`)
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    if (visibility) Object.defineProperty(document, 'visibilityState', visibility)
    if (online) Object.defineProperty(navigator, 'onLine', online)
    restoreClock()
  }
})

test('longer windows cap detailed refresh at thirty and sixty seconds', async (t) => {
  const restoreClock = installClock(t)
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  let aggregates = 0
  let signals = 0
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'server_logs.query') return Response.json(logs)
    if (action === 'gateway.usage.calls') {
      signals += 1
      return Response.json({ latest_ingested_call_id: 1, calls: [{ id: 1, ts_unix: 1_800_000_000 }] })
    }
    aggregates += 1
    return Response.json(aggregate(aggregates))
  }
  function Harness({ period }: { period: '24h' | '7d' }) {
    const result = useDashboardMetrics(period)
    return <span>{result.data?.tool_calls.total ?? 'loading'}</span>
  }
  try {
    for (const [period, minimum] of [['24h', 30_000], ['7d', 60_000]] as const) {
      const before = aggregates
      const view = await renderClient(<SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}><Harness period={period} /></SWRConfig>)
      try {
        assert.equal(aggregates, before + 1)
        await act(async () => { t.mock.timers.tick(minimum - 1); await Promise.resolve() })
        assert.equal(aggregates, before + 1, `${period} refreshed before its interval`)
        await act(async () => { t.mock.timers.tick(1); await Promise.resolve() })
        assert.equal(aggregates, before + 2, `${period} missed its due refresh or duplicated it`)
      } finally { await view.unmount() }
    }
    assert.ok(signals <= 22, `unexpected signal requests: ${signals}`)
  } finally {
    globalThis.fetch = originalFetch
    restoreClock()
  }
})

test('unmounting a hook leaves another subscriber’s shared fetch alive', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  const pending = deferred<Response>()
  let signal: AbortSignal | undefined
  let aggregates = 0
  globalThis.fetch = async (_url, init) => {
    const { action } = JSON.parse(String(init?.body)) as { action: string }
    if (action === 'server_logs.query') return Response.json(logs)
    if (action === 'gateway.usage.calls') return Response.json({ latest_ingested_call_id: 1, calls: [{ id: 1, ts_unix: 1_800_000_000 }] })
    aggregates += 1
    signal = init?.signal ?? undefined
    return pending.promise
  }
  function Harness({ id }: { id: string }) {
    const result = useDashboardMetrics('1h')
    return <span data-id={id}>{result.data?.tool_calls.total ?? 'loading'}</span>
  }
  const config = { provider: () => new Map() }
  const view = await renderClient(<SWRConfig value={config}><Harness key="first" id="first" /></SWRConfig>)
  try {
    assert.equal(aggregates, 1)
    await view.rerender(<SWRConfig value={config}><Harness key="first" id="first" /><Harness key="second" id="second" /></SWRConfig>)
    assert.equal(aggregates, 1, 'SWR subscribers share the active request')
    await view.rerender(<SWRConfig value={config}><Harness key="second" id="second" /></SWRConfig>)
    assert.equal(signal?.aborted, false, 'first unmount must preserve the second subscriber’s request')
    pending.resolve(Response.json(aggregate(7)))
    await settle()
    assert.equal(view.container.querySelector('[data-id="second"]')?.textContent, '7')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
})
