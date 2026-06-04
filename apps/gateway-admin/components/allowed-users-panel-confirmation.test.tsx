import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { Window } from 'happy-dom'

import { authAdminApi, type AllowedEmailEntry } from '@/lib/api/auth-admin-client'

function installDom() {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'HTMLButtonElement', { configurable: true, value: window.HTMLButtonElement })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'MouseEvent', { configurable: true, value: window.MouseEvent })
  Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: window.PointerEvent })
  Object.defineProperty(globalThis, 'KeyboardEvent', { configurable: true, value: window.KeyboardEvent })
  Object.defineProperty(globalThis, 'CustomEvent', { configurable: true, value: window.CustomEvent })
  Object.defineProperty(globalThis, 'DOMException', { configurable: true, value: window.DOMException })
  Object.defineProperty(globalThis, 'MutationObserver', {
    configurable: true,
    value: window.MutationObserver,
  })
  Object.defineProperty(globalThis, 'requestAnimationFrame', {
    configurable: true,
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 0),
  })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', {
    configurable: true,
    value: (handle: number) => window.clearTimeout(handle as unknown as Parameters<typeof window.clearTimeout>[0]),
  })
  Object.defineProperty(globalThis, 'getComputedStyle', {
    configurable: true,
    value: window.getComputedStyle.bind(window),
  })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { configurable: true, value: true })
}

async function renderClient(element: React.ReactElement) {
  const { createRoot } = await import('react-dom/client')
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)

  await act(async () => {
    root.render(element)
  })

  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 20))
      })
    }
  }
  throw lastError
}

test('AllowedUsersPanel asks for confirmation before removing a user', async () => {
  installDom()
  const { AllowedUsersPanel } = await import('./allowed-users-panel')
  const entry: AllowedEmailEntry = {
    email: 'operator@example.com',
    added_by: 'admin@example.com',
    created_at: '2026-06-01T12:00:00Z',
  }
  const originalList = authAdminApi.listAllowedEmails
  const originalAdd = authAdminApi.addAllowedEmail
  const originalRemove = authAdminApi.removeAllowedEmail
  let removeCalls = 0

  authAdminApi.listAllowedEmails = async () => [entry]
  authAdminApi.addAllowedEmail = async () => entry
  authAdminApi.removeAllowedEmail = async () => {
    removeCalls += 1
  }

  try {
    const view = await renderClient(<AllowedUsersPanel />)
    await waitFor(() => assert.match(view.container.textContent ?? '', /operator@example\.com/))

    const removeButton = [...view.container.querySelectorAll('button')]
      .find((button) => button.textContent?.trim() === 'Remove')
    assert.ok(removeButton)
    await act(async () => {
      removeButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
    })

    assert.equal(removeCalls, 0)
    await waitFor(() => assert.match(document.body.textContent ?? '', /Remove user\?/))

    const dialog = document.body.querySelector('[data-slot="alert-dialog-content"]')
    assert.ok(dialog)
    const confirmButton = [...dialog.querySelectorAll('button')]
      .find((button) => button.textContent?.trim() === 'Remove user')
    assert.ok(confirmButton)
    await act(async () => {
      confirmButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
    })

    await waitFor(() => assert.equal(removeCalls, 1))
    await view.unmount()
  } finally {
    authAdminApi.listAllowedEmails = originalList
    authAdminApi.addAllowedEmail = originalAdd
    authAdminApi.removeAllowedEmail = originalRemove
  }
})
