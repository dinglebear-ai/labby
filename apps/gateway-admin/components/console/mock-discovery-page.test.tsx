import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ConsoleShellProvider } from '@/components/console/console-shell-context'
import { MockDiscoveryPage } from './mock-discovery-page'

test('Discovery mirrors the Bazaar structure while marking every fixture region', () => {
  const markup = renderToStaticMarkup(<ConsoleShellProvider><MockDiscoveryPage /></ConsoleShellProvider>)
  assert.match(markup, /Depot · Bazaar/)
  assert.match(markup, /Trending This Week/)
  assert.match(markup, /Search 26 artifacts/)
  assert.match(markup, /data-mock-region="discovery"/)
  assert.match(markup, /data-mock-region="discovery-card"/)
  assert.match(markup, /data-mock-surface="true"/)
  assert.match(markup, /no controls call a Labby service/i)
  assert.match(markup, /disabled=""/)
})
