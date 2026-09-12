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
  assert.match(markup, /Sign in again/)
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
    remediation: 'Complete owner bootstrap to enable multi-user authority.',
  })

  const markup = renderGate()
  assert.match(markup, /Finish setting up Labby/)
  assert.match(markup, /Complete owner bootstrap to enable multi-user authority\./)
  assert.match(markup, /owner@example\.com/)
  assert.match(markup, /name="organization_name"/)
  assert.match(markup, /name="project_name"/)
  assert.match(markup, /Complete owner bootstrap/)
  assert.equal(markup.includes('children'), false)
  assert.equal(markup.includes('Authentication Error'), false)
})

test('AuthBootstrap shows the no-access state for an unprovisioned identity', () => {
  __setBrowserSessionStateForTests({ ...signedIn, authorityState: 'unprovisioned' })

  const markup = renderGate()
  assert.match(markup, /No access yet/)
  assert.match(markup, /Ask an administrator/)
  assert.equal(markup.includes('children'), false)
})

test('AuthBootstrap renders the app for a ready session', () => {
  __setBrowserSessionStateForTests({ ...signedIn, authorityState: 'ready' })

  assert.match(renderGate(), /children/)
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
