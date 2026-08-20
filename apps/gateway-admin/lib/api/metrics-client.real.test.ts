import test from 'node:test'
import assert from 'node:assert/strict'

process.env.NEXT_PUBLIC_MOCK_DATA = 'false'

function metrics(overrides: Record<string, unknown> = {}) {
  return {
    window_total_calls: 2,
    total_calls: 2,
    error_calls: 1,
    avg_elapsed_ms: 12,
    p50_elapsed_ms: 8,
    p95_elapsed_ms: 20,
    p99_elapsed_ms: 20,
    distinct_tools: 2,
    distinct_actors: 1,
    peak_per_min: 2,
    top_tools: [{ upstream: 'github', tool: 'search', calls: 2, failed: 1 }],
    least_tools: [{ upstream: 'github', tool: 'search', calls: 2, failed: 1 }],
    top_actors: [{ actor: 'codex', calls: 2 }],
    slowest_tools: [{ upstream: 'github', tool: 'search', avg_elapsed_ms: 12 }],
    errors: [{ kind: 'timeout', calls: 1 }],
    upstreams: [{ upstream: 'github', calls: 2, failed: 1 }],
    hourly: Array.from({ length: 24 }, (_, hour) => ({ hour, calls: hour === 12 ? 2 : 0 })),
    timeseries: [{ ts_unix: 1_800_000_000, calls: 2, failed: 1 }],
    facets: {
      tools: [{ upstream: 'github', tool: 'search' }, { upstream: 'github', tool: 'create' }],
      actors: ['codex'],
      upstreams: ['github'],
      outcomes: ['ok', 'timeout'],
    },
    ...overrides,
  }
}

test('fetchDashboardMetrics uses complete-window aggregate analytics without raw call sampling', async () => {
  const actions: string[] = []
  let metricsParams: Record<string, unknown> | undefined
  let serverLogParams: Record<string, unknown> | undefined
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params?: Record<string, unknown> }
    actions.push(body.action)
    if (body.action === 'gateway.usage.metrics') metricsParams = body.params
    if (body.action === 'server_logs.query') serverLogParams = body.params
    const payload = body.action === 'gateway.usage.metrics'
      ? metrics({ window_total_calls: 48_649, total_calls: 48_649, error_calls: 12, timeseries: Array.from({ length: 24 }, (_, index) => ({ ts_unix: 1_800_000_000 + index * 3600, calls: index === 0 ? 4_000 : index === 23 ? 1_000 : 0, failed: 0 })) })
      : {
          kind: 'server_logs',
          entries: [{ timestamp: new Date(Date.now() - 1000).toISOString(), level: 'INFO', target: 'labby', message: 'dispatch ok', service: 'gateway', action: 'gateway.list', kind: null, file: 'labby.jsonl', fields: { surface: 'api', input_tokens: 10, output_tokens: 20 } }],
          matched: 1, scanned_lines: 1, malformed_lines: 0, scanned_bytes: 100, max_scan_bytes: 1000, truncated: false,
        }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchDashboardMetrics } = await import('./metrics-client.ts')
    const result = await fetchDashboardMetrics('24h')
    assert.deepEqual(actions.sort(), ['gateway.usage.metrics', 'server_logs.query'])
    assert.equal(metricsParams?.bucket_count, 24)
    assert.equal(typeof metricsParams?.timezone, 'string')
    assert.equal(metricsParams?.include_facets, false)
    assert.deepEqual(serverLogParams, { limit: 500, max_scan_bytes: 2 * 1024 * 1024, stop_after_limit: true })
    assert.equal(result.tool_calls.total, 48_649)
    assert.equal(result.timeseries.length, 24)
    assert.equal(result.timeseries[0].calls, 4_000)
    assert.equal(result.timeseries[23].calls, 1_000)
    assert.deepEqual(result.surfaces, [{ surface: 'api', calls: 1 }])
    assert.equal(result.tokens.total, 30)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('fetchToolDetail uses exact filtered aggregate plus a bounded recent-call page', async () => {
  const requests: Array<{ action: string; params?: Record<string, unknown> }> = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params?: Record<string, unknown> }
    requests.push(body)
    const payload = body.action === 'gateway.usage.metrics'
      ? metrics({ window_total_calls: 20, total_calls: 12, error_calls: 2, avg_elapsed_ms: 17, top_actors: [{ actor: 'codex', calls: 12 }] })
      : { calls: [{ ts_unix: 1_800_000_000, upstream: 'github', tool: 'search', actor: 'codex', outcome: 'ok', elapsed_ms: 12 }], total_matching: 12, next_cursor: 'cursor' }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchToolDetail } = await import('./metrics-client.ts')
    const detail = await fetchToolDetail('github::search', '24h')
    assert.deepEqual(requests.map((request) => request.action).sort(), ['gateway.usage.calls', 'gateway.usage.metrics'])
    assert.equal(requests.find((request) => request.action === 'gateway.usage.metrics')?.params?.tool, 'github::search')
    assert.equal(requests.find((request) => request.action === 'gateway.usage.calls')?.params?.limit, 25)
    assert.equal(detail.calls, 12)
    assert.equal(detail.failed, 2)
    assert.equal(detail.avg_elapsed_ms, 17)
    assert.equal(detail.recent.length, 1)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('fetchToolCalls sends exact filters and cursor to the backend', async () => {
  const requests: Array<{ action: string; params?: Record<string, unknown> }> = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params?: Record<string, unknown> }
    requests.push(body)
    const payload = body.action === 'gateway.usage.metrics'
      ? metrics({ window_total_calls: 5_000, total_calls: 73, error_calls: 73 })
      : { calls: [{ ts_unix: 1_800_000_001, upstream: 'github', tool: 'create', actor: 'codex', outcome: 'timeout', elapsed_ms: 5 }], total_matching: 73, next_cursor: 'next-cursor' }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchToolCalls } = await import('./metrics-client.ts')
    const page = await fetchToolCalls({ window: '24h', upstream: 'github', tool: 'github::create', agent: 'codex', outcome: 'failed', error_kind: 'timeout', search: 'create', cursor: 'prev-cursor', limit: 50 })
    const aggregate = requests.find((request) => request.action === 'gateway.usage.metrics')?.params
    const calls = requests.find((request) => request.action === 'gateway.usage.calls')?.params
    assert.equal(aggregate?.upstream, 'github')
    assert.equal(aggregate?.tool, 'github::create')
    assert.equal(aggregate?.actor, 'codex')
    assert.equal(aggregate?.outcome, 'timeout')
    assert.equal(aggregate?.search, 'create')
    assert.equal(aggregate?.include_facets, true)
    assert.equal(calls?.cursor, 'prev-cursor')
    assert.equal(calls?.limit, 50)
    assert.equal(page.total, 5_000)
    assert.equal(page.filtered, 73)
    assert.equal(page.next_cursor, 'next-cursor')
    assert.equal(page.calls[0].error_kind, 'timeout')
    assert.equal(page.analytics.failed, 73)
  } finally {
    globalThis.fetch = originalFetch
  }
})
