import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { NavigationSection } from './navigation-section'

test('navigation section renders breadcrumbs, tabs, and sidebar-state samples', () => {
  const markup = renderToStaticMarkup(React.createElement(NavigationSection))

  assert.match(markup, /Sidebar item states/)
  assert.match(markup, /Tabs/)
  assert.match(markup, /Breadcrumbs/)
})
