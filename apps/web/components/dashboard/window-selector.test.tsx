import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { WindowSelector } from './window-selector'

test('Activity window selector matches the mock pill geometry', () => {
  const markup = renderToStaticMarkup(<WindowSelector value="24h" onChange={() => {}} />)
  assert.match(markup, /role="tablist"[^>]*style="[^"]*gap:3px;padding:3px;border-radius:999px/)
  assert.match(markup, /aria-selected="true"[^>]*style="[^"]*height:28px;padding:0 13px;border-radius:999px/)
  assert.match(markup, /font-size:11.5px;font-weight:650/)
})


test('Activity retains one-hour drilldowns and exposes the thirty-day window', () => {
  const markup = renderToStaticMarkup(<WindowSelector value="30d" onChange={() => {}} />)
  assert.ok(markup.includes('>1h</button>'))
  assert.match(markup, /aria-selected="true"[^>]*>30d<\/button>/)
})
