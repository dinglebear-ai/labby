import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { UsageCallDetailContent } from './usage-call-detail'
import type { ToolCallRecord } from '@/lib/types/metrics'

const call: ToolCallRecord = {
  id: 'call-1',
  ts: Date.now() - 60_000,
  tool: 'dozzle::get_logs',
  action: 'tool.call',
  capability: 'tools',
  agent_id: 'agent-1',
  agent_label: 'Claude Code · jmagar',
  agent_kind: 'agent',
  ip: '192.0.2.40',
  surface: 'mcp',
  outcome: 'failed',
  error_kind: 'upstream_timeout',
  input_tokens: 120,
  output_tokens: 880,
  elapsed_ms: 1700,
  response_bytes: null,
  subject_scoped: true,
}

test('usage call detail presents retained call facts and keeps traces as an explicit action', () => {
  const html = renderToStaticMarkup(
    <UsageCallDetailContent call={call} tokensCollected ipsCollected />,
  )
  assert.match(html, /dozzle::get_logs/)
  assert.match(html, /tool\.call/)
  assert.match(html, /Claude Code · jmagar/)
  assert.match(html, /upstream_timeout/)
  assert.match(html, /Subject-scoped/)
  assert.match(html, /192\.0\.2\.40/)
  assert.match(html, /View traces/)
  assert.match(html, /MCP/)
  assert.match(html, /Input/)
  assert.match(html, /Output/)
  assert.match(html, /Total/)
})

test('usage call detail omits telemetry dimensions that were not collected', () => {
  const html = renderToStaticMarkup(
    <UsageCallDetailContent call={call} tokensCollected={false} ipsCollected={false} />,
  )
  assert.doesNotMatch(html, /Source IP/)
  assert.doesNotMatch(html, />Tokens</)
})
