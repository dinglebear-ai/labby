import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { AuthBootstrap } from './auth-bootstrap.tsx'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'

function withAuthEnv<T>(run: () => T): T {
  const keys = [
    'NEXT_PUBLIC_API_TOKEN',
    'NEXT_PUBLIC_MOCK_DATA',
    'NEXT_PUBLIC_STANDALONE_BEARER_AUTH',
  ] as const
  const previous = new Map(keys.map((key) => [key, process.env[key]]))

  for (const key of keys) {
    delete process.env[key]
  }

  try {
    return run()
  } finally {
    for (const key of keys) {
      const value = previous.get(key)
      if (value === undefined) {
        delete process.env[key]
      } else {
        process.env[key] = value
      }
    }
  }
}

test('AuthBootstrap renders the auth_error login screen instead of the generic error state', () => {
  __setBrowserSessionStateForTests({
    status: 'auth_error',
    kind: 'internal_error',
    message: 'auth store unavailable',
    requestId: 'req-auth-123',
  })

  const markup = withAuthEnv(() =>
    renderToStaticMarkup(
      React.createElement(
        AuthBootstrap,
        null,
        React.createElement('div', null, 'children'),
      ),
    ),
  )

  assert.match(markup, /Authentication Error/)
  assert.match(markup, /auth store unavailable/)
  assert.match(markup, /Request ID: req-auth-123/)
  assert.match(markup, /Sign In Again/)
})

test('AuthBootstrap does not bypass hosted auth when NEXT_PUBLIC_API_TOKEN is set', () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })

  const markup = withAuthEnv(() => {
    process.env.NEXT_PUBLIC_API_TOKEN = 'dev-token'

    return renderToStaticMarkup(
      React.createElement(
        AuthBootstrap,
        null,
        React.createElement('div', null, 'children'),
      ),
    )
  })

  assert.equal(markup.includes('children'), false)
})

function renderGate() {
  return withAuthEnv(() =>
    renderToStaticMarkup(
      React.createElement(AuthBootstrap, null, React.createElement('div', null, 'children')),
    ),
  )
}

const signedIn = {
  status: 'authenticated' as const,
  user: { sub: 'owner-subject', email: 'owner@example.com' },
  expiresAt: 124,
  csrfToken: 'csrf-owner',
}

test('AuthBootstrap shows owner setup instead of the app while owner bootstrap is pending', () => {
  __setBrowserSessionStateForTests({
    ...signedIn,
    authorityState: 'transport',
    ownerBootstrapAvailable: true,
    remediation: 'Complete owner bootstrap to enable multi-user authority.',
  })

  const markup = renderGate()
  assert.match(markup, /Finish setting up Labby/)
  assert.match(markup, /Complete owner bootstrap to enable multi-user authority\./)
  assert.match(markup, /owner@example\.com/)
  assert.equal(markup.includes('Using Local / Default. No decisions required.'), true)
  assert.match(markup, /Customize organization and project names/)
  assert.match(markup, /Finish setup/)
  assert.equal(markup.includes('name="organization_name"'), false)
  assert.equal(markup.includes('name="project_name"'), false)
  assert.equal(markup.includes('children'), false)
  assert.equal(markup.includes('Authentication Error'), false)
})

function assertNoBootstrapForm(markup: string) {
  assert.equal(markup.includes('name="organization_name"'), false)
  assert.equal(markup.includes('name="project_name"'), false)
  assert.equal(markup.includes('Customize organization and project names'), false)
  assert.equal(markup.includes('Finish setup'), false)
  assert.match(markup, /Sign out/)
  assert.equal(markup.includes('children'), false)
}

test('AuthBootstrap never offers owner bootstrap to an unprovisioned identity', () => {
  // Even a stale or forged availability flag cannot surface the form once an
  // owner exists: unprovisioned is only reachable after bootstrap completed.
  __setBrowserSessionStateForTests({ ...signedIn, authorityState: 'unprovisioned', ownerBootstrapAvailable: true })

  const markup = renderGate()
  assert.match(markup, /Join your team/)
  assert.match(markup, /Use your team invitation to finish joining this Labby./)
  assert.match(markup, /Invitation code/)
  assert.match(markup, /Paste invitation code/)
  assert.match(markup, /Join team/)
  assertNoBootstrapForm(markup)
})

test('AuthBootstrap withholds owner bootstrap from an ineligible caller before setup', () => {
  __setBrowserSessionStateForTests({
    ...signedIn,
    authorityState: 'transport',
    remediation: 'Complete owner bootstrap to enable multi-user authority.',
  })

  const markup = renderGate()
  assert.match(markup, /No access yet/)
  assert.match(markup, /Only its configured owner account can finish setup/)
  assertNoBootstrapForm(markup)
})

test('AuthBootstrap renders the app for a ready session', () => {
  __setBrowserSessionStateForTests({ ...signedIn, authorityState: 'ready' })

  assert.match(renderGate(), /children/)
})

test('AuthBootstrap surfaces a signed Team profile offer after enrollment instead of dropping it', async () => {
  const { installTestDom, renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const { ORGANIZATION_PROFILE_OFFER_SESSION_KEY } = await import('../../lib/auth/organization-profile-handoff.ts')
  installTestDom()
  const offer = {
    signer_fingerprint: 'ab'.repeat(32),
    profile: {
      schema_version: 'labby.organization-bootstrap/v1',
      organization_id: 'team-org',
      issued_at: 123,
      key_id: 'team-key',
      verifying_key: 'verify-key',
      integrations: [],
      signature: 'signature',
    },
  }
  window.sessionStorage.setItem(ORGANIZATION_PROFILE_OFFER_SESSION_KEY, JSON.stringify(offer))
  __setBrowserSessionStateForTests({ ...signedIn, authorityState: 'ready' })

  const view = await withAuthEnv(() => renderClient(React.createElement(AuthBootstrap, null, React.createElement('div', null, 'children'))))
  try {
    assert.match(view.container.textContent ?? '', /Bring team defaults into your personal Labby/)
    assert.match(view.container.textContent ?? '', /Your personal Labby stays the runtime owner/)
    assert.equal((view.container.textContent ?? '').includes('children'), false)
  } finally {
    window.sessionStorage.clear()
    await view.unmount()
  }
})

test('authority changes replace the SWR cache and isolate late mutations', async () => {
  const { act } = await import('react')
  const { useSWRConfig } = await import('swr')
  const { installTestDom, renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  installTestDom()
  let current: ReturnType<typeof useSWRConfig> | undefined
  function CacheConsumer() {
    current = useSWRConfig()
    return React.createElement('span', null, 'cache consumer')
  }
  __setBrowserSessionStateForTests({ ...signedIn, projectId: 'first', isAdmin: true })
  const view = await renderClient(React.createElement(AuthBootstrap, null, React.createElement(CacheConsumer)))
  try {
    const first = current!
    await act(async () => { await first.mutate('/gateways', ['first-project'], false) })
    assert.deepEqual(first.cache.get('/gateways')?.data, ['first-project'])
    await act(async () => {
      __setBrowserSessionStateForTests({ ...signedIn, projectId: 'second', isAdmin: true })
    })
    await view.rerender(React.createElement(AuthBootstrap, null, React.createElement(CacheConsumer)))
    assert.notEqual(current!.cache, first.cache)
    assert.equal(current!.cache.get('/gateways'), undefined)
    await act(async () => { await first.mutate('/gateways', ['late-first-project'], false) })
    assert.equal(current!.cache.get('/gateways'), undefined)
  } finally { await view.unmount() }
})
