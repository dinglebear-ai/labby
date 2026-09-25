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

test('Phoenix groups adjacent tool calls but keeps separate calls and boundaries', () => {
  const html = renderToStaticMarkup(<PhoenixEventTimeline events={[
    { method: 'item/started', params: { item: { id: 'm1', type: 'mcpToolCall', server: 'labby', tool: 'gateway.status', status: 'inProgress' } } },
    { method: 'item/completed', params: { item: { id: 'm1', type: 'mcpToolCall', server: 'labby', tool: 'gateway.status', status: 'completed' } } },
    { method: 'item/completed', params: { item: { id: 'm2', type: 'mcpToolCall', server: 'labby', tool: 'gateway.status', status: 'completed' } } },
    { method: 'item/completed', params: { item: { id: 'c1', type: 'commandExecution', command: 'inspect logs', status: 'completed' } } },
    { method: 'item/completed', params: { item: { id: 'm4', type: 'mcpToolCall', server: 'labby', tool: 'gateway.health', status: 'completed' } } },
    { method: 'item/reasoning/textDelta', params: { delta: 'considering results' } },
    { method: 'item/completed', params: { item: { id: 'm3', type: 'mcpToolCall', server: 'labby', tool: 'server_logs.search', status: 'completed' } } },
  ]}/>)
  assert.match(html, /data-phoenix-tool-group="4"/)
  assert.match(html, /aria-label="4 tool calls: labby · gateway.status, 2 calls; Command; labby · gateway.health"/)
  assert.equal((html.match(/data-phoenix-tool=/g) ?? []).length, 3)
  assert.match(html, /data-phoenix-tool-count="2"/)
  assert.doesNotMatch(html, />4 tool calls</)
  assert.equal((html.match(/data-phoenix-tool-group=/g) ?? []).length, 1)
  assert.ok(html.indexOf('data-phoenix-tool-group="4"') < html.indexOf('aria-label="Reasoning. 1 event"'))
  assert.ok(html.indexOf('aria-label="Reasoning. 1 event"') < html.indexOf('aria-label="labby · server_logs.search. 1 event"'))
  assert.doesNotMatch(html, /considering results/)
})

test('Phoenix context usage honors App Server token usage notifications', () => {
  const events = [{ method: 'thread/tokenUsage/updated', params: { tokenUsage: { total: { totalTokens: 4096 }, modelContextWindow: 200_000 } } }]
  assert.equal(phoenixTotalTokens(events), 4096)
  assert.equal(phoenixContextWindow(events), 200_000)
  assert.equal(phoenixTotalTokens([{ method: 'thread/tokenUsage/updated', params: { tokenUsage: { last: { inputTokens: 1200, outputTokens: 42 } } } }]), 1242)
  assert.equal(phoenixContextWindow([{ method: 'usage', params: { usage: { model_context_window: 128_000 } } }]), 128_000)
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
