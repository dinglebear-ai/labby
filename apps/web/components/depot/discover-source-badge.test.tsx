import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverSourceBadge } from './discover-source-badge'

test('known origins show registry identity without losing backend identity', () => {
  for (const [sourceOrigin, label] of [['mcp-registry', 'MCP Registry'], ['acp-registry', 'ACP Registry'], ['ard', 'ARD'], ['skills-sh', 'skills.sh'], ['github', 'GitHub'], ['claude', 'Claude'], ['gemini', 'Gemini'], ['agent-plugins', 'Agent Plugins'], ['web-crawl', 'Web Crawl']] as const) {
    const html = renderToStaticMarkup(<DiscoverSourceBadge artifact={{ providerId: 'team', sourceOrigin }} />)
    assert.ok(html.includes(`title="${label} · via team"`))
    assert.match(html, /class="sr-only">via team/)
    assert.match(html, /data-source-dot="1"/)
    assert.match(html, /h-\[18px\]/)
    assert.doesNotMatch(html, /<svg/)
  }
})

test('missing origins retain provider labels and do not imply healthy or verified status', () => {
  for (const sourceOrigin of [undefined, null]) {
    const html = renderToStaticMarkup(<DiscoverSourceBadge artifact={{ providerId: 'catalog', sourceOrigin }} />)
    assert.match(html, /title="catalog"/)
    assert.doesNotMatch(html, /Registry|ARD|aurora-success|verified/i)
  }
})
