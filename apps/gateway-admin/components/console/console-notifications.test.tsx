import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom } from '@/lib/testing/dom-install'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import type { ConsoleStatusState } from './console-status-strip'

installTestDom()
__setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'notification-review' }, expiresAt: 100, csrfToken: 'csrf', isAdmin: true })
Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
Object.defineProperty(globalThis, 'NodeFilter', { configurable: true, value: window.NodeFilter })
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })

async function openNotifications(state: ConsoleStatusState) {
  const [{ ConsoleNotifications }, { renderClient }] = await Promise.all([
    import('./console-notifications'), import('@/lib/testing/dom-test-utils'),
  ])
  const view = await renderClient(<ConsoleNotifications state={state} />)
  const trigger = view.container.querySelector<HTMLButtonElement>('[aria-label="Notifications"]')
  assert.ok(trigger)
  await act(async () => trigger.click())
  return view
}

test('notifications show actual disconnected upstream names and preserve the Gateway destination', async () => {
  const view = await openNotifications({ kind: 'ready', snapshot: { connected: 1, total: 2, tools: 7 }, attention: ['source-alpha'] })
  assert.match(document.body.textContent ?? '', /source-alpha/)
  assert.match(document.body.textContent ?? '', /Enabled upstream is disconnected/)
  assert.equal(document.body.querySelector('[data-slot="popover-content"] a')?.getAttribute('href'), '/gateway?id=source-alpha')
  assert.equal(view.container.querySelector('[aria-label="Notifications"]')?.getAttribute('title'), 'Notifications — 1 needs attention')
  await view.unmount()
})

test('a failed inventory is reported as unavailable instead of an all-clear notification', async () => {
  const view = await openNotifications({ kind: 'unavailable', reason: 'Gateway did not respond.' })
  assert.match(document.body.textContent ?? '', /Gateway status unavailable: Gateway did not respond/)
  assert.doesNotMatch(document.body.textContent ?? '', /No disconnected upstreams/)
  await view.unmount()
})

test('notification tooltip reports all clear when there is no unread attention', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'notification-clear' }, expiresAt: 100, csrfToken: 'csrf', isAdmin: true })
  const view = await openNotifications({ kind: 'ready', snapshot: { connected: 1, total: 1, tools: 7 }, attention: [] })
  assert.equal(view.container.querySelector('[aria-label="Notifications"]')?.getAttribute('title'), 'Notifications — all clear')
  await view.unmount()
})

test('clear all removes notifications and shared badges until the disconnected incident recurs', async () => {
  const { ConsoleNotifications } = await import('./console-notifications')
  const { useGatewayNotifications } = await import('@/lib/notification-acknowledgements')
  const { WarningsBanner } = await import('@/components/dashboard/warnings-banner')
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  function OtherSurface() { const { notifications } = useGatewayNotifications(); return <output data-other-surface>{notifications.length}</output> }
  const failed: ConsoleStatusState = { kind: 'ready', snapshot: { connected: 0, total: 1, tools: 7 }, attention: ['repeat-alpha'] }
  const healthy: ConsoleStatusState = { kind: 'ready', snapshot: { connected: 1, total: 1, tools: 7 }, attention: [] }
  const content = (state: ConsoleStatusState) => <><ConsoleNotifications state={state}/><OtherSurface/><WarningsBanner count={1} signature="repeat-alpha" notificationKeys={['gateway:repeat-alpha:disconnected']}/></>
  const view = await renderClient(content(failed))
  try {
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Notifications"]')!.click())
    assert.equal(view.container.querySelector('output')?.textContent, '1')
    assert.match(view.container.textContent ?? '', /1 warning across servers/)
    const clear = [...document.querySelectorAll('button')].find(button => button.textContent?.includes('Clear all'))!
    await act(async () => clear.click())
    assert.equal(view.container.querySelector('output')?.textContent, '0')
    assert.doesNotMatch(view.container.textContent ?? '', /1 warning across servers/)
    assert.match(document.body.textContent ?? '', /No new notifications/)
    await view.rerender(content({ ...failed }))
    assert.equal(view.container.querySelector('output')?.textContent, '0')
    await view.rerender(content(healthy))
    await view.rerender(content(failed))
    assert.equal(view.container.querySelector('output')?.textContent, '1')
  } finally { await view.unmount() }
})
