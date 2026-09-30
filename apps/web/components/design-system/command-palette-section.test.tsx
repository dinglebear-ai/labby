import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { CommandPaletteSection } from './command-palette-section'

test('command palette section renders hybrid spotlight content and mixed result types', () => {
  const markup = renderToStaticMarkup(React.createElement(CommandPaletteSection))

  assert.match(markup, /Hybrid spotlight navigation and actions/)
  assert.match(markup, /Search pages, commands, and recent context/)
  assert.match(markup, /Gateway Admin/)
  assert.match(markup, /Reload gateways/)
  assert.match(markup, /edge-proxy-prod/)
  assert.match(markup, /Destination/)
  assert.match(markup, /Action/)
  assert.match(markup, /Recent/)
  assert.match(markup, /aurora-scrollbar/)
})
