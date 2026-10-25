import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act, useEffect, useState } from 'react'
import { PathnameContext } from 'next/dist/shared/lib/hooks-client-context.shared-runtime'
import { installTestDom } from '@/lib/testing/dom-install'
import { isCommandK, OPEN_COMMAND_PALETTE_EVENT } from '@/lib/command-palette-events'
import { __setBrowserSessionStateForTests, loadBrowserSession } from '@/lib/auth/session-store'
import type { AppCommandPaletteProps } from './app-command-palette'

function FakePalette({ initialOpen = false }: AppCommandPaletteProps) {
  const [open, setOpen] = useState(initialOpen)
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => { if (isCommandK(event)) setOpen(value => !value) }
    window.addEventListener('keydown', keydown)
    return () => window.removeEventListener('keydown', keydown)
  }, [])
  return <div data-loaded-palette={open ? 'open' : 'closed'} />
}
function deferred() {
  let resolve!: (value: { AppCommandPalette: typeof FakePalette }) => void
  const promise = new Promise<{ AppCommandPalette: typeof FakePalette }>(done => { resolve = done })
  return { promise, resolve }
}
async function setup(load: () => Promise<{ AppCommandPalette: typeof FakePalette }>) {
  const window = installTestDom()
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const { AppCommandPaletteEntry } = await import('./app-command-palette-entry')
  const element = (path: string) => <PathnameContext.Provider value={path}><AppCommandPaletteEntry load={load} /></PathnameContext.Provider>
  const view = await renderClient(element('/gateways/'))
  const key = async (key: string, modifiers = { ctrlKey: false, metaKey: false }) => {
    await act(async () => { window.dispatchEvent(new window.KeyboardEvent('keydown', { key, ...modifiers, cancelable: true })) })
  }
  const event = async () => { await act(async () => { window.dispatchEvent(new window.Event(OPEN_COMMAND_PALETTE_EVENT)) }) }
  return { window, view, element, key, event, close: async () => { await view.unmount(); await window.happyDOM.close() } }
}

test('closed palette does not import; a second shortcut cancels pending open and hands off once', async () => {
  const pending = deferred()
  let calls = 0
  const app = await setup(() => { calls++; return pending.promise })
  try {
    assert.equal(calls, 0)
    await app.key('k')
    assert.equal(calls, 0)
    await app.key('k', { ctrlKey: true, metaKey: false })
    assert.equal(calls, 1)
    assert.match(app.view.container.textContent ?? '', /Loading command palette/)
    await app.key('k', { ctrlKey: true, metaKey: false })
    assert.equal(app.view.container.textContent, '')
    await act(async () => { pending.resolve({ AppCommandPalette: FakePalette }) })
    assert.ok(app.view.container.querySelector('[data-loaded-palette="closed"]'))
    await app.key('k', { ctrlKey: true, metaKey: false })
    assert.ok(app.view.container.querySelector('[data-loaded-palette="open"]'))
    assert.equal(calls, 1)
  } finally { await app.close() }
})
test('topbar event loads and Escape prevents delayed reopening or focus theft', async () => {
  const pending = deferred()
  const app = await setup(() => pending.promise)
  const input = app.window.document.createElement('input')
  app.window.document.body.append(input)
  try {
    await app.event()
    await app.key('Escape')
    input.focus()
    await act(async () => { pending.resolve({ AppCommandPalette: FakePalette }) })
    assert.ok(app.view.container.querySelector('[data-loaded-palette="closed"]'))
    assert.equal(app.window.document.activeElement === input, true)
  } finally { input.remove(); await app.close() }
})
test('failed import exposes a generic retry and retry opens the palette', async () => {
  let calls = 0
  const app = await setup(async () => {
    if (++calls === 1) throw new Error('private internal module URL')
    return { AppCommandPalette: FakePalette }
  })
  try {
    await app.key('K', { ctrlKey: false, metaKey: true })
    assert.match(app.view.container.textContent ?? '', /could not load/)
    assert.doesNotMatch(app.view.container.textContent ?? '', /private internal/)
    const retry = [...app.view.container.querySelectorAll('button')].find(button => button.textContent === 'Try again')!
    await act(async () => { retry.click() })
    assert.equal(calls, 2)
    assert.ok(app.view.container.querySelector('[data-loaded-palette="open"]'))
  } finally { await app.close() }
})
test('navigation during a pending import cancels its open intent', async () => {
  const pending = deferred()
  const app = await setup(() => pending.promise)
  const opener = app.window.document.createElement('button')
  const input = app.window.document.createElement('input')
  app.window.document.body.append(opener, input)
  try {
    opener.focus()
    await app.event()
    input.focus()
    await app.view.rerender(app.element('/settings/'))
    await act(async () => { pending.resolve({ AppCommandPalette: FakePalette }) })
    assert.ok(app.view.container.querySelector('[data-loaded-palette="closed"]'))
    assert.equal(app.window.document.activeElement === input, true)
  } finally { opener.remove(); input.remove(); await app.close() }
})


test('authority refresh cancels a pending open without stealing the new input focus', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf' })
  const pending = deferred()
  const app = await setup(() => pending.promise)
  const opener = app.window.document.createElement('button')
  const input = app.window.document.createElement('input')
  app.window.document.body.append(opener, input)
  const originalFetch = globalThis.fetch
  try {
    opener.focus()
    await app.event()
    input.focus()
    globalThis.fetch = (async () => Response.json({ authenticated: false })) as typeof fetch
    await act(async () => { await loadBrowserSession() })
    await act(async () => { pending.resolve({ AppCommandPalette: FakePalette }) })
    assert.ok(app.view.container.querySelector('[data-loaded-palette="closed"]'))
    assert.equal(app.window.document.activeElement === input, true)
  } finally { globalThis.fetch = originalFetch; opener.remove(); input.remove(); await app.close() }
})
