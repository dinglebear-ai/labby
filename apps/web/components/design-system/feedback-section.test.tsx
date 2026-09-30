import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { FeedbackSection } from './feedback-section'

test('feedback section renders empty loading success warning and error states', () => {
  const markup = renderToStaticMarkup(React.createElement(FeedbackSection))

  assert.match(markup, /Loading/)
  assert.match(markup, /Empty state/)
  assert.match(markup, /Success/)
  assert.match(markup, /Warning/)
  assert.match(markup, /Error/)
})
