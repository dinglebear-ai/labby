import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { useCollectionView } from './use-collection-view'

installTestDom()

function Example() {
  const [view, select] = useCollectionView('test.collection.layout')
  return <button onClick={() => select('list')}>{view}</button>
}

test('responsive defaults follow resize until an explicit choice, then survive remount', async () => {
  window.localStorage.clear()
  const original = window.matchMedia
  let narrow = false
  const listeners = new Set<() => void>()
  window.matchMedia = (() => ({
    get matches() { return narrow },
    addEventListener: (_: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
  })) as unknown as typeof window.matchMedia
  let mounted = await renderClient(<Example />)
  try {
    assert.equal(mounted.container.textContent, 'table')
    await act(async () => { narrow = true; listeners.forEach(listener => listener()) })
    assert.equal(mounted.container.textContent, 'cards')
    await act(async () => mounted.container.querySelector('button')!.click())
    await act(async () => { narrow = false; listeners.forEach(listener => listener()) })
    assert.equal(mounted.container.textContent, 'list')
    await mounted.unmount()
    assert.equal(listeners.size, 0)
    mounted = await renderClient(<Example />)
    assert.equal(mounted.container.textContent, 'list')
  } finally {
    await mounted.unmount()
    window.matchMedia = original
    window.localStorage.clear()
  }
})

test('unavailable storage keeps mobile defaults and an explicit session choice', async () => {
  const originalMedia = window.matchMedia
  const originalStorage = Object.getOwnPropertyDescriptor(window, 'localStorage')
  window.matchMedia = (() => ({ matches: true, addEventListener() {}, removeEventListener() {} })) as unknown as typeof window.matchMedia
  Object.defineProperty(window, 'localStorage', { configurable: true, get() { throw new Error('Storage denied') } })
  const mounted = await renderClient(<Example />)
  try {
    assert.equal(mounted.container.textContent, 'cards')
    await act(async () => mounted.container.querySelector('button')!.click())
    assert.equal(mounted.container.textContent, 'list')
  } finally {
    await mounted.unmount()
    window.matchMedia = originalMedia
    if (originalStorage) Object.defineProperty(window, 'localStorage', originalStorage)
    else Reflect.deleteProperty(window, 'localStorage')
  }
})
