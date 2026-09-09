import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverSourceStrip } from './discover-source-strip'
import type { DepotProviderOption } from '@/lib/api/depot-client'

const providers: DepotProviderOption[] = [{ id: 'team', name: 'Team', enabled: true, sourceOrigins: ['ard', 'mcp-registry', 'acp-registry'], health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } }]

test('all nine source identities are present in mock order with self-contained marks', () => {
  const html = renderToStaticMarkup(<DiscoverSourceStrip providers={providers} selectedProvider="all" onSelect={() => {}} />)
  const labels = [...html.matchAll(/aria-label="([^"]+)"/g)].map(match => match[1])
  assert.deepEqual(labels, ['Quick source filters', 'Filter source: MCP Registry', 'Filter source: ACP Registry',
    'skills.sh: source filter unavailable', 'Filter source: ARD', 'GitHub: source filter unavailable',
    'Claude: source filter unavailable', 'Gemini: source filter unavailable', 'Agent Plugins: source filter unavailable',
    'Web Crawl: source filter unavailable'])
  assert.equal((html.match(/<svg/g) ?? []).length, 9)
  assert.doesNotMatch(html, /<img|(?:src|href)="https?:/)
})

test('unsupported provenance stays unavailable, separate from supported selection', () => {
  const html = renderToStaticMarkup(<DiscoverSourceStrip providers={providers} selectedProvider="all" selectedOrigin="ard" onSelect={() => {}} />)
  assert.equal((html.match(/aria-disabled="true"/g) ?? []).length, 6)
  assert.equal((html.match(/aria-pressed="true"/g) ?? []).length, 1)
  assert.match(html, /aria-label="Filter source: ARD" aria-pressed="true"/)
  assert.equal((html.match(/recorded source provenance is not available/g) ?? []).length, 6)
})
