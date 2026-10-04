import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom } from '@/lib/testing/dom-install'

test('local setup capability leaves URL before redeem and never uses a bearer header', async () => {
  installTestDom()
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const { LoginScreen } = await import('./login-screen')
  const { __setBrowserSessionStateForTests } = await import('@/lib/auth/session-store')
  __setBrowserSessionStateForTests({ status: 'unauthenticated', bearerLoginAvailable: true })
  const token = 'a'.repeat(43)
  window.history.replaceState(null, '', `/settings/#setup_handoff=${token}`)
  const originalFetch = globalThis.fetch
  let calls = 0
  globalThis.fetch = async (url, init) => {
    assert.equal(window.location.hash, '')
    assert.equal(String(url), '/auth/setup-handoff/redeem')
    assert.equal(new Headers(init?.headers).get('authorization'), null)
    assert.equal(init?.credentials, 'include')
    assert.deepEqual(JSON.parse(String(init?.body)), { token })
    calls++
    return Response.json({ message: 'expired' }, { status: 403 })
  }
  const view = await renderClient(<LoginScreen returnTo="/settings/" />)
  try {
    for (let i = 0; i < 20 && !view.container.textContent?.includes('expired'); i++) await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    assert.equal(calls, 1)
    assert.equal(window.location.hash, '')
    assert.match(view.container.textContent ?? '', /local setup link expired or could not be verified/)
    assert.doesNotMatch(view.container.textContent ?? '', new RegExp(token))
    assert.equal((view.container.querySelector('input[type=password]') as HTMLInputElement)?.value, '')
  } finally { await view.unmount(); globalThis.fetch = originalFetch; __setBrowserSessionStateForTests({ status: 'unauthenticated' }) }
})
