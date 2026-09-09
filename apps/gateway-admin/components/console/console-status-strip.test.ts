import test from 'node:test'
import assert from 'node:assert/strict'

import { deriveConsoleStatus } from './console-status-strip'

test('console status derives connected upstream, session, and exposed-tool counts', () => {
  const snapshot = deriveConsoleStatus(
    [
      { name: 'alpha', enabled: true, connected: true, exposed_tool_count: 7 },
      { name: 'beta', enabled: true, connected: false, exposed_tool_count: 0 },
      { name: 'gamma', enabled: true, connected: true, exposed_tool_count: 25 },
      { name: 'disabled', enabled: false, connected: true, exposed_tool_count: 999 },
    ],
    [
      { transport: 'http', connected_at: '2026-09-09T00:00:00Z' },
      { transport: 'stdio', connected_at: '2026-09-09T00:01:00Z' },
    ],
  )

  assert.deepEqual(snapshot, {
    connected: 2,
    total: 3,
    sessions: 2,
    tools: 32,
  })
})

test('console status omits the session count when client inventory is unavailable', () => {
  const snapshot = deriveConsoleStatus([
    { name: 'alpha', enabled: true, connected: true, exposed_tool_count: 4 },
  ])
  assert.deepEqual(snapshot, {
    connected: 1,
    total: 1,
    sessions: undefined,
    tools: 4,
  })
})
