import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'

test('direct settings route denies non-admin sessions without fetching providers', async () => {
  installTestDom()
  const session = await import('../../lib/auth/session-store.ts')
  session.__setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'reader' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', isAdmin: false })
  let requests = 0
  const original = globalThis.fetch
  globalThis.fetch = async () => { requests += 1; return Response.json([]) }
  const { DepotProvidersPage } = await import('./depot-providers-page.tsx')
  const view = await renderClient(<DepotProvidersPage />)
  try {
    assert.match(document.querySelector('[role="alert"]')?.textContent ?? '', /Administrator permission/)
    assert.equal(requests, 0)
  } finally {
    globalThis.fetch = original
    await view.unmount()
  }
})

test('host-managed connections explain server ownership and do not offer browser management', async () => {
  installTestDom()
  const session = await import('../../lib/auth/session-store.ts')
  session.__setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', isAdmin: true })
  const original = globalThis.fetch
  globalThis.fetch = async () => Response.json([{
    id: 'team-local', name: 'Team Library', endpoint: 'http://127.0.0.1:4100/',
    enabled: true, authMode: 'bearer', builtin: false, hostManaged: true,
    configVersion: 'v1', credentialConfigured: true,
    health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null },
  }])
  const { DepotProvidersPage } = await import('./depot-providers-page.tsx')
  const view = await renderClient(<DepotProvidersPage />)
  try {
    assert.match(view.container.textContent ?? '', /Managed in server configuration/)
    assert.match(view.container.textContent ?? '', /http:\/\/127\.0\.0\.1:4100\//)
    assert.equal([...view.container.querySelectorAll('button')].some(button => button.textContent === 'Manage'), false)
  } finally {
    globalThis.fetch = original
    await view.unmount()
    session.__setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
