import assert from 'node:assert/strict'
import test from 'node:test'
import { mcpAppsForPhoenixEvent } from './event-apps.ts'

const completed = (item: Record<string, unknown>) => ({ method: 'item/completed', params: { item: { type: 'mcpToolCall', status: 'completed', ...item } } })

test('detects modern and legacy direct MCP App links with stable keys', () => {
  const modern = mcpAppsForPhoenixEvent(completed({ id: 'call-1', tool: 'weather', arguments: { city: 'Boston' }, appContext: { resourceUri: 'ui://weather/app.html', appName: 'Weather' }, result: { content: [{ type: 'text', text: 'Cloudy' }] } }))
  assert.deepEqual(modern, [{ key: 'call-1:ui://weather/app.html', resourceUri: 'ui://weather/app.html', itemId: 'call-1', appName: 'Weather', toolInput: { city: 'Boston' }, toolResult: { content: [{ type: 'text', text: 'Cloudy' }] } }])
  assert.equal(mcpAppsForPhoenixEvent(completed({ id: 'call-2', mcpAppResourceUri: 'ui://legacy/app.html' }))[0]?.resourceUri, 'ui://legacy/app.html')
  assert.equal(mcpAppsForPhoenixEvent(completed({ id: 'call-3', result: { _meta: { ui: { resourceUri: 'ui://modern/app.html' } } } }))[0]?.resourceUri, 'ui://modern/app.html')
  assert.equal(mcpAppsForPhoenixEvent(completed({ id: 'call-4', result: { _meta: { 'ui/resourceUri': 'ui://compat/app.html' } } }))[0]?.resourceUri, 'ui://compat/app.html')
})

test('rejects incomplete, malformed, and non-app events without throwing', () => {
  for (const event of [
    { method: 'item/started', params: { item: { id: 'x', type: 'mcpToolCall', appContext: { resourceUri: 'ui://x/app.html' } } } },
    completed({ id: 'x', appContext: { resourceUri: 'https://example.test/app.html' } }),
    completed({ id: 'x', appContext: { resourceUri: 'ui://missing-path' } }),
    completed({ id: 'x', appContext: { resourceUri: {} } }),
    completed({ id: 'x', result: { content: [{ type: 'text', text: 'ordinary' }] } }),
  ]) assert.deepEqual(mcpAppsForPhoenixEvent(event), [])
})

test('detects nested Code Mode MCP Apps and ignores ordinary calls', () => {
  const apps = mcpAppsForPhoenixEvent(completed({
    id: 'code-1', tool: 'codemode', result: { structuredContent: { kind: 'code_mode_execute_trace', call_count: 3, calls: [
      { id: 'alpha::one', namespace: 'alpha', tool: 'one', ok: true, elapsed_ms: 1, ui: { resourceUri: 'ui://alpha/one.html' } },
      { id: 'beta::plain', namespace: 'beta', tool: 'plain', ok: true, elapsed_ms: 2 },
      { id: 'alpha::one', namespace: 'alpha', tool: 'one', ok: true, elapsed_ms: 3, params: { city: 'Paris' }, ui: { resourceUri: 'ui://alpha/one.html' } },
    ], result_shape: { type: 'object' } } },
  }))
  assert.deepEqual(apps.map(({ key, resourceUri, callId }) => ({ key, resourceUri, callId })), [
    { key: 'code-1:0:alpha::one:ui://alpha/one.html', resourceUri: 'ui://alpha/one.html', callId: 'alpha::one' },
    { key: 'code-1:2:alpha::one:ui://alpha/one.html', resourceUri: 'ui://alpha/one.html', callId: 'alpha::one' },
  ])
  assert.deepEqual(apps.map((app) => app.toolInput), [{}, { city: 'Paris' }])
  assert.ok(apps.every((app) => app.toolResult === undefined), 'outer Code Mode trace is not a nested tool result')
})
