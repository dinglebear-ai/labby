import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'

process.env.NEXT_PUBLIC_MOCK_DATA = 'true'

test('mock list edit hydrates mock overrides without making a real API request', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  for (const key of ['NodeFilter', 'HTMLInputElement', 'InputEvent'] as const) {
    Object.defineProperty(globalThis, key, { configurable: true, value: window[key] })
  }
  const React = await import('react')
  const { act } = React
  const { SWRConfig } = await import('swr')
  const { SidebarProvider } = await import('../ui/sidebar')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const { GatewayListContent } = await import('./gateway-list-content')
  const { setMockGatewayOverride } = await import('../../lib/api/mock-gateway-overrides')
  const { mockGateways } = await import('../../lib/api/mock-data')
  const row = mockGateways.find(row => row.name === 'github-server')!
  setMockGatewayOverride(row.id, { config: { ...row.config, bearer_token_env: 'MOCK_EDIT_TOKEN' } })
  const originalFetch = globalThis.fetch
  const requests: Array<{ path: string; action?: string }> = []
  globalThis.fetch = (async (path, init) => { requests.push({ path: String(path), action: JSON.parse(String(init?.body ?? '{}')).action }); throw new Error('Unexpected real API request') }) as typeof fetch
  const view = await renderClient(React.createElement(SWRConfig,
    { value: { provider: () => new Map(), shouldRetryOnError: false } },
    React.createElement(SidebarProvider, null, React.createElement(GatewayListContent))))
  const wait = async (assertion: () => void) => {
    let last: unknown
    for (let attempt = 0; attempt < 100; attempt++) {
      try { assertion(); return } catch (error) { last = error }
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    throw last
  }
  try {
    await wait(() => assert.ok([...document.querySelectorAll('[data-gwrow]')].some(item => item.querySelector('[aria-label="Select github-server"]') != null)))
    const tableRow = [...document.querySelectorAll('[data-gwrow]')].find(item => item.querySelector('[aria-label="Select github-server"]') != null)!
    const menu = [...tableRow.querySelectorAll('button')].find(item => item.textContent?.trim() === 'More actions')!
    await act(async () => menu.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, button: 0, pointerType: 'mouse' }) as unknown as Event))
    const edit = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(item => item.textContent?.trim() === 'Edit server')!
    assert.ok(edit)
    await act(async () => edit.click())
    await wait(() => assert.ok(document.querySelector('#name')))
    assert.equal((document.querySelector('#name') as HTMLInputElement).value, row.name)
    const { fetchGateway } = await import('../../lib/hooks/use-gateways')
    let hydrated: Awaited<ReturnType<typeof fetchGateway>> | undefined
    await act(async () => { hydrated = await fetchGateway(row.id) })
    assert.equal(hydrated?.config.bearer_token_env, 'MOCK_EDIT_TOKEN')
    assert.deepEqual(requests.filter(request => request.action === 'gateway.server.get' || request.action === 'gateway.get'), [])
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
})

test('mock edit hydration respects cancellation before publishing its configuration', async () => {
  installTestDom()
  const { fetchGateway } = await import('../../lib/hooks/use-gateways')
  const { mockGateways } = await import('../../lib/api/mock-data')
  const controller = new AbortController()
  const pending = fetchGateway(mockGateways[0].id, controller.signal)
  controller.abort()
  await assert.rejects(pending, error => error instanceof DOMException && error.name === 'AbortError')
})

test('mock combined saves reject Team route names and invalid renamed upstream references atomically', async () => {
  installTestDom()
  const React = await import('react')
  const { SWRConfig } = await import('swr')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const { useGatewayMutations, fetchGateway } = await import('../../lib/hooks/use-gateways')
  const { mockGateways } = await import('../../lib/api/mock-data')
  const row = mockGateways.find(row => row.name === 'github-server')!
  let mutations: ReturnType<typeof useGatewayMutations> | undefined
  function Harness() { mutations = useGatewayMutations(); return null }
  const view = await renderClient(React.createElement(SWRConfig, { value: { provider: () => new Map() } }, React.createElement(Harness)))
  const route = { name: 'team:alpha:route', enabled: true, public_host: 'mcp.example.net', public_path: '/atomic-team', upstream: row.name, scopes: ['mcp:read'] }
  try {
    const before = await fetchGateway(row.id)
    await React.act(async () => {
      await assert.rejects(mutations!.updateGateway(row.id, {
        config: { bearer_token_env: 'MUST_NOT_SAVE' },
        protected_route: { operation: 'upsert', route },
      }), /Team/)
    })
    assert.equal((await fetchGateway(row.id)).config.bearer_token_env, before.config.bearer_token_env)
    for (const existingName of [undefined, 'tools']) {
      await React.act(async () => {
        await assert.rejects(mutations!.updateGateway(row.id, {
          config: { bearer_token_env: 'MUST_NOT_SAVE' },
          protected_route: { operation: 'upsert', name: existingName, route: { ...route, name: ' team:alpha:route ' } },
        }), /Team/)
      })
      assert.equal((await fetchGateway(row.id)).config.bearer_token_env, before.config.bearer_token_env)
    }
    await React.act(async () => {
      await assert.rejects(mutations!.updateGateway(row.id, {
        name: 'renamed-github', config: { bearer_token_env: 'MUST_NOT_SAVE' },
        protected_route: { operation: 'upsert', route: { ...route, name: 'atomic-renamed', upstream: row.name } },
      }), /upstream/)
    })
    assert.equal((await fetchGateway(row.id)).config.bearer_token_env, before.config.bearer_token_env)
  } finally { await view.unmount() }
})
