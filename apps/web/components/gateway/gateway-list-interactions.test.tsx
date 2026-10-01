import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'

const summary = {
  id: 'restricted', name: 'restricted', source: 'custom_gateway', configured: true, enabled: true,
  connected: true, discovered_tool_count: 1, exposed_tool_count: 0,
  config_summary: { transport: 'http', target: 'https://restricted.example/mcp', command: null, args: [] },
}
const detail = {
  config: { name: 'restricted', url: 'https://restricted.example/mcp', args: [],
    bearer_token_env: 'RESTRICTED_TOKEN', proxy_resources: true, proxy_prompts: true,
    proxy_mcp_ui: true, expose_tools: ['public.*'] },
  runtime: { name: 'restricted', connected: true, tool_count: 1, resource_count: 0, prompt_count: 0 },
}

async function mountList(handler: (action: string, params: Record<string, unknown>) => unknown) {
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
  const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store')
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (_input, init) => {
    const { action, params } = JSON.parse(String(init?.body ?? '{}'))
    const result = await handler(action, params ?? {})
    return result instanceof Response ? result : new Response(JSON.stringify(result), { status: 200, headers: { 'Content-Type': 'application/json' } })
  }) as typeof fetch
  const view = await renderClient(React.createElement(SWRConfig,
    { value: { provider: () => new Map(), dedupingInterval: 0, shouldRetryOnError: false } },
    React.createElement(SidebarProvider, null, React.createElement(GatewayListContent)))).catch(error => {
      globalThis.fetch = originalFetch
      if (error instanceof AggregateError) console.error(error.errors)
      throw error
    })
  const wait = async (assertion: () => void) => {
    let last: unknown
    for (let attempt = 0; attempt < 100; attempt++) {
      try { assertion(); return } catch (error) { last = error }
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    throw last
  }
  const click = async (label: string) => {
    const item = [...document.querySelectorAll<HTMLElement>('button,[role="menuitem"]')]
      .find(button => button.getAttribute('aria-label') === label || button.textContent?.trim() === label)
    assert.ok(item, `missing ${label}`)
    await act(async () => {
      if (item.hasAttribute('aria-haspopup')) item.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true, button: 0, pointerType: 'mouse' }) as unknown as Event)
      else item.click()
    })
  }
  return { ...view, window, wait, click, cleanup: async () => { await view.unmount(); globalThis.fetch = originalFetch } }
}

function defaultResponse(action: string) {
  if (action === 'gateway.list') return [summary]
  if (action === 'gateway.server.get') return summary
  if (action === 'gateway.get') return detail
  if (action === 'gateway.refresh_status') return {}
  if (action === 'gateway.code_mode.get') return { enabled: false }
  return []
}

test('list edit hydrates authoritative configuration before a display-only save', async () => {
  const writes: Array<Record<string, unknown>> = []
  let detailReads = 0
  const view = await mountList((action, params) => {
    if (action === 'gateway.get') detailReads++
    if (action === 'gateway.update') { writes.push(params); return detail }
    return defaultResponse(action)
  })
  try {
    await view.wait(() => assert.match(view.container.textContent ?? '', /restricted/))
    await view.click('More actions')
    await view.click('Edit server')
    await view.wait(() => assert.ok(document.querySelector('#display_name')))
    const { act } = await import('react')
    const labelInput = document.querySelector('#display_name')!
    await act(async () => {
      Object.getOwnPropertyDescriptor(view.window.HTMLInputElement.prototype, 'value')?.set?.call(labelInput, 'Friendly restricted')
      labelInput.dispatchEvent(new view.window.InputEvent('input', { bubbles: true, data: 'Friendly restricted' }) as unknown as Event)
    })
    await view.click('Save changes')
    await view.wait(() => assert.equal(writes.length, 1))
    assert.ok(detailReads > 0, 'the summary must be hydrated before editing')
    const patch = writes[0].patch as Record<string, unknown>
    assert.equal(patch.display_name, 'Friendly restricted')
    assert.equal(patch.bearer_token_env, 'RESTRICTED_TOKEN')
    assert.equal(patch.proxy_resources, true)
    assert.equal(patch.proxy_prompts, true)
    assert.equal(patch.proxy_mcp_ui, true)
    assert.equal('expose_tools' in patch, false, 'an untouched tool policy must be preserved')
  } finally { await view.cleanup() }
})

test('list import renders errors and skips from a successful API envelope', async () => {
  const view = await mountList((action) => {
    if (action === 'gateway.discover') return [{ name: 'broken', source_client: 'test', source_path: '/fixture.json', transport: 'stdio', command_preview: 'fixture', env_key_count: 0, already_configured: false }]
    if (action === 'gateway.import') return { imported: [], skipped: [{ name: 'removed', reason: 'tombstoned' }], errors: [{ name: 'broken', message: 'Missing command in /fixture.json' }] }
    return defaultResponse(action)
  })
  try {
    await view.click('Gateway actions, search and filters')
    await view.click('Scan MCP configs')
    await view.wait(() => assert.ok(document.querySelector('[aria-label="Import all MCP configs"]')))
    await view.click('Import all MCP configs')
    await view.wait(() => {
      assert.match(document.body.textContent ?? '', /Missing command in \/fixture.json/)
      assert.match(document.body.textContent ?? '', /removed.*previously removed.*restore/)
      assert.match(document.body.textContent ?? '', /0 imported.*1 skipped.*1 failed/)
    })
  } finally { await view.cleanup() }
})

