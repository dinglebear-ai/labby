import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom } from '../../lib/testing/dom-install.ts'
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
