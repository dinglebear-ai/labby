import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DashboardPanel } from './panel'

test('inline panels remove nested card chrome and match expanded-row spacing', () => {
  const html = renderToStaticMarkup(<DashboardPanel variant="inline" title="Profile" action={<button>Edit</button>}>Details</DashboardPanel>)
  assert.match(html, /data-panel-variant="inline"/)
  assert.match(html, /border-radius:0/)
  assert.match(html, /border:none/)
  assert.match(html, /box-shadow:none/)
  assert.match(html, /padding:10px 16px/)
  assert.match(html, /gap:11px;padding:12px 16px 14px/)
  assert.match(html, />Edit</)
  assert.doesNotMatch(html, /data-hovercard/)
})

test('default panels retain their standalone card treatment', () => {
  const html = renderToStaticMarkup(<DashboardPanel title="Card">Details</DashboardPanel>)
  assert.match(html, /data-hovercard="1"/)
  assert.match(html, /border-radius:var\(--radius-2\)/)
  assert.match(html, /padding:12px 14px/)
})
