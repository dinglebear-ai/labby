import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { AppHeader } from './app-header'

test('multi-level headers expose a compact mobile breadcrumb contract', async () => {
  const markup = renderToStaticMarkup(
    <AppHeader breadcrumbs={[{ label: 'Gateway', href: '/gateways' }, { label: 'Github Server' }]} />,
  )

  assert.match(markup, /data-crumb-parent="1"/)
  assert.match(markup, /data-crumb-separator="1"/)
  assert.match(markup, /data-crumbleaf="1"/)

  const css = await readFile(new URL('../app/globals.css', import.meta.url), 'utf8')
  assert.match(css, /@media \(max-width: 520px\)[\s\S]*\[data-crumb-parent\]/)
  assert.match(css, /header\[data-topbar\]:has\(\[data-actioncluster\] > \*\)/)
  assert.match(css, /header\[data-topbar\] \[data-actioncluster\][\s\S]*overflow-x: auto/)
})
