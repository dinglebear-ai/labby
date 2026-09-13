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
  assert.match(html, /Reasoning/)
  assert.match(html, /Checked gateway health/)
  assert.match(html, /Using labby\.gateway/)
  assert.match(html, /1,200 in · 42 out/)
  assert.doesNotMatch(html, /do-not-render|internal protocol detail|future\/unknown/)
})

test('Phoenix runtime summary truthfully distinguishes configured and unavailable MCP', () => {
  const configured = renderToStaticMarkup(<PhoenixRuntimeSummary protocol="0.147.0-v2" mcpConfigured/> )
  assert.match(configured, /v0\.147\.0/)
  assert.match(configured, /Labby MCP/)
  const unavailable = renderToStaticMarkup(<PhoenixRuntimeSummary protocol="0.147.0-v2" mcpConfigured={false}/> )
  assert.match(unavailable, /MCP unavailable/)
})
