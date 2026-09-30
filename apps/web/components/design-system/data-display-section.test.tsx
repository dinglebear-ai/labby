import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { DataDisplaySection } from './data-display-section'

test('data display section renders metric cards, dense rows, and key/value blocks', () => {
  const markup = renderToStaticMarkup(React.createElement(DataDisplaySection))

  assert.match(markup, /Metric cards/)
  assert.match(markup, /Dense table rows/)
  assert.match(markup, /Key\/value blocks/)
})
