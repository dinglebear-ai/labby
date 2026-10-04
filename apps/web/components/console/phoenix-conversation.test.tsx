import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { PhoenixConversation } from './phoenix-conversation.tsx'

const noop = () => undefined

test('a new turn does not claim a previous turn\'s unfinished stream', () => {
  const html = renderToStaticMarkup(<PhoenixConversation
    messages={[
      { role: 'user', text: 'First question', created_at_ms: 100, turn_id: 'first' },
      { role: 'user', text: 'New question', created_at_ms: 300, turn_id: 'second' },
      { role: 'assistant', text: 'Second answer', created_at_ms: 500, turn_id: 'second' },
    ]}
    events={[{ method: 'item/agentMessage/delta', received_at_ms: 200, params: { turnId: 'first', delta: 'Unfinished first answer' } }]}
    mark={<span>PX</span>} onCopy={noop} onRetry={noop} onEdit={noop}
  />)
  assert.match(html, /Unfinished first answer/)
  assert.match(html, /Second answer/)
})

for (const suffix of ['second', 'partial']) {
  test(`same-turn steering reconciles preceding deltas: ${suffix}`, () => {
    const html = renderToStaticMarkup(<PhoenixConversation
      messages={[
        { role: 'user', text: 'Question', created_at_ms: 100, turn_id: 'first' },
        { role: 'user', text: 'Steer', created_at_ms: 300, turn_id: 'first' },
        { role: 'assistant', text: 'First second', created_at_ms: 500, turn_id: 'first' },
      ]}
      events={[
        { method: 'item/agentMessage/delta', received_at_ms: 200, params: { turnId: 'first', delta: 'First ' } },
        { method: 'item/agentMessage/delta', received_at_ms: 400, params: { turnId: 'first', delta: suffix } },
      ]}
      mark={<span>PX</span>} onCopy={noop} onRetry={noop} onEdit={noop}
    />)
    assert.equal((html.match(/First /g) ?? []).length, 1, 'steering must not leave a duplicated prefix')
    assert.equal((html.match(/aria-label="Copy answer"/g) ?? []).length, 1)
    assert.match(html, /Steer/)
    if (suffix === 'partial') {
      assert.match(html, /First second/)
      assert.doesNotMatch(html, /partial/)
    } else {
      assert.ok(html.indexOf('First ') < html.indexOf('Steer'))
      assert.ok(html.indexOf('Steer') < html.indexOf('second'))
    }
  })
}

test('each completed streamed turn keeps its own copy and regenerate controls', async () => {
  const { installTestDom, renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  installTestDom()
  const copied: Array<[string, number]> = []
  const retried: number[] = []
  const view = await renderClient(<PhoenixConversation
    messages={[
      { role: 'user', text: 'First question', created_at_ms: 100 },
      { role: 'assistant', text: 'First answer', created_at_ms: 300 },
      { role: 'user', text: 'Second question', created_at_ms: 400 },
      { role: 'assistant', text: 'Second answer', created_at_ms: 600 },
    ]}
    events={[
      { method: 'item/agentMessage/delta', received_at_ms: 200, params: { turnId: 'first', delta: 'First answer' } },
      { method: 'item/agentMessage/delta', received_at_ms: 500, params: { turnId: 'second', delta: 'Second answer' } },
    ]}
    mark={<span>PX</span>} onCopy={(text, index) => copied.push([text, index])} onRetry={index => retried.push(index)} onEdit={noop}
  />)
  try {
    const copies = [...view.container.querySelectorAll<HTMLButtonElement>('button[aria-label="Copy answer"]')]
    const retries = [...view.container.querySelectorAll<HTMLButtonElement>('button[aria-label="Copy preceding prompt to new conversation"]')]
    assert.equal(copies.length, 2)
    assert.equal(retries.length, 2)
    await act(async () => { copies.forEach(button => button.click()); retries.forEach(button => button.click()) })
    assert.deepEqual(copied, [['First answer', 1], ['Second answer', 3]])
    assert.deepEqual(retried, [1, 3])
  } finally { await view.unmount() }
})

for (const delta of ['First ', 'Earlier answer']) {
  test(`completed assistant text survives an incomplete or revised stream: ${delta}`, () => {
    const html = renderToStaticMarkup(<PhoenixConversation
      messages={[
        { role: 'user', text: 'Question', created_at_ms: 100 },
        { role: 'assistant', text: 'First second', created_at_ms: 300 },
      ]}
      events={[{ method: 'item/agentMessage/delta', received_at_ms: 200, params: { turnId: 'first', delta } }]}
      mark={<span>PX</span>} onCopy={noop} onRetry={noop} onEdit={noop}
    />)
    assert.match(html, /First second/)
    assert.equal((html.match(/aria-label="Copy answer"/g) ?? []).length, 1)
    assert.equal((html.match(/data-phoenix-message="assistant-stream"/g) ?? []).length, 0, 'completed text replaces an incomplete stream')
  })
}

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
