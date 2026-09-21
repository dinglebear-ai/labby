import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
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

test('Phoenix renders direct and Code Mode MCP Apps inline without removing tool fallback', () => {
  const direct = renderToStaticMarkup(<PhoenixConversation messages={[]} events={[
    { method: 'item/agentMessage/delta', received_at_ms: 10, sequence: 1, params: { turnId: 'direct', delta: 'Before app ' } },
    { method: 'item/started', received_at_ms: 20, sequence: 2, params: { item: { id: 'direct-1', type: 'mcpToolCall', server: 'labby', tool: 'weather', appContext: { resourceUri: 'ui://weather/app.html' } } } },
    { method: 'item/completed', received_at_ms: 30, sequence: 3, params: { item: { id: 'direct-1', type: 'mcpToolCall', status: 'completed', server: 'labby', tool: 'weather', appContext: { resourceUri: 'ui://weather/app.html' }, result: { content: [{ type: 'text', text: 'Cloudy' }] } } } },
    { method: 'item/agentMessage/delta', received_at_ms: 40, sequence: 4, params: { turnId: 'direct', delta: 'after app' } },
  ]} mark={<span>PX</span>} onRetry={noop} onCopy={noop} onEdit={noop}/>)
  assert.match(direct, /data-mcp-app="ui:\/\/weather\/app.html"/)
  assert.match(direct, /aria-label="labby · weather\. 2 events"/)
  assert.equal((direct.match(/data-mcp-app=/g) ?? []).length, 1, 'only the completed event creates an app')
  assert.ok(direct.indexOf('Before app') < direct.indexOf('data-mcp-app='), direct)
  assert.ok(direct.indexOf('data-mcp-app=') < direct.indexOf('after app'), direct)

  const nested = renderToStaticMarkup(<PhoenixConversation messages={[]} events={[{ method: 'item/completed', params: { item: { id: 'code-1', type: 'mcpToolCall', status: 'completed', server: 'labby', tool: 'codemode', result: { structuredContent: { kind: 'code_mode_execute_trace', call_count: 3, calls: [
    { id: 'alpha::chart', namespace: 'alpha', tool: 'chart', ok: true, elapsed_ms: 2, ui: { resourceUri: 'ui://alpha/chart.html' } },
    { id: 'alpha::plain', namespace: 'alpha', tool: 'plain', ok: true, elapsed_ms: 1 },
    { id: 'beta::map', namespace: 'beta', tool: 'map', ok: true, elapsed_ms: 3, ui: { resourceUri: 'ui://beta/map.html' } },
  ], result_shape: { type: 'object' } } } } } }]} mark={<span>PX</span>} onRetry={noop} onCopy={noop} onEdit={noop}/>)
  assert.match(nested, /data-mcp-app="ui:\/\/alpha\/chart.html"/)
  assert.match(nested, /data-mcp-app="ui:\/\/beta\/map.html"/)
  assert.equal((nested.match(/data-mcp-app=/g) ?? []).length, 2)
  assert.ok(nested.indexOf('ui://alpha/chart.html') < nested.indexOf('ui://beta/map.html'), nested)
  assert.match(nested, /aria-label="labby · codemode\. 1 event"/)
})

test('Phoenix ignores invalid MCP App metadata and retains normal tool fallback', () => {
  let reads = 0
  const html = renderToStaticMarkup(<PhoenixConversation messages={[]} events={[{ method: 'item/completed', params: { item: { id: 'invalid-1', type: 'mcpToolCall', status: 'completed', server: 'labby', tool: 'weather', appContext: { resourceUri: 'https://example.test/app.html' }, result: { content: [{ type: 'text', text: 'Ordinary result' }] } } } }]} mark={<span>PX</span>} onRetry={noop} onCopy={noop} onEdit={noop} readMcpAppResource={async () => { reads += 1; return { contents: [] } }}/>)
  assert.doesNotMatch(html, /data-mcp-app=/)
  assert.match(html, /aria-label="labby · weather\. 1 event"/)
  assert.equal(reads, 0)
})

