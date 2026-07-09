import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { PatternsSection } from './patterns-section'

test('application pattern demos render gateway toolbar and auth/setup states', () => {
  const markup = renderToStaticMarkup(React.createElement(PatternsSection))

  assert.match(markup, /Gateway toolbar pattern/)
  assert.match(markup, /Auth and setup states/)
})
