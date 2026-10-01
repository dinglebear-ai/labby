import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'
import type { CapabilityObservation } from '../../lib/types/gateway.ts'

const window = installTestDom()

async function mountProbeView(kind: 'list' | 'detail', observation?: CapabilityObservation) {
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  for (const key of ['NodeFilter', 'HTMLInputElement', 'InputEvent'] as const) Object.defineProperty(globalThis, key, { configurable: true, value: window[key] })
  const React = await import('react')
  const { act } = React
  const { SWRConfig } = await import('swr')
  const { AppRouterContext } = await import('next/dist/shared/lib/app-router-context.shared-runtime')
  const { SearchParamsContext } = await import('next/dist/shared/lib/hooks-client-context.shared-runtime')
  const { SidebarProvider } = await import('../ui/sidebar')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const { GatewayListContent } = await import('./gateway-list-content')
  const { GatewayDetailContent } = await import('./gateway-detail-content')
  const { toast } = await import('sonner')
  const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store')
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const initialToasts = toast.getHistory().length
  const probes: Array<{ name: string; signal?: AbortSignal | null; resolve: (value: Response) => void }> = []
  const server = (name: string) => ({ id: name, name, source: 'custom_gateway', configured: true, enabled: true, connected: true, config_summary: { transport: 'http', target: 'https://fixture.example/mcp' }, capability_observation: observation })
  const runtime = (name: string) => ({ name, connected: true, tool_count: 0, resource_count: 0, prompt_count: 0, capability_observation: observation })
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_input, init) => {
    const { action, params = {} } = JSON.parse(String(init?.body ?? '{}'))
    // Deliberately ignore abort so the UI must fence a transport's late completion.
    if (action === 'gateway.test') return new Promise<Response>(resolve => { probes.push({ name: params.name, signal: init?.signal, resolve }) })
    let result: unknown = []
    if (action === 'gateway.list') result = ['alpha', 'beta'].map(server)
    else if (action === 'gateway.server.get') result = server(params.id)
    else if (action === 'gateway.get') result = { config: { name: params.name, enabled: true, url: 'https://fixture.example/mcp', args: [] }, runtime: runtime(params.name) }
    else if (action === 'gateway.mcp.list') result = ['alpha', 'beta'].map(name => ({ ...server(name), ...runtime(name) }))
    else if (action === 'gateway.code_mode.get') result = { enabled: false }
    else if (action === 'gateway.usage.calls') result = { calls: [], total: 0 }
    else if (action === 'gateway.usage.metrics') result = { window_total_calls: 0, total_calls: 0, error_calls: 0, avg_elapsed_ms: 0, p50_elapsed_ms: 0, p95_elapsed_ms: 0, p99_elapsed_ms: 0, distinct_tools: 0, distinct_actors: 0, peak_per_min: 0, top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [], upstreams: [], hourly: [], timeseries: [], facets: { tools: [], actors: [], upstreams: [], outcomes: [] } }
    return new Response(JSON.stringify(result), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }) as typeof fetch
  const router = { bfcacheId: 'probe-test', back() {}, forward() {}, refresh() {}, hmrRefresh() {}, push() {}, prefetch() {}, replace() {} }
  const cache = new Map()
  const element = (id: string) => React.createElement(SWRConfig, { value: { provider: () => cache, dedupingInterval: 0, shouldRetryOnError: false } }, React.createElement(AppRouterContext.Provider, { value: router }, React.createElement(SearchParamsContext.Provider, { value: new URLSearchParams(`id=${id}`) }, React.createElement(SidebarProvider, null, kind === 'list' ? React.createElement(GatewayListContent) : React.createElement(GatewayDetailContent, { gatewayId: id })))))
  const view = await renderClient(element('alpha'))
  const wait = async (assertion: () => void) => {
    let last: unknown
    for (let attempt = 0; attempt < 150; attempt++) {
      try { assertion(); return } catch (error) { last = error }
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    throw last
  }
  const click = async (name: string) => {
    const button = [...document.querySelectorAll<HTMLElement>('button,[role="menuitem"]')].find(item => item.getAttribute('aria-label') === name || item.textContent?.trim() === name)
    assert.ok(button, `missing ${name}`)
    await act(async () => {
      if (button.hasAttribute('aria-haspopup')) button.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, button: 0, pointerType: 'mouse' }) as unknown as Event)
      else button.click()
    })
  }
  const start = async (name = 'alpha') => {
    if (kind === 'list') {
      await wait(() => assert.ok([...view.container.querySelectorAll('[data-gwrow], article')].find(item => [...item.querySelectorAll('a')].some(link => link.textContent === name))))
      const row = [...view.container.querySelectorAll('[data-gwrow], article')].find(item => [...item.querySelectorAll('a')].some(link => link.textContent === name))!
      const trigger = [...row.querySelectorAll<HTMLElement>('button')].find(item => item.textContent?.trim() === 'More actions' || item.getAttribute('aria-label') === `More actions for ${name}`)!
      await act(async () => trigger.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, button: 0, pointerType: 'mouse' }) as unknown as Event))
      await click('Test connection')
    } else await click('Test server')
  }
  const complete = async (index: number, failure = false) => { await act(async () => { probes[index].resolve(new Response(JSON.stringify(failure ? { message: 'Obsolete probe failure' } : runtime(probes[index].name)), { status: failure ? 503 : 200 })) }) }
  let unmounted = false
  const unmount = async () => { if (!unmounted) { await view.unmount(); unmounted = true } }
  return { ...view, wait, click, start, complete, probes, switchTo: (id: string) => view.rerender(element(id)), unmount, newToasts: () => toast.getHistory().slice(initialToasts), cleanup: async () => { await unmount(); globalThis.fetch = originalFetch } }
}

