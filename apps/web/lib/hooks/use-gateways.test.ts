import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { gatewayApi } from '../api/gateway-client'
import { installTestDom, renderClient } from '../testing/dom-test-utils.tsx'
import type { CodeModeConfig } from '../types/gateway'
import {
  applyCodeModeConfigInput,
  GATEWAYS_KEY,
  gatewaysRequestKey,
  gatewaysRuntimeRequestKey,
  useGatewaySnapshots,
} from './use-gateways'

test('mock Code Mode updates project flat search inputs into the nested server shape', () => {
  const current: CodeModeConfig = {
    enabled: false,
    timeout_ms: 5000,
    max_tool_calls: 8,
    max_response_bytes: 24 * 1024,
    max_response_tokens: 6000,
    search: {
      sources: ['personal_labby', 'team_depot', 'public_depot'],
      kinds: ['tool', 'skill', 'command', 'prompt', 'subagent', 'snippet'],
    },
  }

  const updated = applyCodeModeConfigInput(current, {
    enabled: true,
    search_sources: ['public_depot'],
    search_kinds: ['skill', 'prompt'],
  })

  assert.equal(updated.enabled, true)
  assert.deepEqual(updated.search, {
    sources: ['public_depot'],
    kinds: ['skill', 'prompt'],
  })
  assert.equal('search_sources' in updated, false)
  assert.equal('search_kinds' in updated, false)
})

test('gateway loading can be disabled for closed demand-driven dialogs', () => {
  assert.equal(gatewaysRequestKey(false), null)
  assert.equal(gatewaysRequestKey(true), GATEWAYS_KEY)
})

test('configuration-only gateway loading never hydrates fleet runtime state', () => {
  const gateways = [{ id: 'alpha' }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]
  assert.equal(gatewaysRuntimeRequestKey(false, true, gateways), null)
  assert.equal(gatewaysRuntimeRequestKey(true, false, gateways), null)
  assert.deepEqual(gatewaysRuntimeRequestKey(true, true, gateways), ['/gateways/runtime', '[{"id":"alpha","warnings":[]}]'])
})

test('runtime cache key ignores synthesized warning timestamps but preserves warning meaning', () => {
  const base = {
    id: 'alpha',
    warnings: [{ code: 'prompts_unavailable', message: 'Prompts timed out', timestamp: '2026-09-18T23:00:00Z' }],
  } as NonNullable<Parameters<typeof gatewaysRuntimeRequestKey>[2]>[number]
  const laterTimestamp = {
    ...base,
    warnings: [{ ...base.warnings[0], timestamp: '2026-09-18T23:00:05Z' }],
  }
  const changedWarning = {
    ...base,
    warnings: [{ ...base.warnings[0], message: 'Prompts recovered then failed again' }],
  }

  assert.deepEqual(
    gatewaysRuntimeRequestKey(true, true, [base]),
    gatewaysRuntimeRequestKey(true, true, [laterTimestamp]),
  )
  assert.notDeepEqual(
    gatewaysRuntimeRequestKey(true, true, [base]),
    gatewaysRuntimeRequestKey(true, true, [changedWarning]),
  )
})

test('gateway snapshots stay idle until their consumer is enabled', async () => {
  installTestDom()
  const originalList = gatewayApi.list
  let listCalls = 0
  gatewayApi.list = async () => { listCalls += 1; return [] }
  function Harness({ enabled }: { enabled: boolean }) {
    const result = useGatewaySnapshots(enabled)
    return React.createElement('span', null, result.isLoading ? 'loading' : 'ready')
  }

  const view = await renderClient(React.createElement(Harness, { enabled: false }))
  try {
    await act(async () => {})
    assert.equal(listCalls, 0)
    await view.rerender(React.createElement(Harness, { enabled: true }))
    for (let attempt = 0; attempt < 50 && listCalls === 0; attempt += 1) {
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
    }
    assert.equal(listCalls, 1)
  } finally {
    gatewayApi.list = originalList
    await view.unmount()
  }
})


