import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'

function continueButton() {
  return [...document.querySelectorAll('button')]
    .find((button) => button.textContent?.includes('Continue with Google'))
}

test('reauth dialog explains recovery when the browser blocks its popup', async () => {
  const window = installTestDom()
  Object.defineProperty(globalThis, 'getComputedStyle', { value: window.getComputedStyle.bind(window), configurable: true })
  Object.defineProperty(globalThis, 'MutationObserver', { value: window.MutationObserver, configurable: true })
  Object.defineProperty(globalThis, 'Event', { value: window.Event, configurable: true })
  Object.defineProperty(globalThis, 'CustomEvent', { value: window.CustomEvent, configurable: true })
  Object.defineProperty(globalThis, 'NodeFilter', { value: window.NodeFilter, configurable: true })
  Object.defineProperty(globalThis, 'HTMLElement', { value: window.HTMLElement, configurable: true })
  Object.defineProperty(globalThis, 'HTMLInputElement', { value: window.HTMLInputElement, configurable: true })
  const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store.ts')
  const { ReauthDialog } = await import('./reauth-dialog.tsx')
  __setBrowserSessionStateForTests({
    status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000,
    isAdmin: true, csrfToken: 'csrf',
  })
  const view = await renderClient(
    <ReauthDialog
      open
      purpose={{ action: 'provider.save', resource: 'team', version: '7', operation: 'op-1', scope: 'lab:admin', payload: {} }}
      onOpenChange={() => {}}
      onProof={() => {}}
      openPopup={() => null}
    />,
  )
  try {
    const button = continueButton()
    assert.ok(button, document.body.innerHTML)
    await act(async () => { button.click() })
    assert.match(document.querySelector('[role="alert"]')?.textContent ?? '', /Allow popups/)
    assert.match(document.body.textContent ?? '', /Try again/)
  } finally {
    await view.unmount()
  }
})
