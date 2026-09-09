import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { PathnameContext, SearchParamsContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { __setBrowserSessionStateForTests, getBrowserSessionState } from '../../lib/auth/session-store'

const qualifiedProvider = { id: 'public', name: 'Public', enabled: true, sourceOrigins: ['mcp-registry', 'acp-registry', 'ard'], health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } }

test('closing a directly loaded inspector retains search, source, and feed without route navigation', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) {
    Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  }
  const query = 'q=review&provider=catalog&origin=ard&feed=new&artifactProvider=catalog&artifact=selected'
  window.happyDOM.setURL(`http://localhost/depot/?${query}`)
  const originalFetch = globalThis.fetch
  globalThis.fetch = async url => new Response(JSON.stringify(String(url) === '/v1/depot/providers' ? [{ ...qualifiedProvider, id: 'catalog' }] : { providers: [] }), { headers: { 'content-type': 'application/json' } })
  const { DepotPageContent } = await import('./depot-page-content')
  const navigations: string[] = []
  const router = { replace: (url: string) => navigations.push(url), push: (url: string) => navigations.push(url), prefetch: () => {} }
  const tree = (search: string) => <AppRouterContext.Provider value={router as never}>
    <PathnameContext.Provider value="/depot/"><SearchParamsContext.Provider value={new URLSearchParams(search)}>
      <DepotPageContent />
    </SearchParamsContext.Provider></PathnameContext.Provider>
  </AppRouterContext.Provider>
  const view = await renderClient(tree(query))
  try {
    const close = [...document.querySelectorAll('button')].find(node => node.textContent === 'Close')
    assert.ok(close)
    await act(async () => { close.click() })
    assert.equal(window.location.pathname + window.location.search, '/depot/?q=review&provider=catalog&origin=ard&feed=new')
    assert.deepEqual(navigations, [])
    assert.equal(window.history.state, null)
    // The browser test covers Next's native-history search-param integration.
    await view.rerender(tree(window.location.search))
    assert.equal(document.querySelector('[role="dialog"]'), null)
    const mcp = document.querySelector<HTMLButtonElement>('[aria-label="Filter source: MCP Registry"]')
    assert.ok(mcp)
    await act(async () => { mcp.click() })
    assert.equal(window.location.search, '?q=review&provider=catalog&origin=mcp-registry&feed=new')
    assert.deepEqual(navigations, [])
    const unsupported = document.querySelector<HTMLButtonElement>('[aria-label="GitHub: source filter unavailable"]')
    assert.ok(unsupported)
    await act(async () => { unsupported.click() })
    assert.equal(window.location.search, '?q=review&provider=catalog&origin=mcp-registry&feed=new')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    await window.happyDOM.close()
  }
})

