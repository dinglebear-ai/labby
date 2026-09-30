import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ControlsSection } from './controls-section'

test('controls section renders primary controls plus fake async and destructive states', () => {
  const markup = renderToStaticMarkup(React.createElement(ControlsSection))

  assert.match(markup, /Button hierarchy/)
  assert.match(markup, /data-selected glow/)
  assert.match(markup, /Pill filters/)
  assert.match(markup, /Loading state/)
  assert.match(markup, /Destructive action/)
})
