import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { OverviewHero } from './overview-hero'
import type { Gateway } from '@/lib/types/gateway'
import { aggregateGatewayUsage } from '@/lib/dashboard/gateway-usage-adapter'

test('desktop refresh control keeps its recency label visible', () => {
  const html = renderToStaticMarkup(
    <OverviewHero
      gateways={[]}
      live={{
        totalServers: 0,
        connectedServers: 0,
        offlineServers: 0,
        discoveredTools: 0,
        incompleteTools: 0,
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

  assert.match(html, /data-console-hero-actions-mixed="1"/)
  assert.doesNotMatch(html, /data-console-hero-actions="1"/)
  assert.match(html, /data-icon-text-control="1" data-visible-label="1"/)
  assert.match(html, /updated 0s ago/)
  assert.ok(html.includes('>24h<'))
  assert.ok(html.includes('>7d<'))
  assert.ok(html.includes('>30d<'))
  assert.ok(html.includes('>Manage Servers<'))
  assert.ok(html.includes('>1h<'))
  assert.equal((html.match(/data-visible-label="1"/g) ?? []).length, 6)
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


function tokenSummary(collected: boolean) {
  const metrics = aggregateGatewayUsage('1h', 1_800_000_000_000, {
    window_total_calls: 0, total_calls: 0, error_calls: 0,
    avg_elapsed_ms: 0, p50_elapsed_ms: 0, p95_elapsed_ms: 0, p99_elapsed_ms: 0,
    distinct_tools: 0, distinct_actors: 0, peak_per_min: 0,
    top_tools: [], least_tools: [], top_actors: [], slowest_tools: [], errors: [],
    upstreams: [], hourly: [], timeseries: [],
    facets: { tools: [], actors: [], upstreams: [], outcomes: [] },
  })
  metrics.collected.tokens = collected
  metrics.tokens.total = 127
  return metrics
}

test('overview labels sampled tokens and never turns unavailable token telemetry into zero', () => {
  for (const collected of [true, false]) {
    const html = renderToStaticMarkup(<OverviewHero
      gateways={[]}
      live={{ totalServers: 0, connectedServers: 0, offlineServers: 0, discoveredTools: 0, exposedTools: 0, warnings: 0 }}
      metrics={tokenSummary(collected)} activeWindow="1h"
      onWindowChange={() => {}} onRefresh={() => {}} loadedAt={null}
    />)
    const tokenLink = html.match(/<a[^>]*href="[^"]*focus=tokens"[^>]*>[\s\S]*?<\/a>/)?.[0]
    assert.ok(tokenLink, 'token drilldown remains available')
    assert.match(tokenLink, /Tokens \(sample\)/)
    if (collected) assert.match(tokenLink, />127</)
    else {
      assert.match(tokenLink, />—</)
      assert.doesNotMatch(tokenLink, />127</)
    }
  }
})
