import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { PhoenixConversation } from './phoenix-conversation.tsx'

const noop = () => undefined

test('Phoenix interleaves streamed assistant text and tool activity in execution order', () => {
  const html = renderToStaticMarkup(<PhoenixConversation
    messages={[
      { role: 'user', text: 'Investigate it', created_at_ms: 100 },
      { role: 'assistant', text: 'First chunk second chunk', created_at_ms: 500 },
    ]}
    events={[
      { method: 'item/agentMessage/delta', received_at_ms: 200, sequence: 1, params: { turnId: 't1', delta: 'First chunk ' } },
      { method: 'item/started', received_at_ms: 300, sequence: 2, params: { turnId: 't1', item: { id: 'tool-1', type: 'mcpToolCall', server: 'labby', tool: 'gateway.status' } } },
      { method: 'item/completed', received_at_ms: 350, sequence: 3, params: { turnId: 't1', item: { id: 'tool-1', type: 'mcpToolCall', server: 'labby', tool: 'gateway.status', status: 'completed' } } },
      { method: 'item/agentMessage/delta', received_at_ms: 400, sequence: 4, params: { turnId: 't1', delta: 'second chunk' } },
    ]}
    mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop}
  />)
  const first = html.indexOf('First chunk')
  const tool = html.indexOf('labby · gateway.status')
  const second = html.indexOf('second chunk')
  assert.ok(first >= 0 && tool > first && second > tool, html)
  assert.equal((html.match(/First chunk second chunk/g) ?? []).length, 0, 'final persisted assistant message must not duplicate streamed text')
})

test('Phoenix keeps reasoning collapsed while preserving its chronological slot', () => {
  const html = renderToStaticMarkup(<PhoenixConversation
    messages={[{ role: 'user', text: 'Go', created_at_ms: 10 }]}
    events={[
      { method: 'item/agentMessage/delta', received_at_ms: 20, sequence: 1, params: { turnId: 't2', delta: 'Before ' } },
      { method: 'item/reasoning/textDelta', received_at_ms: 30, sequence: 2, params: { turnId: 't2', delta: 'hidden reasoning details' } },
      { method: 'item/agentMessage/delta', received_at_ms: 40, sequence: 3, params: { turnId: 't2', delta: 'after' } },
    ]}
    mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop}
  />)
  assert.ok(html.indexOf('Before') < html.indexOf('aria-label="Reasoning. 1 event"'))
  assert.ok(html.indexOf('aria-label="Reasoning. 1 event"') < html.indexOf('after'))
  assert.doesNotMatch(html, /hidden reasoning details/)
})

test('Phoenix renders a hydrated direct MCP App inline while preserving tool activity', () => {
  const html = renderToStaticMarkup(<PhoenixConversation
    messages={[]}
    events={[{
      method: 'item/completed', received_at_ms: 50, sequence: 1,
      params: { item: { type: 'mcpToolCall', server: 'connexin', tool: 'echo', status: 'completed' } },
      mcp_apps: [{
        resourceUri: 'ui://connexin/echo.html',
        resource: { contents: [{ uri: 'ui://connexin/echo.html', mimeType: 'text/html;profile=mcp-app', text: '<main>Connexin Echo</main>' }] },
      }],
    }]}
    mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop}
  />)
  assert.match(html, /connexin · echo/)
  assert.match(html, /data-phoenix-mcp-app="ready"/)
  assert.match(html, /ui:\/\/connexin\/echo\.html MCP UI/)
  assert.match(html, /Connexin Echo/)
})

test('Phoenix renders nested Code Mode MCP Apps from hydrated events', () => {
  const html = renderToStaticMarkup(<PhoenixConversation
    messages={[]}
    events={[{
      method: 'item/completed', received_at_ms: 60, sequence: 1,
      params: { item: { type: 'mcpToolCall', server: 'labby', tool: 'codemode', status: 'completed' } },
      mcp_apps: [{
        resourceUri: 'ui://connexin/countdown.html',
        resource: { contents: [{ uri: 'ui://connexin/countdown.html', mime_type: 'text/html;profile=mcp-app', text: '<main>Countdown</main>' }] },
      }],
    }]}
    mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop}
  />)
  assert.match(html, /labby · codemode/)
  assert.match(html, /ui:\/\/connexin\/countdown\.html MCP UI/)
  assert.match(html, /Countdown/)
})

test('Phoenix falls back safely when MCP App hydration fails', () => {
  const html = renderToStaticMarkup(<PhoenixConversation
    messages={[]}
    events={[{
      method: 'item/completed', received_at_ms: 70, sequence: 1,
      params: { item: { type: 'mcpToolCall', server: 'connexin', tool: 'shutdown', status: 'completed' } },
      mcp_apps: [{ resourceUri: 'ui://connexin/shutdown.html', errorKind: 'upstream_error' }],
    }]}
    mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop}
  />)
  assert.match(html, /connexin · shutdown/)
  assert.match(html, /data-phoenix-mcp-app="fallback"/)
  assert.match(html, /upstream_error/)
  assert.match(html, /underlying tool result is still preserved/)
})
