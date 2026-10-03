import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { readFileSync } from 'node:fs'
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

test('overview does not call unchecked credential servers nominal', () => {
  const unknown = { state: 'unknown' as const, discovered: null, exposed: null }
  const html = renderGatewayHero(gatewayWithStatus({ connected: false, healthy: false,
    capability_observation: { scope: 'credential', tools: unknown, resources: unknown, prompts: unknown, skills: unknown },
  }))
  assert.match(html, /1 not checked or idle/)
  assert.doesNotMatch(html, /all systems nominal/)
})

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


test('partial catalog keeps metric values compact with a separate disclosure', () => {
  const html = renderToStaticMarkup(<OverviewHero
    gateways={[]} live={{ totalServers: 1, connectedServers: 1, offlineServers: 0, discoveredTools: 1024, exposedTools: 1002, incompleteTools: 1, warnings: 0 }}
    metrics={undefined} activeWindow="1h" onWindowChange={() => {}} onRefresh={() => {}} loadedAt={null}
  />)
  assert.match(html, />1002</)
  assert.doesNotMatch(html, /1002\+|· incomplete/)
  assert.match(html, /role="status"[^>]*title="Tools, prompts and resources show observed exposed counts/)
  assert.match(html, /Partial catalog/)
  assert.match(html, /color:var\(--aurora-accent-pink\)/)
  assert.match(html, /color:var\(--aurora-success\)/)
  assert.match(html, /color:var\(--aurora-accent-strong\)/)
})


test('sampled token values meet large-text contrast on both themed hero gradient stops', () => {
  const html = renderToStaticMarkup(<OverviewHero
    gateways={[]}
    live={{ totalServers: 0, connectedServers: 0, offlineServers: 0, discoveredTools: 0, exposedTools: 0, warnings: 0 }}
    metrics={tokenSummary(true)} activeWindow="1h"
    onWindowChange={() => {}} onRefresh={() => {}} loadedAt={null}
  />)
  const tokenLink = html.match(/<a[^>]*href="[^"]*focus=tokens"[^>]*>[\s\S]*?<\/a>/)?.[0]
  assert.ok(tokenLink)
  const valueStyle = tokenLink.match(/<div style="([^"]+)">127<\/div>/)?.[1]
  assert.ok(valueStyle)
  const fontSize = Number(valueStyle.match(/font-size:([\d.]+)px/)?.[1])
  const fontWeight = Number(valueStyle.match(/font-weight:(\d+)/)?.[1])
  assert.ok(fontSize > 0 && fontWeight > 0, 'resolve rendered value typography')
  const minimumContrast = fontSize >= 24 || (fontSize >= 18.667 && fontWeight >= 700) ? 3 : 4.5
  const valueToken = valueStyle.match(/color:var\((--[\w-]+)\)/)?.[1]
  const gradient = html.match(/background:linear-gradient\(180deg, var\((--[\w-]+)\), var\((--[\w-]+)\)\)/)
  const overlayToken = html.match(/background:var\((--gw[\w-]+)\)/)?.[1]
  assert.ok(valueToken && gradient && overlayToken, 'resolve the rendered metric foreground and surfaces')
  const css = readFileSync(new URL('../../app/globals.css', import.meta.url), 'utf8')
  const defaults = css.match(/:root \{([^}]+)\}/)?.[1]
  assert.ok(defaults)
  const luminance = (rgb: number[]) => rgb.map(channel => {
    const value = channel / 255
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  }).reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0)
  for (const theme of ['dark', 'light']) {
    const block = css.match(new RegExp(`\\.${theme} \\{([^}]+)\\}`))?.[1]
    assert.ok(block, `find ${theme} theme`)
    const token = (name: string) => {
      const declaration = new RegExp(`${name}:\\s*([^;]+);`)
      const value = block.match(declaration)?.[1] ?? defaults.match(declaration)?.[1]
      assert.ok(value, `${theme} defines ${name}`)
      return value
    }
    const rgb = (value: string) => {
      assert.match(value, /^#[0-9a-f]{6}$/i)
      return [1, 3, 5].map(index => Number.parseInt(value.slice(index, index + 2), 16))
    }
    const foreground = luminance(rgb(token(valueToken)))
    const overlay = token(overlayToken).match(/^rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)$/)
    assert.ok(overlay)
    const alpha = Number(overlay[4])
    for (const stop of gradient.slice(1)) {
      const background = luminance(rgb(token(stop)).map((channel, index) => channel * (1 - alpha) + Number(overlay[index + 1]) * alpha))
      const ratio = (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05)
      assert.ok(ratio >= minimumContrast, `${theme} token value on ${stop}: ${ratio.toFixed(2)}:1 is below ${minimumContrast}:1 for ${fontSize}px/${fontWeight}`)
    }
  }
})
