import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { LibraryTabs } from './depot-workspace-pages'

test('attached library tabs preserve routes and reserve mock-aligned count pills', () => {
  const html = renderToStaticMarkup(<LibraryTabs active="snippets" attached counts={{ snippets: 0 }} />)
  assert.match(html, /height:40px/)
  assert.match(html, /rounded-b-aurora-3/)
  assert.match(html, /href="\/library"/)
  assert.match(html, /href="\/loadouts"/)
  assert.match(html, /href="\/snippets" aria-current="page"/)
  assert.match(html, /href="\/tools"/)
  // The mock reserves one pill per tab. Unknown live counts keep that geometry
  // with an em dash instead of disappearing or inventing a value.
  assert.equal((html.match(/tabular-nums/g) ?? []).length, 4)
  assert.equal((html.match(/>—<\/span>/g) ?? []).length, 3)
  assert.match(html, />0<\/span>/)
})
