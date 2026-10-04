import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { SearchParamsContext, PathnameContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import type { FederatedArtifact } from '@/lib/api/depot-client'

const dom = installTestDom()
Object.defineProperty(globalThis, 'self', { value: dom, configurable: true })
Object.defineProperty(globalThis, 'NodeFilter', { value: dom.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: dom.HTMLInputElement, configurable: true })
let DepotPageContent: typeof import('./depot-page-content').DepotPageContent
test.before(async () => { ({ DepotPageContent } = await import('./depot-page-content')) })
const router = { push() {}, replace() {}, prefetch() {}, back() {}, forward() {}, refresh() {} }
const page = () => <AppRouterContext.Provider value={router as never}>
  <PathnameContext.Provider value="/depot"><SearchParamsContext.Provider value={new URLSearchParams()}><DepotPageContent /></SearchParamsContext.Provider></PathnameContext.Provider>
</AppRouterContext.Provider>
const items: FederatedArtifact[] = [
  { providerId: 'catalog', artifactId: 'skill', kind: 'skill', title: 'Retrieved skill', currentRevision: { authoredAt: '2026-10-01T12:00:00Z' } },
  { providerId: 'other', artifactId: 'loadout', kind: 'loadout', title: 'Retrieved loadout', currentRevision: { authoredAt: '2026-10-02T12:00:00Z' } },
]
function response(state: string, rows: FederatedArtifact[], failures: unknown[] = []) {
  return Response.json({ schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'fixture', items: rows,
    providerOutcomes: [], failures, coverageComplete: ['complete', 'empty'].includes(state), knownTotal: state === 'all_failed' ? null : rows.length,
    totalIsExact: ['complete', 'empty'].includes(state), state, nextCursor: null })
}
async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 3000
  while (true) {
    try { assertion(); return } catch (error) { if (Date.now() >= deadline) throw error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
  }
}
function authenticate() { __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'catalog-reader' }, csrfToken: 'fixture', expiresAt: Date.now() + 100000 }) }

test('live Discover collections show loading, API failures and real retrieved cards after retry', async () => {
  const originalFetch = globalThis.fetch
  let release!: (value: Response) => void
  const pending = new Promise<Response>(resolve => { release = resolve })
  let reads = 0
  globalThis.fetch = async (url, init) => {
    if (url === '/v1/depot/providers') return Response.json([])
    assert.equal(url, '/v1/depot/discover')
    assert.deepEqual(JSON.parse(String(init?.body)), { provider: null, query: '', limit: 50 })
    reads++
    return reads === 1 ? pending : response('complete', items)
  }
  authenticate()
  const view = await renderClient(page())
  const collections = () => view.container.querySelector('[data-discover-rails]')!
  try {
    await waitFor(() => assert.equal(reads, 1))
    assert.match(collections().textContent ?? '', /Checking connected sources/)
    await act(async () => release(response('all_failed', [], [{ providerId: 'catalog', kind: 'index_not_ready' }])))
    await waitFor(() => assert.match(collections().textContent ?? '', /catalog index is still preparing/))
    assert.doesNotMatch(collections().textContent ?? '', /No artifacts returned|Recommendation evidence/)
    const retry = [...collections().querySelectorAll('button')].find(button => button.textContent === 'Retry search')!
    await act(async () => retry.click())
    await waitFor(() => assert.match(collections().textContent ?? '', /Retrieved loadout/))
    assert.equal(reads, 2)
    const latest = collections().querySelector('section[aria-label="Recently updated"]')!
    assert.match(latest.textContent ?? '', /Retrieved loadout/)
    assert.ok(collections().querySelector('a[href*="artifactProvider=other"][href*="artifact=loadout"]'))
    assert.doesNotMatch(collections().textContent ?? '', /Popular This Week|New From Your Team|Pairs With Your Loadouts/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})

test('live Discover collections retain successful provider cards during partial coverage', async () => {
  const originalFetch = globalThis.fetch
  globalThis.fetch = async url => url === '/v1/depot/providers' ? Response.json([]) : response('partial', items, [{ providerId: 'failed-source', kind: 'unavailable' }])
  authenticate()
  const view = await renderClient(page())
  try {
    await waitFor(() => assert.match(view.container.querySelector('[data-discover-rails]')?.textContent ?? '', /Retrieved skill/))
    const collections = view.container.querySelector('[data-discover-rails]')!
    assert.match(collections.textContent ?? '', /unavailable[\s\S]*Retry|Retry[\s\S]*unavailable/)
    assert.equal(collections.querySelectorAll('section').length, 3)
    assert.match(collections.querySelector('section[aria-label="Loadouts to explore"]')?.textContent ?? '', /Retrieved loadout/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})

test('live Discover confirmed empty catalog gives publishing and source recovery actions', async () => {
  const originalFetch = globalThis.fetch
  globalThis.fetch = async url => url === '/v1/depot/providers' ? Response.json([]) : response('empty', [])
  authenticate()
  const view = await renderClient(page())
  try {
    await waitFor(() => assert.match(view.container.querySelector('[data-discover-rails]')?.textContent ?? '', /No artifacts returned/))
    const collections = view.container.querySelector('[data-discover-rails]')!
    assert.match(collections.querySelector('a[aria-label="Publish artifact"]')?.getAttribute('href') ?? '', /^\/create\/?$/)
    assert.match(collections.querySelector('a[aria-label="Review sources"]')?.getAttribute('href') ?? '', /^\/settings\/depot\/?$/)
    assert.doesNotMatch(collections.textContent ?? '', /unavailable|Recommendation evidence/)
  } finally { await view.unmount(); globalThis.fetch = originalFetch }
})
