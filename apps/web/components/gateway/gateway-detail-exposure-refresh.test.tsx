import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'

process.env.NEXT_PUBLIC_PROTECTED_MCP_HOST = 'mcp.example.test'

test('exposure draft survives changed remote catalog and cancel adopts current snapshot', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  const React = await import('react')
  const { act } = React
  const { SWRConfig, useSWRConfig } = await import('swr')
  const { AppRouterContext } = await import('next/dist/shared/lib/app-router-context.shared-runtime')
  const { SearchParamsContext } = await import('next/dist/shared/lib/hooks-client-context.shared-runtime')
  const { SidebarProvider } = await import('../ui/sidebar')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const { GatewayDetailContent } = await import('./gateway-detail-content')
  const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store')
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  let remoteChanged = false
  let detailRequests = 0
  let refresh: (() => Promise<unknown>) | undefined
  function RevalidationControl() {
    const { mutate } = useSWRConfig()
    refresh = () => mutate('/gateways/refresh-detail')
    return null
  }
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_input, init) => {
    const { action, params } = JSON.parse(String(init?.body ?? '{}'))
    let result: unknown = []
    if (action === 'gateway.server.get') {
      detailRequests++
      result = { id: params.id, name: params.id, source: 'custom_gateway', configured: true, enabled: true }
    } else if (action === 'gateway.get') {
      result = {
        config: { name: params.name, display_name: 'Retained server', url: 'https://example.test/mcp', args: [], proxy_resources: true, proxy_prompts: true, proxy_mcp_ui: true },
        runtime: { name: params.name, connected: true, tool_count: 0, resource_count: 0, prompt_count: 0 },
      }
    } else if (action === 'gateway.discovered_tools') {
      result = params.name === 'other-detail' ? [{ name: 'gamma', exposed: true, matched_by: '*' }] : [{ name: 'alpha', exposed: true, matched_by: '*' }, ...(remoteChanged ? [{ name: 'beta', exposed: true, matched_by: '*' }] : [])]
    } else if (action === 'gateway.usage.metrics') {
      result = { window_total_calls: 0, total_calls: 0, error_calls: 0, avg_elapsed_ms: 0, p50_elapsed_ms: 0, p95_elapsed_ms: 0, p99_elapsed_ms: 0, distinct_tools: 0, distinct_actors: 0, peak_per_min: 0, top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [], upstreams: [], hourly: [], timeseries: [], facets: { tools: [], actors: [], upstreams: [], outcomes: [] } }
    } else if (action === 'gateway.usage.calls') result = { calls: [], total: 0 }
    return new Response(JSON.stringify(result), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }) as typeof fetch
  const router = { bfcacheId: 'test-entry', back() {}, forward() {}, refresh() {}, hmrRefresh() {}, push() {}, prefetch() {}, replace() {} }
  const cache = new Map()
  const tree = (gatewayId: string) => React.createElement(SWRConfig,
    { value: { provider: () => cache, dedupingInterval: 0, shouldRetryOnError: false } },
    React.createElement(React.Fragment, null, React.createElement(RevalidationControl),
      React.createElement(AppRouterContext.Provider, { value: router },
        React.createElement(SearchParamsContext.Provider, { value: new URLSearchParams('id=refresh-detail') },
          React.createElement(SidebarProvider, null, React.createElement(GatewayDetailContent, { gatewayId }))))))
  const view = await renderClient(tree('refresh-detail'))
  async function wait(assertion: () => void) {
    let last: unknown
    for (let attempt = 0; attempt < 150; attempt++) {
      try { assertion(); return } catch (error) { last = error }
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    throw last
  }
  try {
    await wait(() => assert.ok(document.querySelector('[aria-label="Open catalog"]')))
    const click = async (element: Element | null | undefined) => {
      assert.ok(element)
      await act(async () => (element as HTMLElement).click())
    }
    await click(document.querySelector('[aria-label="Open catalog"]'))
    await click([...view.container.querySelectorAll('button')].find(button => button.textContent?.trim() === 'Exposure editor'))
    await click(document.querySelector('[aria-label="Manage tools"]'))
    await click(document.querySelector('[aria-label="Select alpha"]'))
    await click([...view.container.querySelectorAll('button')].find(button => button.textContent?.trim() === 'Disable selected'))
    assert.match(document.querySelector('[aria-label="Select alpha"]')?.closest('tr')?.textContent ?? '', /Off/)
    remoteChanged = true
    await act(async () => { await refresh?.() })
    await wait(() => assert.match(view.container.textContent ?? '', /changed while you were editing/i))
    assert.equal(document.querySelector('[aria-label="Manage tools"]')?.getAttribute('aria-pressed'), 'true')
    assert.match(document.querySelector('[aria-label="Select alpha"]')?.closest('tr')?.textContent ?? '', /Off/)
    assert.ok(document.querySelector('[aria-label="Select beta"]'), 'new remote catalog row remains editable')
    await click([...view.container.querySelectorAll('button')].find(button => button.textContent?.trim() === 'Cancel'))
    assert.equal(document.querySelector('[aria-label="Manage tools"]')?.getAttribute('aria-pressed'), 'false')
    assert.doesNotMatch(view.container.textContent ?? '', /changed while you were editing/i)
    await click(document.querySelector('[aria-label="Manage tools"]'))
    assert.match(document.querySelector('[aria-label="Select alpha"]')?.closest('tr')?.textContent ?? '', /On/)
    assert.match(document.querySelector('[aria-label="Select beta"]')?.closest('tr')?.textContent ?? '', /On/)
    await click(document.querySelector('[aria-label="Select alpha"]'))
    await click([...view.container.querySelectorAll('button')].find(button => button.textContent?.trim() === 'Disable selected'))
    await view.rerender(tree('other-detail'))
    await wait(() => assert.match(view.container.textContent ?? '', /Gamma/))
    assert.equal(document.querySelector('[aria-label="Manage tools"]')?.getAttribute('aria-pressed'), 'false')
    assert.doesNotMatch(view.container.textContent ?? '', /changed while you were editing/i)
    await click(document.querySelector('[aria-label="Manage tools"]'))
    assert.ok(document.querySelector('[aria-label="Select gamma"]'))
    assert.equal(document.querySelector('[aria-label="Select alpha"]'), null)
    assert.ok(detailRequests >= 3, 'refresh and changing gateway fetch current snapshots')

  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
})
