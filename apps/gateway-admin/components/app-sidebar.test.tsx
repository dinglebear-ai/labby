import test from 'node:test'
import assert from 'node:assert/strict'

import {
  primarySidebarNavigation,
  secondarySidebarNavigation,
} from './app-sidebar'

test('app sidebar navigation excludes design system route', () => {
  const labels = [
    ...primarySidebarNavigation.map((item) => item.title),
    ...secondarySidebarNavigation.map((item) => item.title),
  ]

  assert.equal(labels.includes('Gateway'), true)
  assert.equal(labels.includes('Servers'), false)
  assert.equal(labels.includes('Snippets'), true)
  assert.equal(labels.includes('Design System'), false)

  // Removed surfaces (no backing service): assert each is gone, not just Chat.
  for (const removed of ['Nodes', 'Marketplace', 'Chat', 'Setup', 'Activity', 'Logs']) {
    assert.equal(labels.includes(removed), false, `expected "${removed}" to be removed from nav`)
  }
})

test('snippets is a high-level primary navigation item', () => {
  const snippets = primarySidebarNavigation.find((item) => item.title === 'Snippets')

  assert.ok(snippets)
  assert.equal(snippets.url, '/snippets')
  assert.equal(primarySidebarNavigation.indexOf(snippets), 2)
})
