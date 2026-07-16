import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { Window } from 'happy-dom'

test('detail disable does not mutate until the user confirms', async () => {
  installDom()
  const { GatewayEnabledSetting } = await import('./gateway-enabled-setting')
  let disableCalls = 0
  let enableCalls = 0
  const { container, unmount } = await renderClient(
    <GatewayEnabledSetting
      enabled
      onEnable={() => { enableCalls += 1 }}
      onDisable={() => { disableCalls += 1 }}
    />,
  )

  await click(container.querySelector('[role="switch"]'))
  assert.equal(disableCalls, 0)
  assert.equal(enableCalls, 0)
  assert.match(document.body.textContent ?? '', /Disable server\?/)

  const confirm = [...document.querySelectorAll('button')]
    .find((button) => button.textContent?.trim() === 'Disable server')
  await click(confirm ?? null)
  assert.equal(disableCalls, 1)
  assert.equal(enableCalls, 0)

  await unmount()
})

function installDom() {
  const window = new Window()
  const globals: Array<[string, unknown]> = [
    ['window', window],
    ['self', window],
    ['document', window.document],
    ['navigator', window.navigator],
    ['HTMLElement', window.HTMLElement],
    ['HTMLButtonElement', window.HTMLButtonElement],
    ['HTMLInputElement', window.HTMLInputElement],
    ['Element', window.Element],
    ['DocumentFragment', window.DocumentFragment],
    ['Node', window.Node],
    ['NodeFilter', window.NodeFilter],
    ['Event', window.Event],
    ['MouseEvent', window.MouseEvent],
    ['PointerEvent', window.PointerEvent],
    ['KeyboardEvent', window.KeyboardEvent],
    ['CustomEvent', window.CustomEvent],
    ['DOMException', window.DOMException],
    ['MutationObserver', window.MutationObserver],
  ]
  for (const [key, value] of globals) {
    Object.defineProperty(globalThis, key, { configurable: true, value })
  }
  Object.defineProperty(globalThis, 'requestAnimationFrame', {
    configurable: true,
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 0),
  })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', {
    configurable: true,
    value: (handle: number) => window.clearTimeout(handle as never),
  })
  Object.defineProperty(globalThis, 'getComputedStyle', {
    configurable: true,
    value: window.getComputedStyle.bind(window),
  })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', {
    configurable: true,
    value: true,
  })
}

async function renderClient(element: React.ReactElement) {
  const { createRoot } = await import('react-dom/client')
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  await act(async () => root.render(element))
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}

async function click(element: Element | null) {
  assert.ok(element)
  await act(async () => {
    ;(element as HTMLElement).click()
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}
