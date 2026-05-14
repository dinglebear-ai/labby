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
  assert.equal(labels.includes('Chat'), true)
  assert.equal(labels.includes('Design System'), false)
})
