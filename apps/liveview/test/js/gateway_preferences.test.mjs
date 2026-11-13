import test from 'node:test'
import assert from 'node:assert/strict'
import {readPreferences, writePreferences, normalizePreferences} from '../../assets/js/gateway_preferences.mjs'
const key = 'a'.repeat(64)
const other = 'b'.repeat(64)
function storage() {
  const data = new Map()
  return {getItem: name => data.get(name), setItem: (name, value) => data.set(name, value), data}
}
test('preferences isolate verified namespaces and strip records and credentials', () => {
  const store = storage()
  assert.equal(writePreferences(store, key, {layout: 'cards', query: 'fetch', columns: ['tools', 'secret'], rows: [{token: 'secret'}]}), true)
  assert.deepEqual(readPreferences(store, other), {})
  assert.deepEqual(readPreferences(store, key), {layout: 'cards', query: 'fetch', columns: ['tools']})
  assert.equal(writePreferences(store, 'unsafe', {}), false)
  assert.equal(JSON.stringify([...store.data]).includes('secret'), false)
})
test('invalid, oversized and unavailable storage is bounded', () => {
  assert.deepEqual(readPreferences({getItem: () => '{broken'}, key), {})
  assert.deepEqual(readPreferences({getItem: () => 'x'.repeat(9000)}, key), {})
  assert.equal(writePreferences({setItem() { throw Error('disabled') }}, key, {}), false)
  assert.deepEqual(normalizePreferences({layout: 'invalid', columns: ['tools', 'tools'], direction: 'secret', query: 'x'.repeat(500)}), {columns: ['tools']})
})

test('hook ignores foreign namespaces, resets on namespace change and removes listeners', async () => {
  const {default: hook} = await import('../../assets/js/gateway_preferences.mjs')
  const store = storage()
  const listeners = new Map()
  const oldWindow = globalThis.window
  const oldDocument = globalThis.document
  globalThis.window = {localStorage: store}
  globalThis.document = {
    addEventListener: (event, handler) => listeners.set(event, handler),
    removeEventListener: event => listeners.delete(event)
  }
  let receiveState
  const pushed = []
  let removed = false
  const context = {
    ...hook,
    el: {dataset: {preferenceKey: key}, addEventListener() {}, removeEventListener() {}, querySelectorAll: () => []},
    handleEvent: (_, handler) => { receiveState = handler; return 'event-ref' },
    removeHandleEvent: ref => { removed = ref === 'event-ref' },
    pushEvent: (event, payload, done) => { pushed.push({event, payload}); done?.() }
  }
  try {
    hook.mounted.call(context)
    receiveState({key: other, preferences: {layout: 'cards'}})
    assert.equal(store.data.size, 0)
    receiveState({key, preferences: {layout: 'cards'}})
    assert.equal(readPreferences(store, key).layout, 'cards')
    context.el.dataset.preferenceKey = other
    hook.updated.call(context)
    assert.deepEqual(pushed.at(-1), {event: 'gateway_preferences', payload: {preferences: {}}})
    receiveState({key, preferences: {layout: 'list'}})
    assert.deepEqual(readPreferences(store, other), {})
    hook.destroyed.call(context)
    assert.equal(listeners.size, 0)
    assert.equal(removed, true)
    assert.equal(context.key, null)
  } finally {
    globalThis.window = oldWindow
    globalThis.document = oldDocument
  }
})
