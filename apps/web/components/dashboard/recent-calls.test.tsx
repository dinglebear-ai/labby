import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { SurfaceTag } from './recent-calls'

test('SurfaceTag labels core_runtime and unknown backend surfaces', () => {
  const core = renderToStaticMarkup(<SurfaceTag surface="core_runtime" />)
  const future = renderToStaticMarkup(<SurfaceTag surface="future_surface" />)

  assert.match(core, /Core runtime/)
  assert.match(future, /future surface/)
})
