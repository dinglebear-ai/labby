import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { SearchParamsContext, PathnameContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'

const dom = installTestDom()
Object.defineProperty(globalThis, 'self', { value: dom, configurable: true })
Object.defineProperty(globalThis, 'NodeFilter', { value: dom.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: dom.HTMLInputElement, configurable: true })
let DepotPageContent: typeof import('./depot-page-content.tsx').DepotPageContent
test.before(async () => { ({ DepotPageContent } = await import('./depot-page-content.tsx')) })
async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2000
  while (true) {
    try { assertion(); return } catch (error) {
      if (Date.now() >= deadline) throw error
    }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
  }
}
function deferred() {
  let resolve!: (response: Response) => void
  const promise = new Promise<Response>(yes => { resolve = yes })
  return { promise, resolve }
}
const listing = (title: string) => Response.json({
  schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'epoch',
  items: [{ id: 'private', title, providerId: 'team', artifactId: 'private' }],
  providerOutcomes: [], failures: [], coverageComplete: true,
  knownTotal: 1, totalIsExact: true, state: 'complete', nextCursor: null,
})
const detail = (title: string) => Response.json({
  schemaVersion: 'labby.depot-compatibility/v2', providerId: 'team', artifactId: 'private',
  artifact: { id: 'private', title },
})
const router = { push() {}, replace() {}, prefetch() {}, back() {}, forward() {}, refresh() {} }
const page = () => <AppRouterContext.Provider value={router as never}>
  <PathnameContext.Provider value="/depot"><SearchParamsContext.Provider value={new URLSearchParams({ artifact: 'private', artifactProvider: 'team' })}>
    <DepotPageContent />
  </SearchParamsContext.Provider></PathnameContext.Provider>
</AppRouterContext.Provider>

test('Discover removes retained private results and rejects late details across session changes', async () => {
  const originalFetch = globalThis.fetch
  const reads: ReturnType<typeof deferred>[] = []
  let lists = 0
  globalThis.fetch = async url => {
    if (url === '/v1/depot/providers') return Response.json([])
    if (url === '/v1/depot/artifacts/detail') {
      const read = deferred(); reads.push(read); return read.promise
    }
    if (url === '/v1/depot/discover') return listing(++lists === 1 ? 'Original private catalog' : 'New account catalog')
    throw new Error('unexpected endpoint')
  }
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'first' }, csrfToken: 'first', expiresAt: 1000 })
  const view = await renderClient(page())
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /Original private catalog/))
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await view.rerender(page())
    assert.doesNotMatch(document.body.textContent ?? '', /Original private catalog/)
    await waitFor(() => assert.equal(reads.length, 2))
    await act(async () => reads[0].resolve(detail('Old account private detail')))
    assert.doesNotMatch(document.body.textContent ?? '', /Old account private detail/)
    await act(async () => reads[1].resolve(detail('New account detail')))
    assert.match(document.body.textContent ?? '', /New account detail/)
    await waitFor(() => assert.match(view.container.textContent ?? '', /New account catalog/))
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
})
