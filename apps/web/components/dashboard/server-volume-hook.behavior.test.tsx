import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { SWRConfig, useSWRConfig } from 'swr'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store'
import type { DashboardMetrics } from '../../lib/types/metrics'
import { useServerVolume } from './server-volume-chart'

installTestDom()
process.env.NEXT_PUBLIC_MOCK_DATA = 'false'

test('server chart toggles and automatic events share one transactional sample until its deadline', async (t) => {
  t.mock.timers.enable({ apis: ['Date'], now: 1_800_000_000_000 })
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  let calls = 0
  let release!: (response: Response) => void
  globalThis.fetch = async (_url, init) => {
    calls += 1
    const { action, params } = JSON.parse(String(init?.body))
    assert.equal(action, 'gateway.usage.metrics')
    assert.equal(params.include_upstream_timeseries, true)
    assert.equal(params.upstream, undefined)
    return new Promise(resolve => { release = resolve })
  }
  let mutate!: ReturnType<typeof useSWRConfig>['mutate']
  const metrics = { window: '1h', until_ms: 0, timeseries: [{ ts: 1000, calls: 999, failed: 0 }] } as DashboardMetrics
  function Harness({ active }: { active: boolean }) {
    mutate = useSWRConfig().mutate
    const result = useServerVolume(active ? metrics : undefined)
    return <span>{result.error ? 'unavailable' : result.data?.buckets[0]?.total ?? 'loading'}</span>
  }
  const config = { provider: () => new Map(), dedupingInterval: 0 }
  const render = (active: boolean) => <SWRConfig value={config}><Harness active={active} /></SWRConfig>
  const view = await renderClient(render(true))
  const sample = () => Response.json({ upstreams: [{ upstream: 'A', calls: 6 }], timeseries: [{ ts_unix: 1, calls: 11, failed: 0 }], upstream_timeseries: { A: [{ ts_unix: 1, calls: 6, failed: 0 }] } })
  try {
    assert.equal(calls, 1)
    await view.rerender(render(false))
    await view.rerender(render(true))
    await act(async () => { window.dispatchEvent(new Event('online')); window.dispatchEvent(new Event('focus')); await Promise.resolve() })
    assert.equal(calls, 1)
    await act(async () => { release(sample()); await Promise.resolve() })
    assert.equal(view.container.textContent, '11', 'total came from the transactional response, not older main metrics')
    t.mock.timers.tick(10_000)
    await view.rerender(render(false))
    await view.rerender(render(true))
    await act(async () => { await mutate(key => Array.isArray(key) && key[0] === 'overview-server-volume') })
    assert.equal(calls, 1)
    t.mock.timers.tick(50_000)
    let next!: Promise<unknown>
    await act(async () => { next = mutate(key => Array.isArray(key) && key[0] === 'overview-server-volume'); await Promise.resolve() })
    assert.equal(calls, 2)
    await act(async () => { release(sample()); await next })
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})

test('old server without transactional buckets is honestly unavailable', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  globalThis.fetch = async () => Response.json({ upstreams: [], timeseries: [] })
  function Harness() {
    const result = useServerVolume({ window: '1h' } as DashboardMetrics)
    return <span>{result.error?.message ?? 'loading'}</span>
  }
  const view = await renderClient(<SWRConfig value={{ provider: () => new Map(), shouldRetryOnError: false }}><Harness /></SWRConfig>)
  try { assert.match(view.container.textContent ?? '', /does not support transactional/) }
  finally { await view.unmount(); globalThis.fetch = originalFetch }
})


test('server sample cannot complete into a new API authority', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  const originalBase = process.env.NEXT_PUBLIC_API_URL
  let finishOld!: (response: Response) => void
  let oldSignal: AbortSignal | null | undefined
  let calls = 0
  const response = (total: number) => Response.json({ upstreams: [], timeseries: [{ ts_unix: 1, calls: total, failed: 0 }], upstream_timeseries: {} })
  globalThis.fetch = async (_url, init) => {
    calls += 1
    if (calls === 1) { oldSignal = init?.signal; return new Promise(resolve => { finishOld = resolve }) }
    return response(2)
  }
  function Harness() {
    const result = useServerVolume({ window: '1h' } as DashboardMetrics)
    return <span>{result.data?.buckets[0]?.total ?? 'loading'}</span>
  }
  const config = { provider: () => new Map() }
  const element = () => <SWRConfig value={config}><Harness /></SWRConfig>
  const view = await renderClient(element())
  try {
    process.env.NEXT_PUBLIC_API_URL = '/other-authority'
    await view.rerender(element())
    assert.equal(oldSignal?.aborted, true)
    assert.equal(view.container.textContent, '2')
    await act(async () => { finishOld(response(999)); await Promise.resolve() })
    assert.equal(view.container.textContent, '2')
  } finally {
    await view.unmount(); globalThis.fetch = originalFetch
    if (originalBase === undefined) delete process.env.NEXT_PUBLIC_API_URL
    else process.env.NEXT_PUBLIC_API_URL = originalBase
  }
})