test('failed authoritative edit fetch keeps the lossy summary out of the form', async () => {
  const view = await mountList((action) => action === 'gateway.get'
    ? new Response(JSON.stringify({ message: 'Configuration temporarily unavailable' }), { status: 503 })
    : defaultResponse(action))
  try {
    await view.wait(() => assert.match(view.container.textContent ?? '', /restricted/))
    await view.click('More actions')
    await view.click('Edit server')
    await view.wait(() => assert.match(document.querySelector('[role="alert"]')?.textContent ?? '', /Configuration temporarily unavailable/))
    assert.equal(document.querySelector('#name'), null)
  } finally { await view.cleanup() }
})

test('creating a server supersedes an outstanding edit hydration', async () => {
  let resolveDetail: (value: typeof detail) => void
  const pending = new Promise<typeof detail>(resolve => { resolveDetail = resolve })
  const view = await mountList(action => action === 'gateway.get' ? pending : defaultResponse(action))
  try {
    await view.wait(() => assert.match(view.container.textContent ?? '', /restricted/))
    await view.click('More actions')
    await view.click('Edit server')
    await view.wait(() => assert.match(document.body.textContent ?? '', /Loading configuration for restricted/))
    assert.equal(document.querySelector('#name'), null)
    await view.click('Gateway actions, search and filters')
    await view.click('Add server')
    await view.wait(() => assert.ok(document.querySelector('#name')))
    const { act } = await import('react')
    await act(async () => { resolveDetail(detail) })
    assert.equal((document.querySelector('#name') as HTMLInputElement).value, '')
  } finally { await view.cleanup() }
})

test('list edit preserves stdio arguments from the authoritative detail', async () => {
  const stdioSummary = { ...summary, config_summary: { transport: 'stdio', target: 'npx', command: 'npx', args: ['-y', 'fixture', '/path with spaces'] } }
  const stdioDetail = { ...detail, config: { name: 'restricted', command: 'npx', args: ['-y', 'fixture', '/path with spaces'], proxy_resources: true, proxy_prompts: true, proxy_mcp_ui: true } }
  let patch: Record<string, unknown> | undefined
  const view = await mountList((action, params) => {
    if (action === 'gateway.list') return [stdioSummary]
    if (action === 'gateway.server.get') return stdioSummary
    if (action === 'gateway.get') return stdioDetail
    if (action === 'gateway.update') { patch = params.patch as Record<string, unknown>; return stdioDetail }
    return defaultResponse(action)
  })
  try {
    await view.wait(() => assert.match(view.container.textContent ?? '', /restricted/))
    await view.click('More actions')
    await view.click('Edit server')
    await view.wait(() => assert.ok(document.querySelector('#command')))
    await view.click('Save changes')
    await view.wait(() => assert.ok(patch))
    assert.equal(patch?.command, 'npx')
    assert.deepEqual(patch?.args, ['-y', 'fixture', '/path with spaces'])
  } finally { await view.cleanup() }
})

for (const result of [
  { imported: [{ config: { name: 'added', enabled: false } }], skipped: [], errors: [{ name: 'broken', message: 'Cannot read config' }] },
  { imported: [], skipped: [{ name: 'existing', reason: 'already_configured' }], errors: [] },
]) {
  test(`import results persist across scan failure (${result.imported.length} imported, ${result.skipped.length} skipped)`, async () => {
    let scans = 0
    const view = await mountList(action => {
      if (action === 'gateway.discover') {
        scans++
        return scans === 1
          ? [{ name: 'fixture', source_client: 'test', source_path: '/fixture.json', transport: 'stdio', command_preview: 'fixture', env_key_count: 0, already_configured: false }]
          : new Response(JSON.stringify({ message: 'Scan offline' }), { status: 503 })
      }
      if (action === 'gateway.import') return result
      return defaultResponse(action)
    })
    try {
      await view.click('Gateway actions, search and filters')
      await view.click('Scan MCP configs')
      await view.wait(() => assert.ok(document.querySelector('[aria-label="Import all MCP configs"]')))
      await view.click('Import all MCP configs')
      await view.wait(() => assert.match(document.querySelector('[aria-label="MCP config import results"]')?.textContent ?? '',
        new RegExp(`${result.imported.length} imported, ${result.skipped.length} skipped, ${result.errors.length} failed`)))
      if (result.errors.length) assert.match(document.body.textContent ?? '', /Cannot read config/)
      if (result.skipped.length) assert.match(document.body.textContent ?? '', /existing: skipped.*already configured/)
    } finally { await view.cleanup() }
  })
}
