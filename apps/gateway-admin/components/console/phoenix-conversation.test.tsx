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
