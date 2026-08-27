import React from 'react'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { Window } from 'happy-dom'

export function installTestDom() {
  const window = new Window()
  Object.defineProperty(globalThis, 'window', { value: window, configurable: true })
  Object.defineProperty(globalThis, 'document', { value: window.document, configurable: true })
  Object.defineProperty(globalThis, 'navigator', { value: window.navigator, configurable: true })
  Object.defineProperty(globalThis, 'DOMException', { value: window.DOMException, configurable: true })
  Object.defineProperty(globalThis, 'Node', { value: window.Node, configurable: true })
  Object.defineProperty(globalThis, 'MouseEvent', { value: window.MouseEvent, configurable: true })
  Object.defineProperty(globalThis, 'PointerEvent', { value: window.PointerEvent, configurable: true })
  Object.defineProperty(globalThis, 'KeyboardEvent', { value: window.KeyboardEvent, configurable: true })
  Object.defineProperty(globalThis, 'CustomEvent', { value: window.CustomEvent, configurable: true })
  Object.defineProperty(globalThis, 'Element', { value: window.Element, configurable: true })
  Object.defineProperty(globalThis, 'HTMLElement', { value: window.HTMLElement, configurable: true })
  Object.defineProperty(globalThis, 'getComputedStyle', {
    value: window.getComputedStyle.bind(window),
    configurable: true,
  })
  Object.defineProperty(globalThis, 'MutationObserver', { value: window.MutationObserver, configurable: true })
  Object.defineProperty(globalThis, 'requestAnimationFrame', {
    configurable: true,
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 0),
  })
  Object.defineProperty(globalThis, 'cancelAnimationFrame', {
    configurable: true,
    value: (handle: number) => window.clearTimeout(handle as unknown as Parameters<typeof window.clearTimeout>[0]),
  })
  Object.defineProperty(globalThis, 'IS_REACT_ACT_ENVIRONMENT', { value: true, configurable: true })
  return window
}

export async function renderClient(element: React.ReactElement) {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)

  await act(async () => {
    root.render(element)
  })

  return {
    container,
    rerender: async (next: React.ReactElement) => {
      await act(async () => {
        root.render(next)
      })
    },
    unmount: async () => {
      await act(async () => root.unmount())
      container.remove()
    },
  }
}
