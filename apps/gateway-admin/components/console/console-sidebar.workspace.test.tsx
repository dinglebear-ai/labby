import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'

import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { PathnameContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'

import type { AuthoritySnapshot } from '@/lib/auth/authority.ts'
import { __setBrowserSessionStateForTests, getBrowserSessionState } from '@/lib/auth/session-store.ts'
import { installTestDom } from '@/lib/testing/dom-install.ts'

// The DOM is installed before react-dom and the sidebar are evaluated so React
// sees a real document (see lib/testing/dom-install.ts).
installTestDom()
// The sidebar's dependencies probe `self` for a browser; happy-dom does not
// define it on the Node global.
Object.defineProperty(globalThis, 'self', { configurable: true, value: window })

const pushed: string[] = []
const router = { push: (href: string) => { pushed.push(href) }, replace: () => {}, back: () => {}, forward: () => {}, refresh: () => {}, prefetch: () => {} }

const authority: AuthoritySnapshot = {
  schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1',
  activeOwner: { kind: 'team', id: 'team-a' }, activeTeamId: 'team-a',
  teams: [{ id: 'team-a', role: 'member', membershipEpoch: 1, policyEpoch: 1 }],
  projects: [{ id: 'project-a', role: 'manager', name: 'Project Alpha' }],
  capabilities: ['scope.read'], generation: 3,
}

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  pushed.splice(0)
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

async function renderSidebar(snapshot: AuthoritySnapshot | undefined) {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', authority: snapshot })
  globalThis.fetch = (async () => Response.json([])) as typeof globalThis.fetch
  const [{ ConsoleSidebar }, { ConsoleShellProvider }, { renderClient }] = await Promise.all([
    import('./console-sidebar.tsx'),
    import('./console-shell-context.tsx'),
    import('@/lib/testing/dom-test-utils.tsx'),
  ])
  document.body.replaceChildren()
  const view = await renderClient(
    <AppRouterContext.Provider value={router as never}>
      <PathnameContext.Provider value="/">
        <ConsoleShellProvider>
          <ConsoleSidebar />
        </ConsoleShellProvider>
      </PathnameContext.Provider>
    </AppRouterContext.Provider>,
  )
  const switcher = view.container.querySelector<HTMLButtonElement>('button[aria-label="Switch workspace"]')
  assert.ok(switcher, 'the workspace switcher must render')
  await act(async () => { switcher.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  const rows = [...view.container.querySelectorAll<HTMLButtonElement>('button[data-menurow="1"]')]
  assert.ok(rows.length > 0, 'opening the switcher must list workspace rows')
  const personal = rows.find(row => /Personal/.test(row.textContent ?? ''))
  assert.ok(personal, 'the Personal row must render')
  return { view, rows, personal }
}

/**
 * `selectSessionWorkspace({})` throws when the session carries no authority
 * projection. The Personal row used to call it unguarded, so a click became
 * an unhandled render-time error; it must be disabled instead, and the rows
 * that are enabled must route a rejected selection to an error toast rather
 * than letting it escape.
 */
test('the Personal workspace row is disabled while the authority projection is unavailable', async () => {
  const { view, rows, personal } = await renderSidebar(undefined)
  assert.equal(personal.disabled, true)
  assert.equal(personal.getAttribute('aria-disabled'), 'true')
  assert.match(personal.getAttribute('title') ?? '', /unavailable until the server projects your authority/)
  assert.equal(rows.length, 1, 'no team or project rows can exist without a projection')
  await act(async () => { personal.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  assert.deepEqual(pushed, [], 'a disabled row must not navigate')
  assert.equal(getBrowserSessionState().status, 'authenticated', 'a disabled row must not disturb the session')
  await view.unmount()
})

test('with a projection the Personal row is enabled, switches the workspace, and project rows show their display names', async () => {
  const { view, rows, personal } = await renderSidebar(authority)
  assert.equal(personal.disabled, false)
  assert.equal(personal.getAttribute('aria-disabled'), 'false')
  const project = rows.find(row => /Project Alpha/.test(row.textContent ?? ''))
  assert.ok(project, 'a project row must render the server-projected display name')
  assert.match(project.textContent ?? '', /project-a/, 'the identifier stays visible next to the display name')

  await act(async () => { personal.dispatchEvent(new window.MouseEvent('click', { bubbles: true })) })
  const state = getBrowserSessionState()
  assert.equal(state.status, 'authenticated')
  assert.deepEqual(state.status === 'authenticated' ? state.authority?.activeOwner : undefined, { kind: 'personal', id: 'principal-1' })
  assert.deepEqual(pushed, ['/'], 'a successful switch navigates home')
  await view.unmount()
})