for (const fails of [false, true]) {
  test(`list probes keep beta's result when an earlier alpha ${fails ? 'failure' : 'success'} ignores cancellation`, async () => {
  const view = await mountProbeView('list')
  try {
    await view.wait(() => assert.ok(view.container.querySelector('[aria-label="Test server"], [aria-label="More actions"]')))
    await view.start(); await view.wait(() => assert.equal(view.probes.length, 1))
    await view.start('beta'); await view.wait(() => assert.equal(view.probes.length, 2))
    await view.complete(1)
    await view.wait(() => assert.match(document.querySelector('[role="dialog"]')?.textContent ?? '', /Test results for beta/))
    await view.complete(0, fails)
    assert.match(document.querySelector('[role="dialog"]')?.textContent ?? '', /Test results for beta/)
    assert.doesNotMatch(document.querySelector('[role="dialog"]')?.textContent ?? '', /Obsolete probe failure/)
    assert.equal(view.newToasts().some(item => 'title' in item && item.title === 'Obsolete probe failure'), false)
    assert.equal(view.probes[0].signal?.aborted, true)
  } finally { await view.cleanup() }
})

}
for (const kind of ['list', 'detail'] as const) {
  test(`${kind} unmount cancels the probe and suppresses late errors`, async () => {
    const view = await mountProbeView(kind)
    try {
      await view.wait(() => assert.ok(view.container.querySelector('[aria-label="Test server"], [aria-label="More actions"]')))
      await view.start(); await view.wait(() => assert.equal(view.probes.length, 1))
      await view.unmount()
      const toastCount = view.newToasts().length
      await view.complete(0, true)
      assert.equal(view.newToasts().length, toastCount)
      assert.equal(view.probes[0].signal?.aborted, true)
    } finally { await view.cleanup() }
  })
}

test('detail gateway switches retire old probes without disturbing the new loading state', async () => {
  const view = await mountProbeView('detail')
  try {
    await view.wait(() => assert.ok(view.container.querySelector('[aria-label="Test server"]')))
    await view.start(); await view.wait(() => assert.equal(view.probes.length, 1))
    await view.switchTo('beta')
    await view.wait(() => assert.ok(view.container.querySelector('[aria-label="Test server"]')))
    assert.equal(view.container.querySelector<HTMLButtonElement>('[aria-label="Test server"]')?.disabled, false)
    await view.start(); await view.wait(() => assert.equal(view.probes.length, 2))
    await view.complete(0, true)
    assert.equal(view.container.querySelector<HTMLButtonElement>('[aria-label="Test server"]')?.disabled, true)
    assert.equal(document.querySelector('[role="dialog"]'), null)
    await view.complete(1)
    await view.wait(() => assert.match(document.querySelector('[role="dialog"]')?.textContent ?? '', /Test results for beta/))
    assert.equal(view.newToasts().some(item => 'title' in item && item.title === 'Obsolete probe failure'), false)
    assert.equal(view.probes[0].signal?.aborted, true)
  } finally { await view.cleanup() }
})

for (const [state, label, discovered, exposed] of [ ['unknown', 'Not discovered', null, null], ['failed', 'Discovery failed', null, null], ['stale', '1/2 · stale', 2, 1], ['known', '0/0', 0, 0] ] as const) {
  test(`detail skills overview honors ${state} observations`, async () => {
    const unknown = { state: 'unknown' as const, discovered: null, exposed: null }
    const view = await mountProbeView('detail', { scope: 'credential', tools: unknown, resources: unknown, prompts: unknown, skills: { state, discovered, exposed } })
    try {
      await view.wait(() => assert.ok(view.container.querySelector('[aria-label="Test server"]')))
      const skillsLabel = [...view.container.querySelectorAll('span')].find(item => item.textContent === 'Skills')
      assert.ok(skillsLabel)
      assert.ok(skillsLabel.parentElement?.textContent?.includes(label), `skills row must show ${label}, got ${skillsLabel.parentElement?.textContent}`)
    } finally { await view.cleanup() }
  })
}
