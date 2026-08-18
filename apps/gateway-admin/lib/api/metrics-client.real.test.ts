import test from 'node:test'
import assert from 'node:assert/strict'

process.env.NEXT_PUBLIC_MOCK_DATA = 'false'

test('fetchDashboardMetrics uses bounded retained-log sampling', async () => {
  const actions: string[] = []
  let serverLogParams: Record<string, unknown> | undefined
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params?: Record<string, unknown> }
    actions.push(body.action)
    if (body.action === 'server_logs.query') serverLogParams = body.params
    const payload = body.action === 'gateway.usage.metrics'
      ? {
          total_calls: 1,
          error_calls: 0,
          avg_elapsed_ms: 12,
          top_tools: [{ upstream: 'github', tool: 'search', calls: 1 }],
          top_actors: [{ actor: 'codex', calls: 1 }],
        }
      : body.action === 'gateway.usage.calls' ? {
          calls: [{ ts_unix: 1_800_000_000, upstream: 'github', tool: 'search', actor: 'codex', outcome: 'ok', elapsed_ms: 12 }],
          total_matching: 1,
          next_cursor: null,
        } : {
          kind: 'server_logs',
          entries: [{
            timestamp: new Date(Date.now() - 1000).toISOString(), level: 'INFO', target: 'labby', message: 'dispatch ok',
            service: 'gateway', action: 'gateway.list', kind: null, file: 'labby.jsonl',
            fields: { surface: 'api', input_tokens: 10, output_tokens: 20 },
          }],
          matched: 1, scanned_lines: 1, malformed_lines: 0, scanned_bytes: 100, max_scan_bytes: 1000, truncated: false,
        }
    return new Response(JSON.stringify(payload), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }

  try {
    const { fetchDashboardMetrics } = await import('./metrics-client.ts')
    const result = await fetchDashboardMetrics('24h')
    assert.deepEqual(actions.sort(), ['gateway.usage.calls', 'gateway.usage.metrics', 'server_logs.query'])
    assert.deepEqual(serverLogParams, {
      limit: 500,
      max_scan_bytes: 2 * 1024 * 1024,
      stop_after_limit: true,
    })
    assert.equal(result.tool_calls.total, 1)
    assert.deepEqual(result.surfaces, [{ surface: 'api', calls: 1 }])
    assert.equal(result.tokens.total, 30)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('fetchToolDetail derives drill-down data from persisted gateway call rows', async () => {
  const actions: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    actions.push(body.action)
    return new Response(JSON.stringify({
      calls: [
        { ts_unix: 1_800_000_000, upstream: 'github', tool: 'search', actor: 'codex', outcome: 'ok', elapsed_ms: 12 },
        { ts_unix: 1_800_000_001, upstream: 'other', tool: 'search', actor: 'codex', outcome: 'ok', elapsed_ms: 5 },
      ],
      total_matching: 2,
      next_cursor: null,
    }), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchToolDetail } = await import('./metrics-client.ts')
    const detail = await fetchToolDetail('github::search', '24h')
    assert.deepEqual(actions, ['gateway.usage.calls'])
    assert.equal(detail.calls, 1)
    assert.equal(detail.avg_elapsed_ms, 12)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('fetchToolCalls serves the usage explorer from persisted gateway rows', async () => {
  const actions: string[] = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    actions.push(body.action)
    const payload = body.action === 'gateway.usage.metrics'
      ? { total_calls: 2, error_calls: 1, avg_elapsed_ms: 5, top_tools: [], top_actors: [] }
      : {
          calls: [
            { ts_unix: 1_800_000_000, upstream: 'github', tool: 'search', actor: 'codex', outcome: 'ok', elapsed_ms: 5 },
            { ts_unix: 1_800_000_001, upstream: 'github', tool: 'create', actor: 'codex', outcome: 'timeout', elapsed_ms: 5 },
          ],
          total_matching: 2,
          next_cursor: null,
        }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchToolCalls } = await import('./metrics-client.ts')
    const page = await fetchToolCalls({ window: '24h', outcome: 'failed', limit: 50 })
    assert.deepEqual(actions.sort(), ['gateway.usage.calls', 'gateway.usage.metrics'])
    assert.equal(page.total, 2)
    assert.equal(page.filtered, 1)
    assert.equal(page.calls[0].error_kind, 'timeout')
    assert.deepEqual(page.collected, {
      actors: true,
      ips: false,
      surfaces: false,
      tokens: false,
    })
  } finally {
    globalThis.fetch = originalFetch
  }
})
