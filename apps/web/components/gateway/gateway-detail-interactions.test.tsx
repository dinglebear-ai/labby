import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'

process.env.NEXT_PUBLIC_PROTECTED_MCP_HOST = 'mcp.example.test'

for (const routeFails of [false, true]) {
  test(`renamed detail ${routeFails ? 'compensates the returned identity on route failure' : 'navigates only after the route commits'}`, async () => {
    const window = installTestDom()
    Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
    for (const key of ['NodeFilter', 'HTMLInputElement', 'InputEvent'] as const) {
      Object.defineProperty(globalThis, key, { configurable: true, value: window[key] })
    }
    const React = await import('react')
    const { act } = React
    const { SWRConfig } = await import('swr')
    const { AppRouterContext } = await import('next/dist/shared/lib/app-router-context.shared-runtime')
    const { SearchParamsContext } = await import('next/dist/shared/lib/hooks-client-context.shared-runtime')
    const { SidebarProvider } = await import('../ui/sidebar')
    const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
    const { GatewayDetailContent } = await import('./gateway-detail-content')
    const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store')
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    const requests: Array<{ action: string; params: Record<string, unknown> }> = []
    const navigations: string[] = []
    let currentId = 'old-id'
    const backendView = () => ({
      config: { name: currentId, display_name: 'Original label', url: 'https://example.test/mcp', args: [], proxy_resources: true, proxy_prompts: true, proxy_mcp_ui: true },
      runtime: { name: currentId, connected: true, tool_count: 0, resource_count: 0, prompt_count: 0 },
    })
    const originalFetch = globalThis.fetch
    globalThis.fetch = (async (_input, init) => {
      const request = JSON.parse(String(init?.body ?? '{}'))
      requests.push(request)
      const { action, params } = request
      let result: unknown = []
      if (action === 'gateway.server.get') {
        if (params.id !== currentId) return new Response(JSON.stringify({ message: 'not found' }), { status: 404 })
        result = { id: currentId, name: currentId, source: 'custom_gateway', configured: true, enabled: true }
      } else if (action === 'gateway.get') result = backendView()
      else if (action === 'gateway.update') {
        assert.equal(params.name, currentId, 'writes must address the ID currently accepted by the backend')
        currentId = params.patch.name
        result = backendView()
      } else if (action === 'gateway.protected_route.add') {
        assert.equal(params.route.upstream, 'new-id')
        assert.deepEqual(navigations, [], 'navigation must wait for the protected-route write')
        if (routeFails) return new Response(JSON.stringify({ message: 'Route write failed' }), { status: 503 })
        result = params.route
      } else if (action === 'gateway.usage.metrics') {
        result = { window_total_calls: 0, total_calls: 0, error_calls: 0,
          avg_elapsed_ms: 0, p50_elapsed_ms: 0, p95_elapsed_ms: 0, p99_elapsed_ms: 0,
          distinct_tools: 0, distinct_actors: 0, peak_per_min: 0,
          top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [],
          upstreams: [], hourly: [], timeseries: [],
          facets: { tools: [], actors: [], upstreams: [], outcomes: [] } }
      } else if (action === 'gateway.usage.calls') {
        result = { calls: [], total: 0 }
      }
      return new Response(JSON.stringify(result), { status: 200, headers: { 'Content-Type': 'application/json' } })
    }) as typeof fetch
    const router = { bfcacheId: 'test-entry', back() {}, forward() {}, refresh() {}, hmrRefresh() {}, push() {}, prefetch() {}, replace(href: string) { navigations.push(href) } }
    const view = await renderClient(React.createElement(SWRConfig,
      { value: { provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false } },
      React.createElement(AppRouterContext.Provider, { value: router },
        React.createElement(SearchParamsContext.Provider, { value: new URLSearchParams('id=old-id&tab=settings') },
          React.createElement(SidebarProvider, null, React.createElement(GatewayDetailContent, { gatewayId: 'old-id' }))))))
    const wait = async (assertion: () => void) => {
      let last: unknown
      for (let attempt = 0; attempt < 150; attempt++) {
        try { assertion(); return } catch (error) { last = error }
        await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
      }
      throw last
    }
    const setInput = async (selector: string, value: string) => {
      const input = document.querySelector(selector)
      assert.ok(input)
      await act(async () => {
        Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set?.call(input, value)
        input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
      })
    }
    try {
      await wait(() => assert.ok(document.querySelector('[aria-label="Edit server"]')))
      await act(async () => (document.querySelector('[aria-label="Edit server"]') as HTMLElement).click())
      await wait(() => assert.ok(document.querySelector('#name')))
      await setInput('#name', 'new-id')
      await setInput('#protected-public-path', '/named/mcp')
      const save = [...document.querySelectorAll('button')].find(button => button.textContent?.trim() === 'Save changes')
      assert.ok(save)
      await act(async () => save.click())
      if (routeFails) {
        await wait(() => assert.equal(requests.filter(row => row.action === 'gateway.update').length, 2))
        const writes = requests.filter(row => row.action === 'gateway.update')
        assert.equal(writes[1].params.name, 'new-id')
        assert.equal((writes[1].params.patch as Record<string, unknown>).name, 'old-id')
        assert.equal((writes[1].params.patch as Record<string, unknown>).display_name, 'Original label')
        assert.deepEqual(navigations, [])
        assert.equal(currentId, 'old-id')
      } else {
        await wait(() => assert.equal(navigations.length, 1))
        const url = new URL(navigations[0], 'https://console.example')
        assert.equal(url.pathname, '/gateway/')
        assert.equal(url.searchParams.get('id'), 'new-id')
        assert.equal(url.searchParams.get('tab'), 'settings')
        assert.equal(currentId, 'new-id')
      }
      assert.doesNotMatch(view.container.textContent ?? '', /Failed to load server/)
    } finally {
      await view.unmount()
      globalThis.fetch = originalFetch
    }
  })
}
