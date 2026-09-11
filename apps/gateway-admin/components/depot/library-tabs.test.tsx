import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { LibraryTabs } from './depot-workspace-pages'

test('attached library tabs preserve routes and show only known counts', () => {
  const html = renderToStaticMarkup(<LibraryTabs active="snippets" attached counts={{ snippets: 0 }} />)
  assert.match(html, /h-\[38px\]/)
  assert.match(html, /rounded-b-aurora-3/)
  assert.match(html, /href="\/library"/)
  assert.match(html, /href="\/loadouts"/)
  assert.match(html, /href="\/snippets" aria-current="page"/)
  assert.equal((html.match(/>—</g) ?? []).length, 2)
  assert.match(html, />0<\/span>/)
})