test('same-ID configuration edits create a fresh runtime cache entry', () => {
  const oldRows = [{ id: 'alpha', enabled: true, name: 'old gateway' }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]
  const newRows = [{ id: 'alpha', enabled: false, name: 'new gateway' }] as Parameters<typeof gatewaysRuntimeRequestKey>[2]
  assert.notDeepEqual(gatewaysRuntimeRequestKey(true, true, oldRows), gatewaysRuntimeRequestKey(true, true, newRows))
})

test('tools view hydrates full tool records on demand', async () => {
  const { SWRConfig } = await import('swr')
  const { useGateways } = await import('./use-gateways')
  const { mockGateways } = await import('../api/mock-data')
  installTestDom()
  const original = {
    list: gatewayApi.list,
    hydrateRuntime: gatewayApi.hydrateRuntime,
    hydrateToolInventory: gatewayApi.hydrateToolInventory,
    refreshStatus: gatewayApi.refreshStatus,
  }
  const summaryOnly = {
    ...mockGateways[0],
    discovery: { ...mockGateways[0].discovery, tools: [] },
    status: { ...mockGateways[0].status, discovered_tool_count: 1, exposed_tool_count: 1 },
  }
  let inventoryCalls = 0
  gatewayApi.list = async () => [summaryOnly]
  gatewayApi.hydrateRuntime = async rows => rows
  gatewayApi.hydrateToolInventory = async rows => {
    inventoryCalls += 1
    return rows.map(row => ({
      ...row,
      discovery: { ...row.discovery, tools: [{ name: 'search', exposed: true, matched_by: '*' }] },
    }))
  }
  gatewayApi.refreshStatus = async () => undefined
  function Harness() {
    const result = useGateways(true, true)
    return React.createElement('span', null, result.data?.[0]?.discovery.tools[0]?.name ?? 'loading')
  }
  const view = await renderClient(React.createElement(
    SWRConfig,
    { value: { provider: () => new Map(), shouldRetryOnError: false } },
    React.createElement(Harness),
  ))
  try {
    for (let attempt = 0; attempt < 50 && view.container.textContent !== 'search'; attempt += 1) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(view.container.textContent, 'search')
    assert.equal(inventoryCalls, 1)
  } finally {
    await view.unmount()
    gatewayApi.list = original.list
    gatewayApi.hydrateRuntime = original.hydrateRuntime
    gatewayApi.hydrateToolInventory = original.hydrateToolInventory
    gatewayApi.refreshStatus = original.refreshStatus
  }
})

