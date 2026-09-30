import test from 'node:test'
import assert from 'node:assert/strict'
import { updateLibraryUrl } from './library-url.ts'

test('initial library filters do not navigate or reload the current route', () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const replacements: string[] = []
  Object.defineProperty(globalThis, 'window', { configurable: true, value: {
    location: { href: 'https://labby.example/library/?q=review#details' },
    history: { replaceState: (_state: unknown, _unused: string, url: string) => replacements.push(url) },
  } })
  try {
    updateLibraryUrl({ artifact: null, q: 'review' })
    assert.deepEqual(replacements, [])
    updateLibraryUrl({ artifact: 'art-1' })
    assert.deepEqual(replacements, ['/library/?q=review&artifact=art-1#details'])
  } finally {
    if (original) Object.defineProperty(globalThis, 'window', original)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})
