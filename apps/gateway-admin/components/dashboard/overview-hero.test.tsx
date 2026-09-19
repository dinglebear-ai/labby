import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { OverviewHero } from './overview-hero'
import type { Gateway } from '@/lib/types/gateway'

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

function gatewayWithStatus(status: Partial<Gateway['status']>): Gateway {
  return {
    id: 'gw',
    name: 'gw',
    transport: 'stdio',
    enabled: true,
    config: { command: '/usr/bin/ssh' },
    discovery: { tools: [], resources: [], prompts: [] },
    warnings: [],
    status: {
      connected: true,
      healthy: true,
      discovered_tool_count: 1,
      exposed_tool_count: 1,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
      ...status,
    },
  }
}

function renderGatewayHero(gateway: Gateway): string {
  return renderToStaticMarkup(
    <OverviewHero
      gateways={[gateway]}
      live={{ totalServers: 1, connectedServers: 1, offlineServers: 0, discoveredTools: 1, exposedTools: 1, warnings: 0 }}
      metrics={undefined}
      activeWindow="24h"
      onWindowChange={() => {}}
      onRefresh={() => {}}
      loadedAt={Date.now()}
    />,
  )
}

test('overview treats catalog warming as discovery instead of an alert', () => {
  const html = renderGatewayHero(gatewayWithStatus({ healthy: false, catalog_warming: true }))
  assert.match(html, /1 discovering/)
  assert.match(html, /0\/1 healthy/)
  assert.doesNotMatch(html, /needs attention/)
})

test('overview includes stale runtime state in needs-attention semantics', () => {
  const html = renderGatewayHero(gatewayWithStatus({ likely_stale_count: 1 }))
  assert.match(html, /1 needs attention/)
  assert.match(html, /0\/1 healthy/)
})
