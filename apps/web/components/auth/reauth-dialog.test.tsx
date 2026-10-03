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

for (const boundary of ['start', 'poll'] as const) {
  for (const transition of ['cancel', 'close', 'unmount', 'epoch'] as const) {
    test(`reauth ${transition} owns cancellation while ${boundary} is pending`, async () => {
      const window = installTestDom()
      for (const key of ['MutationObserver', 'Event', 'CustomEvent', 'NodeFilter', 'HTMLElement', 'HTMLInputElement'] as const) {
        Object.defineProperty(globalThis, key, { value: window[key], configurable: true })
      }
      Object.defineProperty(globalThis, 'getComputedStyle', { value: window.getComputedStyle.bind(window), configurable: true })
      const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store.ts')
      const { ReauthDialog } = await import('./reauth-dialog.tsx')
      const authenticate = () => __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
      authenticate()
      const originalFetch = globalThis.fetch
      let finish!: (response: Response) => void
      const cancelled: string[] = []
      let pollSignal: AbortSignal | null | undefined
      let proofCalls = 0
      const popup = { closed: false, location: { href: '' }, close() { this.closed = true } }
      globalThis.fetch = async (url, init) => {
        if (init?.method === 'DELETE') { cancelled.push(String(url)); return new Response(null, { status: 204 }) }
        if (String(url) === '/auth/reauth') {
          if (boundary === 'start') return new Promise<Response>(resolve => { finish = resolve })
          return Response.json({ interaction: 'owned', authorizationUrl: 'https://accounts.example/auth', expiresAt: 100 })
        }
        pollSignal = init?.signal
        return new Promise<Response>(resolve => { finish = resolve })
      }
      const props = {
        purpose: { action: 'providers.remove', resource: 'team', version: '7', operation: 'op-1', scope: 'lab:admin', payload: {} },
        onOpenChange: () => {}, onProof: () => { proofCalls += 1 }, openPopup: () => popup as unknown as Window,
      }
      const view = await renderClient(<ReauthDialog {...props} open />)
      let unmounted = false
      try {
        await act(async () => { continueButton()!.click() })
        assert.ok(finish)
        if (transition === 'cancel') {
          const cancel = [...document.querySelectorAll('button')].find(button => button.textContent === 'Cancel')
          assert.ok(cancel)
          assert.equal(cancel.disabled, false)
          await act(async () => cancel.click())
        } else if (transition === 'close') await view.rerender(<ReauthDialog {...props} open={false} />)
        else if (transition === 'unmount') { await view.unmount(); unmounted = true }
        else authenticate()
        if (transition !== 'epoch') {
          assert.equal(popup.closed, true)
          if (boundary === 'poll') assert.equal(pollSignal?.aborted, true)
        }
        await act(async () => {
          finish(Response.json(boundary === 'start'
            ? { interaction: 'owned', authorizationUrl: 'https://accounts.example/auth', expiresAt: 100 }
            : { status: 'Completed', proof: 'must-not-deliver' }))
        })
        assert.equal(proofCalls, 0)
        assert.equal(popup.closed, true)
        assert.deepEqual(cancelled, ['/auth/reauth/owned'])
      } finally {
        if (!unmounted) await view.unmount()
        globalThis.fetch = originalFetch
      }
    })
  }
}

test('provider removal cannot dispatch after its pending confirmation is unmounted', async () => {
  const window = installTestDom()
  for (const key of ['MutationObserver', 'Event', 'CustomEvent', 'NodeFilter', 'HTMLElement', 'HTMLInputElement'] as const) {
    Object.defineProperty(globalThis, key, { value: window[key], configurable: true })
  }
  Object.defineProperty(globalThis, 'getComputedStyle', { value: window.getComputedStyle.bind(window), configurable: true })
  const { __setBrowserSessionStateForTests } = await import('../../lib/auth/session-store.ts')
  const { DepotProviderDialog } = await import('../settings/depot-provider-dialog.tsx')
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const originalOpen = window.open
  let finish!: (response: Response) => void
  const requests: Array<{ url: string; method?: string }> = []
  const popup = { closed: false, location: { href: '' }, opener: null, close() { this.closed = true } }
  window.open = (() => popup) as unknown as typeof window.open
  globalThis.fetch = async (url, init) => {
    requests.push({ url: String(url), method: init?.method })
    if (init?.method === 'DELETE') return new Response(null, { status: 204 })
    if (String(url) === '/auth/reauth') return Response.json({ interaction: 'owned', authorizationUrl: 'https://accounts.example/auth', expiresAt: 100 })
    return new Promise<Response>(resolve => { finish = resolve })
  }
  const provider = { id: 'catalog', name: 'Catalog', endpoint: 'https://catalog.example', enabled: true, authMode: 'anonymous' as const, builtin: false, configVersion: 'v1', credentialConfigured: false, health: { state: 'unknown' as const, observedAt: null, provenance: null, retryNotBefore: null } }
  const view = await renderClient(<DepotProviderDialog provider={provider} baseVersion="v1" onSaved={() => assert.fail('cancelled removal saved')} onClose={() => {}} />)
  let unmounted = false
  try {
    const remove = [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Remove')
    assert.ok(remove)
    await act(async () => remove.click())
    await act(async () => continueButton()!.click())
    assert.ok(finish)
    await view.unmount(); unmounted = true
    await act(async () => finish(Response.json({ status: 'Completed', proof: 'late-proof' })))
    assert.equal(popup.closed, true)
    assert.equal(requests.some(request => request.url.startsWith('/v1/depot/providers')), false)
    assert.ok(requests.some(request => request.url === '/auth/reauth/owned' && request.method === 'DELETE'))
  } finally {
    if (!unmounted) await view.unmount()
    window.open = originalOpen
    globalThis.fetch = originalFetch
  }
})
