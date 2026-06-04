import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'
import { Window } from 'happy-dom'

import type { ACPProject, ACPRun } from './types'

function installDom() {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: window.document })
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: window.navigator })
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: window.HTMLElement })
  Object.defineProperty(globalThis, 'HTMLButtonElement', { configurable: true, value: window.HTMLButtonElement })
  Object.defineProperty(globalThis, 'Node', { configurable: true, value: window.Node })
  Object.defineProperty(globalThis, 'MouseEvent', { configurable: true, value: window.MouseEvent })
  Object.defineProperty(globalThis, 'PointerEvent', { configurable: true, value: window.PointerEvent })
  Object.defineProperty(globalThis, 'Event', { configurable: true, value: window.Event })
  Object.defineProperty(globalThis, 'CustomEvent', { configurable: true, value: window.CustomEvent })
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

const projects: ACPProject[] = [{ id: 'workspace', name: 'Workspace', agentId: 'codex' }]
const runs: ACPRun[] = []

test('hidden session cleanup ignores repeat clicks while cleanup is in flight', async () => {
  installDom()
  const { SessionSidebar } = await import('./session-sidebar')
  let cleanupCalls = 0
  let resolveCleanup: ((value: { closedCount: number; failedCount: number }) => void) | null = null
  const cleanupPromise = new Promise<{ closedCount: number; failedCount: number }>((resolve) => {
    resolveCleanup = resolve
  })
  const view = await renderClient(
    <SessionSidebar
      projects={projects}
      runs={runs}
      selectedRunId={null}
      selectedProjectId="workspace"
      onSelectRun={() => {}}
      onNewRun={() => {}}
      hiddenRunCount={3}
      onBulkCloseHidden={() => {
        cleanupCalls += 1
        return cleanupPromise
      }}
    />,
  )

  const cleanupButton = [...view.container.querySelectorAll('button')]
    .find((button) => button.textContent?.trim() === 'Clean up')
  assert.ok(cleanupButton)

  await act(async () => {
    cleanupButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    cleanupButton.dispatchEvent(new MouseEvent('click', { bubbles: true }))
  })

  assert.equal(cleanupCalls, 1)
  assert.equal(cleanupButton.hasAttribute('disabled'), true)

  await act(async () => {
    resolveCleanup?.({ closedCount: 3, failedCount: 0 })
    await cleanupPromise
  })

  assert.equal(cleanupButton.hasAttribute('disabled'), false)
  await view.unmount()
})