test('same-ID edits replace hydrated rows even when catalog polling is inactive', async () => {
  const { SWRConfig, useSWRConfig } = await import('swr')
  const { useGateways } = await import('./use-gateways')
  const { mockGateways } = await import('../api/mock-data')
  installTestDom()
  const originalList = gatewayApi.list
  const originalHydrate = gatewayApi.hydrateRuntime
  const originalRefresh = gatewayApi.refreshStatus
  const first = { ...mockGateways[0], id: 'configuration-regression', name: 'Before edit' }
  const second = { ...first, name: 'After edit', enabled: false }
  let update: ReturnType<typeof useSWRConfig>['mutate']
  const hydrated: string[] = []
  gatewayApi.list = async () => [first]
  gatewayApi.hydrateRuntime = async rows => { hydrated.push(rows[0]?.name); return rows }
  // Catalog warming has stopped; configuration changes must independently
  // invalidate hydrated rows, without relying on its refresh interval.
  gatewayApi.refreshStatus = async () => { throw new Error('warming unavailable') }
  function Harness() {
    update = useSWRConfig().mutate
    const rows = useGateways()
    return React.createElement('span', null, rows.data?.[0]?.name ?? 'loading')
  }
  const view = await renderClient(React.createElement(SWRConfig, { value: { provider: () => new Map(), shouldRetryOnError: false } }, React.createElement(Harness)))
  const waitForName = async (name: string) => {
    for (let attempt = 0; attempt < 50 && view.container.textContent !== name; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(view.container.textContent, name)
  }
  try {
    await waitForName('Before edit')
    await act(async () => { await update(GATEWAYS_KEY, [second], false) })
    await waitForName('After edit')
    assert.ok(hydrated.includes('After edit'))
  } finally {
    await view.unmount()
    gatewayApi.list = originalList
    gatewayApi.hydrateRuntime = originalHydrate
    gatewayApi.refreshStatus = originalRefresh
  }
})

test('active catalog polling refreshes same-ID tool inventory and retries a failed inventory', async () => {
  const { SWRConfig } = await import('swr')
  const { useGateways } = await import('./use-gateways')
  const { mockGateways } = await import('../api/mock-data')
  const window = installTestDom()
  const original = { list: gatewayApi.list, runtime: gatewayApi.hydrateRuntime,
    inventory: gatewayApi.hydrateToolInventory, refresh: gatewayApi.refreshStatus,
    interval: window.setInterval, clearInterval: window.clearInterval }
  let tick: (() => void) | undefined
  window.setInterval = (callback, delay, ...args) => {
    tick = () => callback(...args)
    return original.interval.call(window, () => {}, delay)
  }
  const row = { ...mockGateways[0], id: 'stable-id', name: 'stable-id' }
  let calls = 0
  let toolName = 'alpha'
  gatewayApi.list = async () => [row]
  gatewayApi.hydrateRuntime = async rows => rows
  gatewayApi.refreshStatus = async () => { throw new Error('warming unavailable') }
  gatewayApi.hydrateToolInventory = async rows => {
    calls++
    return rows.map(item => ({ ...item,
      discovery: { ...item.discovery, tools: calls === 1 ? [] : [{ name: toolName, exposed: false, matched_by: null }] },
      warnings: calls === 1 ? [{ code: 'tool_inventory_unavailable', message: 'offline', timestamp: '2026-09-30T00:00:00Z' }] : [],
    }))
  }
  function Harness() {
    const result = useGateways(true, true)
    return React.createElement('span', null, result.data?.[0]?.discovery.tools[0]?.name ?? 'empty')
  }
  const view = await renderClient(React.createElement(SWRConfig,
    { value: { provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false } }, React.createElement(Harness)))
  const settle = async (expected: string) => {
    for (let attempt = 0; attempt < 50 && view.container.textContent !== expected; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(view.container.textContent, expected)
  }
  try {
    for (let attempt = 0; attempt < 50 && calls === 0; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(calls, 1)
    assert.ok(tick)
    await act(async () => tick!())
    await settle('alpha')
    toolName = 'beta'
    await act(async () => tick!())
    await settle('beta')
    assert.equal(calls, 3)
  } finally {
    await view.unmount()
    gatewayApi.list = original.list; gatewayApi.hydrateRuntime = original.runtime
    gatewayApi.hydrateToolInventory = original.inventory; gatewayApi.refreshStatus = original.refresh
    window.setInterval = original.interval; window.clearInterval = original.clearInterval
  }
})

test('a mounted gateway table observes runtime-only changes after sixty seconds', async () => {
  const { SWRConfig } = await import('swr')
  const { useGateways } = await import('./use-gateways')
  const { mockGateways } = await import('../api/mock-data')
  installTestDom()
  const original = { list: gatewayApi.list, hydrate: gatewayApi.hydrateRuntime, refresh: gatewayApi.refreshStatus,
    interval: window.setInterval, clearInterval: window.clearInterval, timeout: window.setTimeout, clearTimeout: window.clearTimeout }
  let now = 0
  let nextId = 1
  const timers = new Map<number, { run: () => void; at: number; interval: number }>()
  window.setInterval = ((run: () => void, delay: number) => { const id = nextId++; timers.set(id, { run, at: now + delay, interval: delay }); return id }) as typeof window.setInterval
  window.setTimeout = ((run: () => void, delay: number) => { const id = nextId++; timers.set(id, { run, at: now + delay, interval: 0 }); return id }) as typeof window.setTimeout
  window.clearInterval = window.clearTimeout = ((id: number) => { timers.delete(id) }) as typeof window.clearInterval
  const row = { ...mockGateways[0], id: 'runtime-only-polling' }
  let count = 1
  gatewayApi.list = async () => [row]
  gatewayApi.hydrateRuntime = async rows => rows.map(row => ({ ...row, status: { ...row.status, discovered_tool_count: count } }))
  gatewayApi.refreshStatus = async () => undefined
  function Harness() { const result = useGateways(); return React.createElement('span', null, result.data?.[0]?.status.discovered_tool_count ?? 'loading') }
  const view = await renderClient(React.createElement(SWRConfig, { value: { provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false } }, React.createElement(Harness)))
  const settle = async (expected: string) => {
    for (let attempt = 0; attempt < 50 && view.container.textContent !== expected; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(view.container.textContent, expected)
  }
  const advance = async (until: number) => {
    while (true) {
      const pending = [...timers].filter(([, timer]) => timer.at <= until).sort((a, b) => a[1].at - b[1].at)[0]
      if (!pending) break
      const [id, timer] = pending
      now = timer.at
      if (timer.interval) timer.at += timer.interval
      else timers.delete(id)
      await act(async () => { timer.run(); await new Promise(resolve => setTimeout(resolve, 1)) })
    }
    now = until
  }
  try {
    await settle('1')
    await advance(61_000)
    count = 7
    await advance(66_000)
    await settle('7')
  } finally {
    await view.unmount()
    gatewayApi.list = original.list; gatewayApi.hydrateRuntime = original.hydrate; gatewayApi.refreshStatus = original.refresh
    window.setInterval = original.interval; window.clearInterval = original.clearInterval
    window.setTimeout = original.timeout; window.clearTimeout = original.clearTimeout
  }
})

test('overlapping reloads of one server issue a single restart request', async () => {
  const { SWRConfig } = await import('swr')
  const { useGatewayMutations } = await import('./use-gateways')
  const { RELOAD_IN_FLIGHT_MESSAGE } = await import('../api/gateway-client')
  installTestDom()
  const original = { list: gatewayApi.list, reload: gatewayApi.reload, hydrate: gatewayApi.hydrateRuntime, refresh: gatewayApi.refreshStatus }
  let reloadCalls = 0
  let finishReload: (() => void) | undefined
  gatewayApi.list = async () => []
  gatewayApi.hydrateRuntime = async rows => rows
  gatewayApi.refreshStatus = async () => { throw new Error('warming unavailable') }
  gatewayApi.reload = async () => {
    reloadCalls += 1
    await new Promise<void>((resolve) => { finishReload = resolve })
    return { success: true, message: 'Server restarted successfully', previous_tool_count: 1, new_tool_count: 1 }
  }
  let reload: ReturnType<typeof useGatewayMutations>['reloadGateway'] | undefined
  function Harness() {
    reload = useGatewayMutations().reloadGateway
    return React.createElement('span', null, 'ready')
  }
  const view = await renderClient(React.createElement(SWRConfig, { value: { provider: () => new Map(), shouldRetryOnError: false } }, React.createElement(Harness)))
  try {
    await act(async () => {})
    assert.ok(reload)
    const first = reload('alpha')
    const second = await reload('alpha')
    assert.equal(reloadCalls, 1, 'the overlapping reload must not send a second restart')
    assert.equal(second.pending, true)
    assert.equal(second.message, RELOAD_IN_FLIGHT_MESSAGE)
    await act(async () => { finishReload?.() })
    const result = await first
    assert.equal(result.success, true)
    // A later reload, after the first completed, is a real request again.
    const later = reload('alpha')
    await act(async () => { finishReload?.() })
    await later
    assert.equal(reloadCalls, 2)
  } finally {
    await view.unmount()
    gatewayApi.list = original.list
    gatewayApi.reload = original.reload
    gatewayApi.hydrateRuntime = original.hydrate
    gatewayApi.refreshStatus = original.refresh
  }
})

test('a rename seeds the new detail cache without fetching the removed ID', async () => {
  const { SWRConfig, useSWRConfig } = await import('swr')
  const { useGateway, useGatewayMutations } = await import('./use-gateways')
  const { __setBrowserSessionStateForTests } = await import('../auth/session-store')
  installTestDom()
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const requests: Array<{ action: string; params: Record<string, unknown> }> = []
  let currentId = 'old-id'
  const viewFor = () => ({ config: { name: currentId, url: 'https://example.test/mcp', proxy_resources: true },
    runtime: { name: currentId, connected: true, tool_count: 0, resource_count: 0, prompt_count: 0 } })
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_input, init) => {
    const request = JSON.parse(String(init?.body ?? '{}'))
    requests.push(request)
    const { action, params } = request
    let result: unknown = []
    if (action === 'gateway.server.get') {
      if (params.id !== currentId) return new Response(JSON.stringify({ message: 'not found' }), { status: 404 })
      result = { id: currentId, name: currentId, source: 'custom_gateway' }
    } else if (action === 'gateway.get') result = viewFor()
    else if (action === 'gateway.update') { assert.equal(params.name, currentId); currentId = params.patch.name; result = viewFor() }
    return new Response(JSON.stringify(result), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }) as typeof fetch
  let rename: ReturnType<typeof useGatewayMutations>['updateGateway']
  let cache: ReturnType<typeof useSWRConfig>['cache'] | undefined
  let navigate: (id: string) => void
  function Harness() {
    const [id, setId] = React.useState('old-id')
    navigate = setId
    rename = useGatewayMutations().updateGateway
    cache = useSWRConfig().cache
    const row = useGateway(id)
    return React.createElement('span', null, row.error ? 'error' : row.data?.id ?? 'loading')
  }
  const view = await renderClient(React.createElement(SWRConfig,
    { value: { provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false } }, React.createElement(Harness)))
  try {
    for (let attempt = 0; attempt < 50 && view.container.textContent !== 'old-id'; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(view.container.textContent, 'old-id')
    assert.ok(cache, 'the mounted SWR provider must supply its cache')
    requests.length = 0
    await act(async () => { await rename('old-id', { name: 'new-id' }) })
    assert.equal(view.container.textContent, 'new-id')
    assert.equal(cache.get('/gateways/new-id')?.data?.id, 'new-id')
    assert.equal(requests.some(row => row.action === 'gateway.server.get' && row.params.id === 'old-id'), false)
    await act(async () => navigate('new-id'))
    assert.equal(cache.get('/gateways/old-id')?.data, undefined, 'navigation retires the removed-ID cache')
    await act(async () => { await rename('new-id', { name: 'old-id' }) })
    assert.equal(view.container.textContent, 'old-id')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
})

test('a six-second tool inventory publishes without overlapping five-second polls', async () => {
  const { SWRConfig } = await import('swr')
  const { useGateways } = await import('./use-gateways')
  const { mockGateways } = await import('../api/mock-data')
  const window = installTestDom()
  const original = { list: gatewayApi.list, runtime: gatewayApi.hydrateRuntime,
    inventory: gatewayApi.hydrateToolInventory, refresh: gatewayApi.refreshStatus,
    interval: window.setInterval }
  let tick: (() => void) | undefined
  window.setInterval = (callback, delay, ...args) => {
    tick = () => callback(...args)
    return original.interval.call(window, () => {}, delay)
  }
  const row = { ...mockGateways[0], id: 'slow-inventory' }
  let calls = 0
  gatewayApi.list = async () => [row]
  gatewayApi.hydrateRuntime = async rows => rows
  gatewayApi.refreshStatus = async () => { throw new Error('warming unavailable') }
  gatewayApi.hydrateToolInventory = async rows => {
    calls++
    await new Promise(resolve => setTimeout(resolve, 6_000))
    return rows.map(item => ({ ...item,
      discovery: { ...item.discovery, tools: [{ name: 'slow-tool', exposed: true, matched_by: '*' }] },
    }))
  }
  function Harness() {
    const result = useGateways(true, true)
    return React.createElement('span', null, result.data?.[0]?.discovery.tools[0]?.name ?? 'loading')
  }
  const view = await renderClient(React.createElement(SWRConfig,
    { value: { provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false } }, React.createElement(Harness)))
  try {
    for (let attempt = 0; attempt < 50 && calls === 0; attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.equal(calls, 1)
    await act(async () => {
      await new Promise(resolve => setTimeout(resolve, 5_000))
      tick!()
      await new Promise(resolve => setTimeout(resolve, 1_100))
    })
    assert.equal(view.container.textContent, 'slow-tool', 'the first slow response must publish')
    assert.equal(calls, 1, 'polling must not supersede an outstanding inventory request')
  } finally {
    await view.unmount()
    gatewayApi.list = original.list; gatewayApi.hydrateRuntime = original.runtime
    gatewayApi.hydrateToolInventory = original.inventory; gatewayApi.refreshStatus = original.refresh
    window.setInterval = original.interval
  }
})
