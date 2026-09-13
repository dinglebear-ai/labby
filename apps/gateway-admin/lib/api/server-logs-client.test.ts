import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests } from '../auth/session-store.ts'

process.env.NEXT_PUBLIC_MOCK_DATA = 'false'

const result = (filters?: { levels?: string[] }) => ({
  kind: 'server_logs',
  filters,
  entries: [],
  available_sources: ['gateway'],
  available_sources_complete: true,
  matched: 0,
  scanned_lines: 0,
  malformed_lines: 0,
  scanned_bytes: 0,
  max_scan_bytes: 1024,
  truncated: false,
})

test('plural log levels are sent exactly and require the normalized response echo', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'admin' },
    expiresAt: Date.now() + 60_000,
    csrfToken: 'csrf',
    isAdmin: true,
  })
  const requests: Array<{ params: Record<string, unknown> }> = []
  const previousFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body))
    requests.push(request)
    return new Response(JSON.stringify(result({ levels: ['ERROR', 'WARN'] })), { status: 200 })
  }

  try {
    const { queryServerLogs } = await import('./server-logs-client.ts')
    await queryServerLogs({ levels: ['warn', 'ERROR', 'warn'] })

    assert.deepEqual(requests[0]?.params.levels, ['warn', 'ERROR', 'warn'])
  } finally {
    globalThis.fetch = previousFetch
  }
})

test('plural log levels fail closed when an older server omits the applied-filter echo', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'admin' },
    expiresAt: Date.now() + 60_000,
    csrfToken: 'csrf',
    isAdmin: true,
  })
  const previousFetch = globalThis.fetch
  globalThis.fetch = async () => new Response(JSON.stringify(result()), { status: 200 })

  try {
    const { queryServerLogs, ServerLogsApiError } = await import('./server-logs-client.ts')
    await assert.rejects(
      queryServerLogs({ levels: ['ERROR', 'WARN'] }),
      (error: unknown) => error instanceof ServerLogsApiError
        && error.status === 409
        && error.code === 'server_logs_levels_unsupported'
        && error.param === 'levels',
    )
  } finally {
    globalThis.fetch = previousFetch
  }
})
