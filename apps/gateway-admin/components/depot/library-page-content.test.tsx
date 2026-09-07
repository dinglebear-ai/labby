import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'

import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { PathnameContext, SearchParamsContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'

import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store.ts'
import { installTestDom } from '@/lib/testing/dom-install.ts'

// The DOM is installed before react-dom and the page are evaluated so React
// sees a real document (see lib/testing/dom-install.ts).
installTestDom()

const router = { push: () => {}, replace: () => {}, back: () => {}, forward: () => {}, refresh: () => {}, prefetch: () => {} }

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw lastError
}

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

function statValue(container: HTMLElement, label: string) {
  return container.querySelector(`[data-console-hero-stat="${label}"] [data-console-hero-stat-value="1"]`)?.textContent?.trim()
}

async function renderLibrary(depot: Record<string, unknown>) {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const requested: string[] = []
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const path = new URL(String(input), 'http://labby.test').pathname
    requested.push(path)
    if (path === '/v1/depot/status') return Response.json({ depot })
    return Response.json({ artifacts: [], total: 0 })
  }) as typeof globalThis.fetch
  const [{ LibraryPageContent }, { renderClient }] = await Promise.all([
    import('./library-page-content.tsx'),
    import('@/lib/testing/dom-test-utils.tsx'),
  ])
  document.body.replaceChildren()
  const view = await renderClient(
    <AppRouterContext.Provider value={router as never}>
      <PathnameContext.Provider value="/library">
        <SearchParamsContext.Provider value={new URLSearchParams() as never}>
          <LibraryPageContent />
        </SearchParamsContext.Provider>
      </PathnameContext.Provider>
    </AppRouterContext.Provider>,
  )
  return { view, requested }
}

/**
 * The Authority stat and the live/unavailable pulse used to be rendered from a
 * hardcoded `DepotStatus` literal, which reported every installation as a
 * healthy read-only catalog regardless of what the server projected. They
 * must come from the server's `/v1/depot/status` projection.
 */
test('the Library authority stat and pulse come from the server Depot status, not a constant', async () => {
  const write = await renderLibrary({ configured: true, enabled: true, authority: 'write', maxResponseBytes: 1_048_576 })
  await waitFor(() => assert.equal(statValue(write.view.container, 'Authority'), 'Read + write'))
  assert.ok(write.requested.includes('/v1/depot/status'), 'the page must ask the server for the Depot status')
  assert.match(write.view.container.textContent ?? '', /live catalog/)
  await write.view.unmount()

  const disabled = await renderLibrary({ configured: true, enabled: false, authority: 'read', maxResponseBytes: 1_048_576 })
  await waitFor(() => assert.equal(statValue(disabled.view.container, 'Authority'), 'Read only'))
  assert.match(disabled.view.container.textContent ?? '', /Depot unavailable/)
  assert.doesNotMatch(disabled.view.container.textContent ?? '', /live catalog/)
  await disabled.view.unmount()
})

test('a Depot status the browser cannot validate is surfaced as an error instead of a fabricated authority', async () => {
  const broken = await renderLibrary({ configured: true, enabled: true, authority: 'root', maxResponseBytes: 1 })
  await waitFor(() => assert.match(broken.view.container.textContent ?? '', /incompatible status response/))
  assert.notEqual(statValue(broken.view.container, 'Authority'), 'Read + write')
  assert.notEqual(statValue(broken.view.container, 'Authority'), 'Read only')
  await broken.view.unmount()
})
