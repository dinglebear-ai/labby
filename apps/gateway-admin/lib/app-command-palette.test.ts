import assert from 'node:assert/strict'
import test from 'node:test'

import {
  appCommandItems,
  buildAppCommandState,
  findAppCommandItemById,
} from './app-command-palette'

test('app command palette ranks server searches first', () => {
  const state = buildAppCommandState('server')

  assert.equal(state.activeItemId, 'destination-gateways')
  assert.equal(state.groups[0]?.key, 'best-match')
  assert.equal(state.groups[0]?.items[0]?.href, '/gateways')
  assert.equal(state.groups[0]?.items[0]?.title, 'Gateway')
})

test('app command palette includes core admin destinations', () => {
  const hrefs = new Set(appCommandItems.map((item) => item.href))

  for (const href of [
    '/',
    '/gateways',
    '/snippets',
    '/usage',
    '/settings',
    '/docs',
  ]) {
    assert.equal(hrefs.has(href), true, `${href} should be searchable`)
  }

  // Removed surfaces (no backing service): must not be advertised as destinations.
  for (const href of ['/marketplace', '/chat', '/setup', '/activity', '/logs', '/registry']) {
    assert.equal(hrefs.has(href), false, `${href} should not be searchable — surface was removed`)
  }
})

test('app command palette reports empty state for unmatched queries', () => {
  const state = buildAppCommandState('zzzz-no-match')

  assert.equal(state.activeItemId, null)
  assert.deepEqual(state.items, [])
  assert.deepEqual(state.groups, [])
})

test('findAppCommandItemById returns matching command item', () => {
  const item = findAppCommandItemById('destination-usage', appCommandItems)

  assert.equal(item?.title, 'Usage')
  assert.equal(item?.href, '/usage')
})
