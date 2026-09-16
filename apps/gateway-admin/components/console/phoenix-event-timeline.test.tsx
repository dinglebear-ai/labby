import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { PhoenixEventTimeline, PhoenixRuntimeSummary } from './phoenix-event-timeline.tsx'

test('Phoenix renders only recognized App Server events without dumping raw protocol payloads', () => {
  const html = renderToStaticMarkup(<PhoenixEventTimeline events={[
    { method: 'item/completed', params: { item: { type: 'reasoning', summary: 'Checked gateway health' }, secret: 'do-not-render' } },
    { method: 'item/started', params: { item: { type: 'mcpToolCall', name: 'labby.gateway' } } },
    { method: 'thread/tokenUsage/updated', params: { tokenUsage: { inputTokens: 1200, outputTokens: 42 } } },
    { method: 'future/unknown', params: { text: 'internal protocol detail' } },
  ]}/>)
  assert.match(html, /1 tool call · 1 reasoning step/)
  assert.match(html, /aria-expanded="false"/)
  assert.doesNotMatch(html, /Checked gateway health|Using labby\.gateway|1,200 in · 42 out/)
  assert.doesNotMatch(html, /do-not-render|internal protocol detail|future\/unknown/)
})

test('Phoenix runtime summary truthfully distinguishes configured and unavailable MCP', () => {
  const configured = renderToStaticMarkup(<PhoenixRuntimeSummary protocol="0.147.0-v2" mcpConfigured/> )
  assert.match(configured, /v0\.147\.0/)
  assert.match(configured, /Labby MCP/)
  const unavailable = renderToStaticMarkup(<PhoenixRuntimeSummary protocol="0.147.0-v2" mcpConfigured={false}/> )
  assert.match(unavailable, /MCP unavailable/)
})

test('Phoenix shows the detected runtime and an inspectable capability count', () => {
  const html = renderToStaticMarkup(<PhoenixRuntimeSummary
    protocol="v2"
    runtimeVersion="codex-cli 0.147.0"
    mcpConfigured
    capabilities={['start', 'interrupt', 'text']}
    unsupported={['realtime']}
  />)
  assert.match(html, /codex-cli 0\.147\.0/)
  assert.match(html, /3 capabilities/)
  assert.match(html, /Unavailable: realtime/)
})

test('Phoenix summarizes streamed text and nested token usage without exposing raw JSON', () => {
  const html = renderToStaticMarkup(<PhoenixEventTimeline events={[
    { method: 'item/agentMessage/delta', params: { delta: 'Shipping the answer' } },
    { method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { inputTokens: 1200, cachedInputTokens: 800, outputTokens: 42 } } } },
  ]}/>)
  assert.match(html, /2 updates/)
  assert.match(html, /aria-expanded="false"/)
  assert.doesNotMatch(html, /Shipping the answer|1,200 in · 800 cached · 42 out/)
  assert.doesNotMatch(html, /inputTokens/)
})
