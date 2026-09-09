import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DevContainerCard } from './dev-container-card'

test('container cards use compact SVG stacks and expose missing runtime data honestly', () => {
  const html = renderToStaticMarkup(<DevContainerCard row={['Ready', 'base', 'Ubuntu 24.04', 'Node · Python · Rust', '38 pulls']}/>)
  assert.match(html, /data-brand-slug="nodedotjs"/)
  assert.match(html, /aria-label="Ready"/)
  assert.match(html, /Runtime measurements unavailable/)
  assert.match(html, /Mounts unavailable/)
  assert.match(html, /Ports unavailable/)
  assert.match(html, /Recorded pulls/)
  assert.doesNotMatch(html, /<button|https:|34%|4.2 \/ 8 GB/)
})

test('building cards show their supplied stage without inventing pull counts', () => {
  const html = renderToStaticMarkup(<DevContainerCard row={['Building', 'edge', 'Alpine 3.21', 'Go · Docker', 'Layer 4/7']}/>)
  assert.match(html, /Layer 4\/7/)
  assert.match(html, /aria-label="Building"/)
  assert.doesNotMatch(html.replace(/<[^>]*>/g, ''), /38/)
})