test('switching to New discards a late Catalog response and retains feed in artifact links', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  window.happyDOM.setURL('http://localhost/depot')
  const originalFetch = globalThis.fetch
  let releaseCatalog!: (response: Response) => void
  let catalogStarted = false
  const requests: Array<Record<string, unknown>> = []
  const page = (feed?: string) => ({
    schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'epoch',
    items: [{ providerId: 'public', artifactId: feed ? 'new-artifact' : 'old-artifact', title: feed ? 'New result' : 'Stale catalog result', firstSeenAt: '2026-09-08T11:00:00Z' }],
    providerOutcomes: [{ providerId: 'public', state: 'exhausted' }], failures: [],
    coverageComplete: true, knownTotal: 1, totalIsExact: true, state: 'complete',
    ...(feed ? { feed, asOf: '2026-09-08T12:00:00Z', rankingVersion: 'new/v1', feedCoverage: [] } : {}),
  })
  const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } })
  globalThis.fetch = (async (url, init) => {
    if (String(url) !== '/v1/depot/discover') return json({ providers: [] })
    const body = JSON.parse(String(init?.body))
    requests.push(body)
    if (body.feed === 'new') return json(page('new'))
    catalogStarted = true
    return new Promise<Response>(resolve => { releaseCatalog = resolve })
  }) as typeof fetch
  const { DepotPageContent } = await import('./depot-page-content')
  let destination = ''
  const router = { replace: (url: string) => { destination = url }, push: () => {}, prefetch: () => {} }
  const tree = (query: string) => <AppRouterContext.Provider value={router as never}>
    <PathnameContext.Provider value="/depot"><SearchParamsContext.Provider value={new URLSearchParams(query)}>
      <DepotPageContent />
    </SearchParamsContext.Provider></PathnameContext.Provider>
  </AppRouterContext.Provider>
  const view = await renderClient(tree(''))
  const settle = async (predicate: () => boolean) => {
    for (let attempt = 0; attempt < 50 && !predicate(); attempt++) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.ok(predicate(), 'expected UI condition within bounded wait')
  }
  try {
    await settle(() => catalogStarted)
    assert.match(view.container.textContent ?? '', /Indexed/)
    const publishLink = [...view.container.querySelectorAll('a')].find(node => node.textContent === 'Publish Artifact')
    assert.ok(publishLink)
    assert.equal(new URL(publishLink.href).pathname, '/create')
    const button = [...view.container.querySelectorAll('button')].find(node => node.textContent === 'New')
    assert.ok(button)
    await act(async () => { button.click() })
    assert.equal(destination, '/depot?feed=new')
    window.history.replaceState({}, '', destination)
    await view.rerender(tree('feed=new'))
    await settle(() => view.container.textContent?.includes('New result') ?? false)
    assert.match(view.container.textContent ?? '', /New in 7 days/)
    assert.doesNotMatch(view.container.textContent ?? '', /Indexed/)
    await act(async () => { releaseCatalog(json(page())) })
    assert.doesNotMatch(view.container.textContent ?? '', /Stale catalog result/)
    const link = [...view.container.querySelectorAll('a')].find(node => node.href.includes('artifact=new-artifact'))
    assert.ok(link)
    const grid = link.closest('[class*="grid-cols-"]')
    assert.ok(grid)
    assert.match(grid.className, /repeat\(auto-fill,minmax\(min\(100%,268px\),1fr\)\)/)
    assert.doesNotMatch(grid.className, /grid-cols-3/)
    assert.equal(new URL(link.href).searchParams.get('feed'), 'new')
    assert.equal(requests.at(-1)?.feed, 'new')
    assert.equal(requests.at(-1)?.cursor, undefined)
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    await window.happyDOM.close()
  }
})

test('reselecting the active source popup option preserves loaded results', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) {
    Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  }
  const originalFetch = globalThis.fetch
  const { DepotPageContent } = await import('./depot-page-content')
  let discoverRequests = 0
  let providerReads = 0
  globalThis.fetch = (async url => {
    const origin = new URL(window.location.href).searchParams.get('origin') ?? undefined
    if (String(url) !== '/v1/depot/discover') {
      return new Response(JSON.stringify([{ ...qualifiedProvider, sourceOrigins: providerReads++ === 0 ? null : qualifiedProvider.sourceOrigins }]))
    }
    discoverRequests++
    return new Response(JSON.stringify({
      schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'epoch',
      items: [{ providerId: 'public', artifactId: 'retained', title: 'Retained result', sourceOrigin: origin }],
      providerOutcomes: [{ providerId: 'public', state: 'exhausted' }], failures: [],
      coverageComplete: true, knownTotal: 1, totalIsExact: true, state: 'complete', sourceOrigin: origin,
    }))
  }) as typeof fetch
  try {
    for (const origin of [undefined, 'ard']) {
      providerReads = 0
      window.happyDOM.setURL(`http://localhost/depot/${origin ? '?origin=ard' : ''}`)
      const view = await renderClient(<AppRouterContext.Provider value={{ replace: () => assert.fail('same selection must not navigate'), prefetch: () => {} } as never}>
        <PathnameContext.Provider value="/depot/"><SearchParamsContext.Provider value={new URLSearchParams(window.location.search)}>
          <DepotPageContent />
        </SearchParamsContext.Provider></PathnameContext.Provider>
      </AppRouterContext.Provider>)
      try {
        for (let attempt = 0; attempt < 50 && !view.container.textContent?.includes('Retained result'); attempt++) {
          await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
        }
        assert.match(view.container.textContent ?? '', /Retained result/)
        for (let attempt = 0; attempt < 50 && document.querySelector('[aria-label="Filter source: ARD"]')?.getAttribute('aria-disabled') === 'true'; attempt++) {
          await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
        }
        assert.ok(providerReads >= 2, 'discovery completion refreshes the initial unknown capability')
        assert.equal(document.querySelector('[aria-label="Filter source: ARD"]')?.getAttribute('aria-disabled'), null)
        const requestsBefore = discoverRequests
        const trigger = document.querySelector<HTMLButtonElement>('[aria-label="Kind and source filters"]')
        assert.ok(trigger)
        await act(async () => { trigger.click() })
        const option = [...document.querySelectorAll('button')].find(node => node.textContent === (origin ? 'ARD' : 'All origins'))
        assert.ok(option)
        await act(async () => { option.click() })
        assert.match(view.container.textContent ?? '', /Retained result/)
        assert.doesNotMatch(view.container.textContent ?? '', /Searching…/)
        assert.equal(discoverRequests, requestsBefore)
      } finally { await view.unmount() }
    }
  } finally {
    globalThis.fetch = originalFetch
    await window.happyDOM.close()
  }
})

