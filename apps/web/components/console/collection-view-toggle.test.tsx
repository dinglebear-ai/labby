import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { CollectionViewToggle } from './collection-view-toggle'

test('collection view toggle exposes the standard three presentation modes', () => {
  const html = renderToStaticMarkup(
    <CollectionViewToggle value="list" onChange={() => undefined} ariaLabel="Server view" />,
  )
  assert.match(html, /data-collection-view-toggle="1"/)
  assert.match(html, /aria-label="Server view"/)
  assert.match(html, /aria-label="Table view"/)
  assert.match(html, /aria-label="List view"/)
  assert.match(html, /aria-label="Card view"/)
  assert.match(html, /aria-label="List view"[^>]*aria-pressed="true"/)
})

test('collection view toggle can expose a bounded subset of modes', () => {
  const html = renderToStaticMarkup(
    <CollectionViewToggle value="cards" modes={['list', 'cards']} onChange={() => undefined} />,
  )
  assert.doesNotMatch(html, /aria-label="Table view"/)
  assert.match(html, /aria-label="List view"/)
  assert.match(html, /aria-label="Card view"/)
})
