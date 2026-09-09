import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DevContainersPageContent } from './dev-containers-page-content'

test('container page follows the mock card-only grid without an extra view toolbar', () => {
  const html = renderToStaticMarkup(<DevContainersPageContent/>)
  assert.match(html, /aria-label="Container images"/)
  assert.match(html, /repeat\(auto-fill,minmax\(min\(310px,100%\),1fr\)\)/)
  assert.match(html, /items-start gap-3/)
  assert.equal((html.match(/<section data-hovercard/g) ?? []).length, 3)
  assert.doesNotMatch(html, /Container views|Table view|List view|Cards view|<table/)
  assert.match(html, /New Container/)
})
