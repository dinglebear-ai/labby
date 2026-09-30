import test from 'node:test'
import assert from 'node:assert/strict'

import { buildCommandPaletteState } from './command-palette-model'

test('empty query returns mixed groups and a default active item', () => {
  const state = buildCommandPaletteState('')

  assert.ok(state.items.length > 0)
  assert.equal(state.groups[0]?.label, 'Best match')
  assert.ok(state.groups.some((group) => group.label === 'Suggested actions'))
  assert.ok(state.groups.some((group) => group.label === 'Recent context'))
  assert.equal(state.activeItemId, state.items[0]?.id)
})

test('gate query ranks Gateway Admin before Reload gateways', () => {
  const state = buildCommandPaletteState('gate')

  assert.equal(state.items[0]?.title, 'Gateway Admin')
  assert.equal(state.items[1]?.title, 'Reload gateways')
})

test('logs query ranks Logs before Tail local logs', () => {
  const state = buildCommandPaletteState('logs')

  assert.equal(state.items[0]?.title, 'Logs')
  assert.equal(state.items[1]?.title, 'Tail local logs')
})

test('unknown query returns no matches', () => {
  const state = buildCommandPaletteState('zzzz-no-match')

  assert.equal(state.items.length, 0)
  assert.equal(state.groups.length, 0)
  assert.equal(state.activeItemId, null)
})
