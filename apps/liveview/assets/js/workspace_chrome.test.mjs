import {test} from 'node:test'
import assert from 'node:assert/strict'
import {readCollapsed, writeCollapsed} from './workspace_chrome.mjs'
test('sidebar preferences are caller-scoped, bounded, and malformed values are ignored', () => {
  const entries = new Map()
  const storage = {getItem: key => entries.get(key), setItem: (key, value) => entries.set(key, value)}
  writeCollapsed(storage, 'gateway:caller-a', true)
  assert.equal(readCollapsed(storage, 'gateway:caller-a'), true)
  assert.equal(readCollapsed(storage, 'gateway:caller-b'), false)
  writeCollapsed(storage, null, true)
  assert.equal(entries.size, 1)
  entries.set('labby:gateway:caller-a:sidebar', '"true"')
  assert.equal(readCollapsed(storage, 'gateway:caller-a'), false)
  assert.equal(readCollapsed(storage, 'x'.repeat(513)), false)
})
test('unavailable storage leaves navigation functional', () => {
  const storage = {getItem() {throw Error('blocked')}, setItem() {throw Error('blocked')}}
  assert.equal(readCollapsed(storage, 'gateway:caller'), false)
  assert.doesNotThrow(() => writeCollapsed(storage, 'gateway:caller', true))
})
