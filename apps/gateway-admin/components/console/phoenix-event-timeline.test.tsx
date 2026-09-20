import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { PhoenixEventTimeline, PhoenixRuntimeSummary, phoenixContextWindow, phoenixTotalTokens } from './phoenix-event-timeline.tsx'

test('Phoenix activity is an icon-only collapsed graph with grouped tool calls', () => {
  const html = renderToStaticMarkup(<PhoenixEventTimeline events={[
    { method: 'item/completed', params: { item: { id: 'r1', type: 'reasoning', summary: 'Checked gateway health' }, secret: 'do-not-render' } },
    { method: 'item/started', params: { item: { id: 'm1', type: 'mcpToolCall', server: 'labby', tool: 'gateway', status: 'inProgress' } } },
    { method: 'item/completed', params: { item: { id: 'm1', type: 'mcpToolCall', server: 'labby', tool: 'gateway', status: 'completed' } } },
    { method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { totalTokens: 1242 } } } },
    { method: 'future/unknown', params: { text: 'internal protocol detail' } },
  ]}/> )
  assert.match(html, /aria-label="Phoenix activity"/)
  assert.match(html, /aria-label="labby · gateway\. 2 events"/)
  assert.match(html, /aria-expanded="false"/)
  assert.doesNotMatch(html, />Activity<|>labby · gateway<|Checked gateway health|do-not-render|internal protocol detail/)
})

test('Phoenix preserves reasoning, hooks and subagents behind distinct icon nodes', () => {
  const html = renderToStaticMarkup(<PhoenixEventTimeline events={[
    { method: 'item/reasoning/textDelta', params: { delta: 'private-ish visible reasoning summary' } },
    { method: 'hook/started', params: { run: { name: 'PreToolUse' } } },
    { method: 'item/started', params: { item: { type: 'collabToolCall', receiverThreadId: 'agent-7' } } },
  ]}/> )
  assert.match(html, /aria-label="Reasoning\. 1 event"/)
  assert.match(html, /aria-label="PreToolUse\. 1 event"/)
  assert.match(html, /aria-label="agent-7\. 1 event"/)
  assert.doesNotMatch(html, /private-ish visible reasoning summary/)
})

test('Phoenix context usage honors App Server token usage notifications', () => {
  const events = [{ method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { totalTokens: 4096 }, modelContextWindow: 200_000 } } }]
  assert.equal(phoenixTotalTokens(events), 4096)
  assert.equal(phoenixContextWindow(events), 200_000)
  assert.equal(phoenixTotalTokens([{ method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { totalTokens: 50_000 }, last: { totalTokens: 12_000 }, modelContextWindow: 200_000 } } }]), 12_000)
  assert.equal(phoenixTotalTokens([{ method: 'thread/tokenUsage/updated', params: { tokenUsage: { last: { inputTokens: 1200, outputTokens: 42 } } } }]), 1242)
  assert.equal(phoenixContextWindow([{ method: 'usage', params: { usage: { model_context_window: 128_000 } } }]), 128_000)
})

test('Phoenix context usage ignores later unrelated usage events', () => {
  const events = [
    { method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { totalTokens: 4096 }, modelContextWindow: 200_000 } } },
    { method: 'account/usage/updated', params: { usage: { remaining: 0.42 } } },
  ]
  assert.equal(phoenixTotalTokens(events), 4096)
  assert.equal(phoenixContextWindow(events), 200_000)
})

test('Phoenix runtime summary truthfully distinguishes configured and unavailable MCP', () => {
  const configured = renderToStaticMarkup(<PhoenixRuntimeSummary protocol="0.147.0-v2" mcpConfigured/> )
  assert.match(configured, /v0\.147\.0/)
  assert.match(configured, /Labby MCP/)
  const unavailable = renderToStaticMarkup(<PhoenixRuntimeSummary protocol="0.147.0-v2" mcpConfigured={false}/> )
  assert.match(unavailable, /MCP unavailable/)
})

test('Phoenix shows the detected runtime and an inspectable capability count', () => {
  const html = renderToStaticMarkup(<PhoenixRuntimeSummary protocol="v2" runtimeVersion="codex-cli 0.147.0" mcpConfigured capabilities={['start', 'interrupt', 'text']} unsupported={['realtime']}/> )
  assert.match(html, /codex-cli 0\.147\.0/)
  assert.match(html, /3 capabilities/)
  assert.match(html, /Unavailable: realtime/)
})
