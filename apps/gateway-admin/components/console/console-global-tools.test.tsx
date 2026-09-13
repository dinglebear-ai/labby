import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom } from '../../lib/testing/dom-install.ts'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'
installTestDom()
Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
Object.defineProperty(globalThis, 'NodeFilter', { configurable: true, value: window.NodeFilter })
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })

test('global library tray links real routes and distinguishes zero from unavailable', async () => {
  const { ConsoleLibraryTray } = await import('./console-global-tools.tsx')
  const html = renderToStaticMarkup(<ConsoleLibraryTray counts={{ artifacts: 0, tools: 17 }} />)
  for (const path of ['/library', '/loadouts', '/snippets', '/tools']) assert.match(html, new RegExp(`href="${path}"`))
  assert.match(html, />0<\/span>/)
  assert.match(html, />17<\/span>/)
  assert.equal((html.match(/Count unavailable for the current authority/g) ?? []).length, 2)
})

test('Phoenix opens an explicit unavailable session panel without simulated send actions', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    const trigger = view.container.querySelector('button[aria-label="Ask Phoenix"]'); assert.ok(trigger)
    await act(async () => { trigger.dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event) })
    const panel = document.querySelector('[aria-label="Phoenix session"]'); assert.ok(panel)
    assert.match(panel.textContent ?? '', /Session execution is unavailable/)
    assert.equal(panel.querySelector('textarea'), null)
    assert.equal(panel.querySelector('button[aria-label="Send"]'), null)
    assert.equal(panel.querySelector('a')?.getAttribute('href'), '/agents')
    const close = panel.querySelector('button[aria-label="Close Phoenix panel"]'); assert.ok(close)
    await act(async () => { close.dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event) })
    assert.equal(document.querySelector('[aria-label="Phoenix session"]'), null)
  } finally { await view.unmount() }
})

test('Phoenix sends a real turn through the container-local service and renders the reply', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const originalFetch = globalThis.fetch
  const actions: string[] = []
  globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    actions.push(body.action)
    if (body.action === 'phoenix.status') return new Response(JSON.stringify({ enabled: true, available: true, runtime: 'container_local', service: 'codex-app-server', sandbox: 'read-only' }), { status: 200 })
    if (body.action === 'phoenix.session.start') return new Response(JSON.stringify({ session_id: 'phoenix-1', status: 'ready', messages: [] }), { status: 200 })
    return new Response(JSON.stringify({ session_id: 'phoenix-1', status: 'ready', messages: [{ role: 'user', text: 'Is Labby healthy?' }, { role: 'assistant', text: 'The gateway is healthy.' }] }), { status: 200 })
  }) as typeof fetch
  const { PhoenixAvailability } = await import('./console-global-tools.tsx')
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const view = await renderClient(<PhoenixAvailability />)
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('button[aria-label="Ask Phoenix"]')!.click())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const panel = document.querySelector('[aria-label="Phoenix session"]'); assert.ok(panel)
    assert.match(panel.textContent ?? '', /CodexApp ServerRead only/)
    assert.match(panel.textContent ?? '', /Attached to the container-local session/)
    assert.equal(panel.querySelectorAll('button').length >= 5, true)
    const dockRight = panel.querySelector<HTMLButtonElement>('button[aria-label="Dock Phoenix right"]'); assert.ok(dockRight)
    await act(async () => dockRight.click())
    assert.equal(panel.getAttribute('data-dock'), 'right')
    const floatPanel = panel.querySelector<HTMLButtonElement>('button[aria-label="Float Phoenix panel"]'); assert.ok(floatPanel)
    await act(async () => floatPanel.click())
    assert.equal(panel.getAttribute('data-dock'), 'float')
    const input = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message Phoenix"]')
    assert.ok(input)
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value')?.set
      setter?.call(input, 'Is Labby healthy?')
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: 'Is Labby healthy?' }) as unknown as Event)
    })
    await act(async () => input.form!.requestSubmit())
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)) })
    assert.deepEqual(actions, ['phoenix.status', 'phoenix.session.start', 'phoenix.turn.send'])
    assert.match(panel.textContent ?? '', /The gateway is healthy/)
    assert.equal(panel.querySelectorAll('[data-phoenix-message]').length, 2)
    assert.ok(panel.querySelector('button[aria-label="Edit message"]'))
    assert.ok(panel.querySelector('button[aria-label="Regenerate"]'))
    assert.ok(panel.querySelector('button[aria-label="Copy answer"]'))
    assert.equal(document.querySelector('[aria-label="Close Phoenix panel"]')?.classList.contains('size-11'), true)
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
