import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { PaletteAddServer } from './palette-add-server'

test('inline Add Server preserves both reference completion paths', () => {
  const markup = renderToStaticMarkup(
    <PaletteAddServer isSubmitting={false} onOpenFullDialog={() => {}} onSubmit={() => {}} />,
  )

  assert.match(markup, /Command or endpoint/)
  assert.match(markup, /Full Dialog/)
  assert.match(markup, /Add &amp; Probe/)
})
