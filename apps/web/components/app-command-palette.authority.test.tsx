import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { PathnameContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'

import { OPEN_COMMAND_PALETTE_EVENT } from '../lib/command-palette-events'
import { __setBrowserSessionStateForTests, loadBrowserSession } from '../lib/auth/session-store.ts'
import { installTestDom } from '../lib/testing/dom-install.ts'

// The DOM must exist before react-dom and the palette module are evaluated:
// react-dom decides at load time whether `input` events exist (otherwise it
// never turns them into `onChange`), and Radix resolves its `useLayoutEffect`
// shim to a no-op when `globalThis.document` is absent, so the dialog portal
// never mounts. Both are therefore imported inside the test, after
// `installTestDom()` has run.
installTestDom()
// cmdk measures its list with ResizeObserver and scrolls the active row into
// view; happy-dom implements neither, so inert stand-ins keep the palette
// renderable without changing what this test observes.
Object.defineProperty(globalThis, 'ResizeObserver', {
  configurable: true,
  value: class { observe() {} unobserve() {} disconnect() {} },
})
if (typeof window.Element.prototype.scrollIntoView !== 'function') {
  Object.defineProperty(window.Element.prototype, 'scrollIntoView', { configurable: true, value: () => {} })
}

const sessionPayload = {
  authenticated: true,
  user: { sub: 'operator' },
  expires_at: 999,
  csrf_token: 'csrf-rotated',
  principal_id: 'principal-1',
  organization_id: 'org-1',
  active_owner: { kind: 'team', id: 'team-a' },
  active_team_id: 'team-a',
  teams: [{ id: 'team-a', role: 'member', membership_epoch: 1, policy_epoch: 1 }],
  projects: [],
  capabilities: ['scope.read', 'platform.manage'],
  authority_generation: 7,
}

const authority = {
  schemaVersion: 1 as const, compatibilityGeneration: 1 as const, principalId: 'principal-1', organizationId: 'org-1',
  activeOwner: { kind: 'team' as const, id: 'team-a' }, activeTeamId: 'team-a', activeProjectId: undefined,
  teams: [{ id: 'team-a', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read', 'platform.manage'], generation: 7,
}

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw lastError
}

const router = { push: () => {}, replace: () => {}, back: () => {}, forward: () => {}, refresh: () => {}, prefetch: () => {} }
const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

/**
 * A workspace switch made in this tab is not the only way the palette's
 * authority can change: a session refresh can observe a server-side change
 * (new generation, revoked capability, different principal). Either must
 * discard the palette's open state and query, so the effect subscribes to the
 * shared authority identity rather than only the local switch event.
 */
test('the command palette closes and forgets its query when a session refresh observes a server-side authority change', async () => {
  document.body.replaceChildren()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', isAdmin: true, authority })
  let sessionGeneration = 7
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    if (String(input) === '/auth/session') {
      return new Response(JSON.stringify({ ...sessionPayload, authority_generation: sessionGeneration }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    return new Response('[]', { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof globalThis.fetch

  const [{ AppCommandPalette }, { renderClient }] = await Promise.all([
    import('./app-command-palette'),
    import('../lib/testing/dom-test-utils.tsx'),
  ])
  let unmount: (() => Promise<void>) | undefined
  try {
    ;({ unmount } = await renderClient(
      <AppRouterContext.Provider value={router as never}>
        <PathnameContext.Provider value="/loadouts">
          <AppCommandPalette />
        </PathnameContext.Provider>
      </AppRouterContext.Provider>,
    ))
    // Compared as booleans on purpose: a failing `assert.equal(element, null)`
    // makes node:assert deep-inspect the happy-dom element, which is a huge
    // cyclic graph and turns a two-second failure into an unreported hang.
    const dialog = () => document.querySelector<HTMLElement>('[data-palette="1"]')
    assert.equal(dialog() === null, true, 'the palette starts closed')

    await act(async () => { window.dispatchEvent(new window.Event(OPEN_COMMAND_PALETTE_EVENT)) })
    await waitFor(() => assert.ok(dialog(), 'the open event shows the palette'))
    const input = document.querySelector<HTMLInputElement>('[data-palette="1"] input')
    assert.ok(input)
    await act(async () => {
      const setValue = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
      setValue?.call(input, 'secret query')
      input.dispatchEvent(new window.Event('input', { bubbles: true }))
    })
    await waitFor(() => assert.equal(document.querySelector<HTMLInputElement>('[data-palette="1"] input')?.value, 'secret query'))

    // A transport-only refresh (same generation, rotated CSRF) is not an
    // authority change and must leave the open palette alone.
    await act(async () => { await loadBrowserSession() })
    assert.ok(dialog(), 'a transport-only session refresh keeps the palette open')
    assert.equal(document.querySelector<HTMLInputElement>('[data-palette="1"] input')?.value, 'secret query')

    // The server now reports a new authority generation: the palette must
    // close and discard the query without any local workspace switch event.
    sessionGeneration = 8
    await act(async () => { await loadBrowserSession() })
    await waitFor(() => assert.equal(dialog() === null, true, 'a server-side authority change closes the palette'))

    await act(async () => { window.dispatchEvent(new window.Event(OPEN_COMMAND_PALETTE_EVENT)) })
    await waitFor(() => assert.ok(dialog()))
    assert.equal(document.querySelector<HTMLInputElement>('[data-palette="1"] input')?.value, '', 'the previous query is not restored after an authority change')
  } finally {
    await unmount?.()
  }
})

test('the command palette resets when a project-only session refresh switches projects', async () => {
  document.body.replaceChildren()
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'operator' },
    expiresAt: Date.now() + 60_000,
    csrfToken: 'csrf',
    projectId: 'project-a',
  })
  let projectId = 'project-a'
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    if (String(input) === '/auth/session') {
      return new Response(JSON.stringify({
        authenticated: true,
        user: { sub: 'operator' },
        expires_at: 999,
        csrf_token: 'csrf-rotated',
        project_id: projectId,
        authority_generation: null,
        organization_id: null,
        owner: null,
        active_owner: null,
        teams: [],
        projects: [],
        capabilities: [],
      }), { status: 200, headers: { 'content-type': 'application/json' } })
    }
    return new Response('[]', { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof globalThis.fetch

  const [{ AppCommandPalette }, { renderClient }] = await Promise.all([
    import('./app-command-palette'),
    import('../lib/testing/dom-test-utils.tsx'),
  ])
  let unmount: (() => Promise<void>) | undefined
  try {
    ;({ unmount } = await renderClient(
      <AppRouterContext.Provider value={router as never}>
        <PathnameContext.Provider value="/skills">
          <AppCommandPalette />
        </PathnameContext.Provider>
      </AppRouterContext.Provider>,
    ))
    const dialog = () => document.querySelector<HTMLElement>('[data-palette="1"]')
    await act(async () => { window.dispatchEvent(new window.Event(OPEN_COMMAND_PALETTE_EVENT)) })
    await waitFor(() => assert.ok(dialog(), 'the open event shows the palette'))
    const input = document.querySelector<HTMLInputElement>('[data-palette="1"] input')
    assert.ok(input)
    await act(async () => {
      const setValue = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
      setValue?.call(input, 'project A query')
      input.dispatchEvent(new window.Event('input', { bubbles: true }))
    })
    await waitFor(() => assert.equal(document.querySelector<HTMLInputElement>('[data-palette="1"] input')?.value, 'project A query'))

    projectId = 'project-b'
    await act(async () => { await loadBrowserSession() })
    await waitFor(() => assert.equal(dialog() === null, true, 'a project-only context change closes the palette'))

    await act(async () => { window.dispatchEvent(new window.Event(OPEN_COMMAND_PALETTE_EVENT)) })
    await waitFor(() => assert.ok(dialog()))
    assert.equal(document.querySelector<HTMLInputElement>('[data-palette="1"] input')?.value, '', 'the old project query is discarded')
  } finally {
    await unmount?.()
  }
})