test('Add to Library refuses mutation after the session changes during preflight', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'self', { value: window, configurable: true })
  for (const name of ['Event', 'NodeFilter', 'HTMLInputElement'] as const) {
    Object.defineProperty(globalThis, name, { value: window[name], configurable: true })
  }
  window.happyDOM.setURL('http://localhost/depot')
  const originalFetch = globalThis.fetch
  const originalSession = getBrowserSessionState()
  const actions: string[] = []
  let releaseLibrary: ((response: Response) => void) | undefined
  const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' } })
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'first' }, expiresAt: 42, csrfToken: 'csrf-first', projectId: 'first-project' })
  globalThis.fetch = async (url, init) => {
    if (String(url) === '/v1/depot/discover') return json({
      schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'epoch',
      items: [{ providerId: 'public', artifactId: 'artifact', kind: 'skill', title: 'Import target', currentRevisionId: 'revision' }],
      providerOutcomes: [{ providerId: 'public', state: 'exhausted' }], failures: [],
      coverageComplete: true, knownTotal: 1, totalIsExact: true, state: 'complete',
    })
    const body = init?.body ? JSON.parse(String(init.body)) : {}
    if (!body.action) return json({ providers: [] })
    actions.push(body.action)
    if (body.action === 'artifacts.depot_membership') return json({ library_version: 1, items: body.params.items.map((item: object) => ({ ...item, status: 'absent' })) })
    if (body.action === 'artifacts.list_connections') return json({ connections: [{ id: 'public' }] })
    if (body.action === 'artifacts.list') return new Promise(resolve => { releaseLibrary = resolve })
    throw new Error(`Unexpected action: ${body.action}`)
  }
  const { DepotPageContent } = await import('./depot-page-content')
  const router = { replace: () => {}, push: () => {}, prefetch: () => {} }
  const view = await renderClient(<AppRouterContext.Provider value={router as never}>
    <PathnameContext.Provider value="/depot"><SearchParamsContext.Provider value={new URLSearchParams()}>
      <DepotPageContent />
    </SearchParamsContext.Provider></PathnameContext.Provider>
  </AppRouterContext.Provider>)
  const settle = async (predicate: () => boolean) => {
    for (let attempt = 0; attempt < 50 && !predicate(); attempt++) await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    assert.ok(predicate(), 'expected UI condition within bounded wait')
  }
  try {
    await settle(() => view.container.textContent?.includes('Import target') ?? false)
    const button = [...view.container.querySelectorAll('button')].find(node => node.textContent === 'Add to Library')
    assert.ok(button)
    await act(async () => { button.click(); button.click() })
    await settle(() => Boolean(releaseLibrary))
    assert.equal(actions.filter(action => action === 'artifacts.list').length, 1)
    __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'second' }, expiresAt: 42, csrfToken: 'csrf-second', projectId: 'second-project' })
    await act(async () => { releaseLibrary!(json({ library_version: 1 })) })
    await settle(() => !button.disabled)
    assert.ok(!actions.includes('artifacts.import'))
    const send = view.container.querySelector<HTMLButtonElement>('button[title="Send to Labby"]')
    assert.ok(send)
    await act(async () => { send.click() })
    await settle(() => document.querySelector('[role="dialog"]')?.textContent?.includes('Add this exact Skill revision') ?? false)
    assert.ok(!actions.includes('artifacts.import'))
    assert.ok(!actions.includes('artifacts.activate'))
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests(originalSession)
    await window.happyDOM.close()
  }
})
