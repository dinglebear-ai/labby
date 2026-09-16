import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { installTestDom } from '../../lib/testing/dom-install.ts'

async function waitFor(assertion: () => void, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    try {
      assertion()
      return
    } catch (error) {
      if (Date.now() > deadline) throw error
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 10))
      })
    }
  }
}

test('surfaces the exchange failure message and clears the token', async () => {
  installTestDom()
  const originalFetch = globalThis.fetch
  const requests: Array<{ url: string; init?: RequestInit }> = []
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    requests.push({ url: String(input), init })
    return new Response(
      JSON.stringify({ kind: 'forbidden', message: 'Browser token sign-in is not offered over this transport.' }),
      { status: 403, headers: { 'content-type': 'application/json' } },
    )
  }) as typeof globalThis.fetch
  // react-dom decides whether a DOM exists when it loads, so the renderer and
  // the component are imported only after the DOM is installed.
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store.ts')
  const { LoginScreen } = await import('./login-screen.tsx')
  __setBrowserSessionStateForTests({ status: 'unauthenticated', bearerLoginAvailable: true })

  const view = await renderClient(<LoginScreen returnTo="/" />)
  try {
    const input = document.querySelector<HTMLInputElement>('#labby-bearer-token')
    assert.ok(input, document.body.innerHTML)
    // The setup token is a long-lived operator credential: the form must not
    // invite a password manager to store it.
    assert.equal(input.getAttribute('autocomplete'), 'off')

    await act(async () => {
      const setValue = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
      setValue?.call(input, 'operator-token')
      input.dispatchEvent(new window.Event('input', { bubbles: true }))
    })
    assert.equal(input.value, 'operator-token')
    const form = input.closest('form')
    assert.ok(form)
    await act(async () => {
      form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))
    })

    await waitFor(() =>
      assert.match(document.body.textContent ?? '', /Browser token sign-in is not offered over this transport\./),
    )
    assert.equal(requests.length, 1)
    assert.equal(requests[0].url, '/auth/bearer-session')
    assert.equal(new Headers(requests[0].init?.headers).get('authorization'), 'Bearer operator-token')
    // A rejected token must not linger in the form.
    assert.equal(document.querySelector<HTMLInputElement>('#labby-bearer-token')?.value, '')
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
  }
})
