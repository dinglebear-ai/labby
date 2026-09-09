import { Window } from 'happy-dom'

/**
 * Install a happy-dom window on `globalThis`.
 *
 * This module deliberately imports nothing from React or react-dom. React
 * decides at module load whether a DOM exists (`canUseDOM`), and Radix
 * resolves its `useLayoutEffect` shim the same way; a test that needs real
 * `input` events or a portal-mounted dialog must install the DOM before those
 * modules are evaluated. Import `installTestDom` from here, call it, and then
 * dynamically import `dom-test-utils` and the component under test.
 */
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
