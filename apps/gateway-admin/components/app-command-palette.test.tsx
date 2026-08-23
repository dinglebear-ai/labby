import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { AppRouterContext } from 'next/dist/shared/lib/app-router-context.shared-runtime'
import { PathnameContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'

import { AppCommandPalette } from './app-command-palette'
import { __setBrowserSessionStateForTests } from '../lib/auth/session-store.ts'
import { installTestDom, renderClient } from '../lib/testing/dom-test-utils.tsx'

/**
 * `gateway.list` warms the upstream pool, which cold-spawns every configured
 * stdio MCP server. The palette is mounted by the admin layout on every page,
 * so an ungated fetch here spawned the whole fleet on each navigation and
 * defeated the gating the Loadouts page had added for exactly this reason.
 */
test('the always-mounted command palette does not fetch gateways until it is opened', async () => {
  installTestDom()
  const originalFetch = globalThis.fetch
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'operator' },
    expiresAt: Date.now() + 60_000,
    csrfToken: 'csrf',
    isAdmin: true,
  })

  const actions: string[] = []
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    let action = 'unknown'
    if (typeof init?.body === 'string') {
      action = (JSON.parse(init.body) as { action?: string }).action ?? 'unknown'
    }
    actions.push(action)
    return new Response('[]', { status: 200, headers: { 'content-type': 'application/json' } })
  }) as typeof globalThis.fetch

  // Unmount must run in `finally`: if the assertion fails while the
  // SWR-subscribed tree is still mounted, its pending work pins the event loop
  // and the whole single-process `tsx --test` run hangs (buffering the failure
  // output) instead of reporting the regression.
  let unmount: (() => Promise<void>) | undefined
  try {
    const router = {
      push: () => {},
      replace: () => {},
      back: () => {},
      forward: () => {},
      refresh: () => {},
      prefetch: () => {},
    }
    ;({ unmount } = await renderClient(
      <AppRouterContext.Provider value={router as never}>
        <PathnameContext.Provider value="/loadouts">
          <AppCommandPalette />
        </PathnameContext.Provider>
      </AppRouterContext.Provider>,
    ))

    assert.equal(
      actions.includes('gateway.list'),
      false,
      `closed palette must not list gateways, saw: ${actions.join(', ')}`,
    )
  } finally {
    await unmount?.()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
