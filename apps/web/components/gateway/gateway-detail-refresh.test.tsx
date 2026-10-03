import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'

process.env.NEXT_PUBLIC_PROTECTED_MCP_HOST = 'mcp.example.test'

for (const initialFailure of [false, true]) {
 test(initialFailure ? 'detail shows the full error when no successful snapshot exists' : 'detail keeps the last successful snapshot through a failed refresh and retries', async () => {
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
  let failDetail = initialFailure
  let detailRequests = 0
  let refresh: (() => Promise<unknown>) | undefined
  function RevalidationControl() {
    const { mutate } = useSWRConfig()
    refresh = () => mutate('/gateways/refresh-detail')
    return null
  }
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_input, init) => {
    const { action } = JSON.parse(String(init?.body ?? '{}'))
    let result: unknown = []
    if (action === 'gateway.server.get') {
      detailRequests++
      if (failDetail) return new Response(JSON.stringify({ message: 'Temporary status outage' }), { status: 503 })
      result = { id: 'refresh-detail', name: 'refresh-detail', source: 'custom_gateway', configured: true, enabled: true }
    } else if (action === 'gateway.get') {
      result = {
        config: { name: 'refresh-detail', display_name: 'Retained server', url: 'https://example.test/mcp', args: [], proxy_resources: true, proxy_prompts: true, proxy_mcp_ui: true },
        runtime: { name: 'refresh-detail', connected: true, tool_count: 0, resource_count: 0, prompt_count: 0 },
      }
    } else if (action === 'gateway.usage.metrics') {
      result = { window_total_calls: 0, total_calls: 0, error_calls: 0, avg_elapsed_ms: 0, p50_elapsed_ms: 0, p95_elapsed_ms: 0, p99_elapsed_ms: 0, distinct_tools: 0, distinct_actors: 0, peak_per_min: 0, top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [], upstreams: [], hourly: [], timeseries: [], facets: { tools: [], actors: [], upstreams: [], outcomes: [] } }
    } else if (action === 'gateway.usage.calls') result = { calls: [], total: 0 }
    return new Response(JSON.stringify(result), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }) as typeof fetch
  const router = { bfcacheId: 'test-entry', back() {}, forward() {}, refresh() {}, hmrRefresh() {}, push() {}, prefetch() {}, replace() {} }
  const view = await renderClient(React.createElement(SWRConfig,
    { value: { provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false } },
    React.createElement(React.Fragment, null, React.createElement(RevalidationControl),
      React.createElement(AppRouterContext.Provider, { value: router },
        React.createElement(SearchParamsContext.Provider, { value: new URLSearchParams('id=refresh-detail') },
          React.createElement(SidebarProvider, null, React.createElement(GatewayDetailContent, { gatewayId: 'refresh-detail' })))))))
  async function wait(assertion: () => void) {
    let last: unknown
    for (let attempt = 0; attempt < 150; attempt++) {
      try { assertion(); return } catch (error) { last = error }
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    throw last
  }
  try {
    if (initialFailure) {
      await wait(() => assert.match(view.container.textContent ?? '', /Failed to load server/))
      assert.match(view.container.textContent ?? '', /Temporary status outage/)
      assert.doesNotMatch(view.container.textContent ?? '', /Connection status may be out of date/)
      assert.equal(document.querySelector('[aria-label="Edit server"]'), null)
      return
    }
    await wait(() => assert.ok(document.querySelector('[aria-label="Edit server"]')))
    assert.match(view.container.textContent ?? '', /Retained server/)
    failDetail = true
    await act(async () => { await refresh?.().catch(() => undefined) })
    await wait(() => assert.match(view.container.textContent ?? '', /Connection status may be out of date/))
    assert.doesNotMatch(view.container.textContent ?? '', /Failed to load server/)
    assert.ok(document.querySelector('[aria-label="Edit server"]'), 'cached detail controls remain mounted')
    failDetail = false
    const retry = [...view.container.querySelectorAll('button')].find(button => button.textContent?.trim() === 'Retry connection status')
    assert.ok(retry)
    await act(async () => retry.click())
    await wait(() => assert.doesNotMatch(view.container.textContent ?? '', /Connection status may be out of date/))
    assert.ok(detailRequests >= 3, 'retry must fetch current server detail')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
})

}