test('Phoenix preserves tool fallback when an MCP App resource read fails', async () => {
  installTestDom()
  const event = { method: 'item/completed', params: { item: { id: 'failed-1', type: 'mcpToolCall', status: 'completed', server: 'labby', tool: 'weather', appContext: { resourceUri: 'ui://weather/app.html' }, result: { content: [{ type: 'text', text: 'Cloudy' }] } } } }
  const { container, unmount } = await renderClient(<PhoenixConversation messages={[]} events={[event]} mark={<span>PX</span>} onRetry={noop} onCopy={noop} onEdit={noop} readMcpAppResource={async () => { throw new Error('sensitive upstream detail') }}/>)
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
  assert.match(container.textContent ?? '', /Failed to load MCP App resource/)
  assert.match(container.textContent ?? '', /Cloudy/, 'retained tool result remains visible as fallback')
  assert.ok(container.querySelector('[aria-label="labby · weather. 1 event"]'), 'normal tool activity remains visible')
  assert.doesNotMatch(container.textContent ?? '', /sensitive upstream detail/)
  await unmount()
})

test('Phoenix deduplicates replayed completed app events by stable identity', async () => {
  installTestDom()
  let reads = 0
  const event = { method: 'item/completed', params: { item: { id: 'duplicate-1', type: 'mcpToolCall', status: 'completed', server: 'labby', tool: 'weather', appContext: { resourceUri: 'ui://weather/app.html' } } } }
  const { container, unmount } = await renderClient(<PhoenixConversation messages={[]} events={[event, event]} mark={<span>PX</span>} onRetry={noop} onCopy={noop} onEdit={noop} readMcpAppResource={async ({ uri }) => { reads += 1; return { contents: [{ uri, mimeType: 'text/html', text: '<main>Weather</main>' }] } }}/>)
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
  assert.equal(container.querySelectorAll('[data-mcp-app]').length, 1)
  assert.equal(reads, 1)
  await unmount()
})

test('Phoenix reconstructs an MCP App from retained events on a fresh render', () => {
  const retainedEvent = { method: 'item/completed', params: { item: { id: 'retained-1', type: 'mcpToolCall', status: 'completed', server: 'labby', tool: 'weather', appContext: { resourceUri: 'ui://weather/app.html' } } } }
  const renderReload = () => renderToStaticMarkup(<PhoenixConversation messages={[]} events={[retainedEvent]} mark={<span>PX</span>} onRetry={noop} onCopy={noop} onEdit={noop}/>)
  assert.match(renderReload(), /data-mcp-app="ui:\/\/weather\/app.html"/)
  assert.match(renderReload(), /data-mcp-app="ui:\/\/weather\/app.html"/)
})

test('Phoenix retains one loaded app across equivalent retained-event rerenders', async () => {
  installTestDom()
  let reads = 0
  const readMcpAppResource = async ({ uri }: { uri: string }) => {
    reads += 1
    assert.equal(uri, 'ui://weather/app.html')
    return { contents: [{ uri, mimeType: 'text/html;profile=mcp-app', text: '<main>Weather</main>' }] }
  }
  const events = () => [{
    method: 'item/completed',
    received_at_ms: 20,
    sequence: 1,
    params: { item: { id: 'direct-1', type: 'mcpToolCall', status: 'completed', server: 'labby', tool: 'weather', appContext: { resourceUri: 'ui://weather/app.html' }, result: { content: [{ type: 'text', text: 'Cloudy' }] } } },
  }]
  const view = (retainedEvents: ReturnType<typeof events>) => <PhoenixConversation messages={[]} events={retainedEvents} mark={<span>PX</span>} onRetry={noop} onCopy={noop} onEdit={noop} readMcpAppResource={readMcpAppResource}/>
  const { container, rerender, unmount } = await renderClient(view(events()))
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
  assert.equal(reads, 1)
  assert.equal(container.querySelectorAll('[data-mcp-app="ui://weather/app.html"]').length, 1)
  assert.match(container.querySelector('iframe')?.getAttribute('srcdoc') ?? '', /Weather/)

  await rerender(view(events()))
  await act(async () => { await Promise.resolve() })
  assert.equal(reads, 1, 'equivalent retained events must preserve the mounted app')
  assert.equal(container.querySelectorAll('[data-mcp-app="ui://weather/app.html"]').length, 1)
  await unmount()
})
