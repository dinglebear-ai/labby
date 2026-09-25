import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
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
        id: '00000000000000000001:echo-1:ui://connexin/echo.html', sequence: 1, callId: 'echo-1',
        resourceUri: 'ui://connexin/echo.html', toolResult: { ok: true },
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
        id: '00000000000000000001:countdown-1:ui://connexin/countdown.html', sequence: 1, callId: 'countdown-1',
        resourceUri: 'ui://connexin/countdown.html', toolResult: { seconds: 5 },
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
      mcp_apps: [{ id: '00000000000000000001:shutdown-1:ui://connexin/shutdown.html', sequence: 1, callId: 'shutdown-1', resourceUri: 'ui://connexin/shutdown.html', toolResult: { safe: true }, errorKind: 'upstream_error' }],
    }]}
    mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop}
  />)
  assert.match(html, /connexin · shutdown/)
  assert.match(html, /data-phoenix-mcp-app="fallback"/)
  assert.match(html, /upstream_error/)
  assert.match(html, /Underlying tool result/)
  assert.match(html, /&quot;safe&quot;: true/)
})

test('Phoenix keeps two MCP App calls distinct when they share one resource URI', () => {
  const uri = 'ui://connexin/echo.html'
  const html = renderToStaticMarkup(<PhoenixConversation
    messages={[]}
    events={[{
      method: 'item/completed', received_at_ms: 80, sequence: 8,
      params: { item: { type: 'mcpToolCall', server: 'connexin', tool: 'echo', status: 'completed' } },
      mcp_apps: [
        { id: '00000000000000000008:echo-a:' + uri, sequence: 8, callId: 'echo-a', resourceUri: uri, resource: { contents: [{ uri, mimeType: 'text/html', text: '<main>A</main>' }] } },
        { id: '00000000000000000008:echo-b:' + uri, sequence: 8, callId: 'echo-b', resourceUri: uri, resource: { contents: [{ uri, mimeType: 'text/html', text: '<main>B</main>' }] } },
      ],
    }]}
    mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop}
  />)
  assert.equal((html.match(/data-phoenix-mcp-app="ready"/g) ?? []).length, 2)
  assert.match(html, /echo-a/)
  assert.match(html, /echo-b/)
})

test('Phoenix preserves the exact MCP App iframe node across repeated rerenders and session reload data', async () => {
  installTestDom()
  const uri = 'ui://connexin/echo.html'
  const app = { id: '00000000000000000009:echo:' + uri, sequence: 9, callId: 'echo', resourceUri: uri, resource: { contents: [{ uri, mimeType: 'text/html', text: '<main>stateful</main>' }] } }
  const baseEvents = [{ method: 'item/completed', received_at_ms: 90, sequence: 9, params: { item: { type: 'mcpToolCall', server: 'connexin', tool: 'echo', status: 'completed' } }, mcp_apps: [app] }]
  const view = await renderClient(<PhoenixConversation messages={[]} events={baseEvents} mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop} />)
  try {
    const first = view.container.querySelector('iframe'); assert.ok(first)
    await view.rerender(<PhoenixConversation messages={[]} events={[...baseEvents, { method: 'thread/tokenUsage/updated', received_at_ms: 100, sequence: 10, params: { tokenUsage: { total: { totalTokens: 10 } } } }]} mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop} />)
    const afterStream = view.container.querySelector('iframe'); assert.strictEqual(afterStream, first)
    const reloadedEvents = JSON.parse(JSON.stringify(baseEvents))
    await view.rerender(<PhoenixConversation messages={[]} events={reloadedEvents} mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop} />)
    const afterReload = view.container.querySelector('iframe'); assert.strictEqual(afterReload, first)
  } finally {
    await view.unmount()
  }
})

test('Phoenix MCP App fallback recovers in place when hydration later succeeds', async () => {
  installTestDom()
  const uri = 'ui://connexin/countdown.html'
  const id = '00000000000000000011:countdown:' + uri
  const failed = [{ method: 'item/completed', received_at_ms: 110, sequence: 11, params: { item: { type: 'mcpToolCall', server: 'connexin', tool: 'countdown', status: 'completed' } }, mcp_apps: [{ id, sequence: 11, callId: 'countdown', resourceUri: uri, toolResult: { seconds: 5 }, errorKind: 'timeout' }] }]
  const view = await renderClient(<PhoenixConversation messages={[]} events={failed} mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop} />)
  try {
    assert.equal(view.container.querySelector('iframe'), null)
    assert.match(view.container.textContent ?? '', /Underlying tool result/)
    const recovered = [{ ...failed[0], mcp_apps: [{ id, sequence: 11, callId: 'countdown', resourceUri: uri, toolResult: { seconds: 5 }, resource: { contents: [{ uri, mimeType: 'text/html', text: '<main>recovered</main>' }] } }] }]
    await view.rerender(<PhoenixConversation messages={[]} events={recovered} mark={<span>PX</span>} copiedIndex={undefined} onRetry={noop} onCopy={noop} onEdit={noop} />)
    assert.ok(view.container.querySelector('iframe'))
    assert.equal(view.container.querySelector('[data-phoenix-mcp-app]')?.getAttribute('data-phoenix-mcp-app'), 'ready')
  } finally {
    await view.unmount()
  }
})
