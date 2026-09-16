import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { OverviewHero } from './overview-hero'

test('desktop refresh control keeps its recency label visible', () => {
  const html = renderToStaticMarkup(
    <OverviewHero
      gateways={[]}
      live={{
        totalServers: 0,
        connectedServers: 0,
        offlineServers: 0,
        discoveredTools: 0,
        exposedTools: 0,
        warnings: 0,
      }}
      metrics={undefined}
      activeWindow="24h"
      onWindowChange={() => {}}
      onRefresh={() => {}}
      loadedAt={Date.now()}
    />,
  )

  assert.match(html, /data-icon-text-control="1" data-visible-label="1"/)
  assert.match(html, /updated 0s ago/)
})
