import test from 'node:test'
import assert from 'node:assert/strict'

import type { AuthoritySnapshot } from '../auth/authority.ts'
import { __resetAuthorityContextForTests } from '../auth/authority-context.ts'
import {
  __setBrowserSessionStateForTests,
  selectSessionWorkspace,
} from '../auth/session-store.ts'

process.env.NEXT_PUBLIC_MOCK_DATA = 'false'

const authority: AuthoritySnapshot = {
  schemaVersion: 1,
  compatibilityGeneration: 1,
  principalId: 'operator',
  organizationId: 'org-1',
  activeOwner: { kind: 'team', id: 'team-a' },
  activeTeamId: 'team-a',
  teams: [
    { id: 'team-a', role: 'owner', membershipEpoch: 1, policyEpoch: 1 },
    { id: 'team-b', role: 'owner', membershipEpoch: 1, policyEpoch: 1 },
  ],
  projects: [],
  capabilities: ['scope.read'],
  generation: 1,
}

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
      ? metrics({ window_total_calls: 48_649, total_calls: 48_649, error_calls: 12, timeseries: Array.from({ length: 24 }, (_, index) => ({ ts_unix: 1_800_000_000 + index * 3600, calls: index === 0 ? 4_000 : index === 23 ? 1_000 : 0, failed: index === 0 ? 2 : 0, ...(index === 0 ? { outcomes: [{ kind: 'timeout', calls: 2 }] } : {}) })) })
      : {
          kind: 'server_logs',
          // Keep the fixture comfortably inside the 24-hour window. A one-second
          // offset can cross the fetcher's captured `now` under a heavily loaded
          // CI runner, causing a valid entry to be discarded as future-dated.
          entries: [{ timestamp: new Date(Date.now() - 60_000).toISOString(), level: 'INFO', target: 'labby', message: 'dispatch ok', service: 'gateway', action: 'gateway.list', kind: null, file: 'labby.jsonl', fields: { surface: 'api', input_tokens: 10, output_tokens: 20 } }],
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
    assert.deepEqual(result.timeseries[0].outcomes, [{ kind: 'timeout', count: 2 }])
    assert.equal(result.timeseries[23].calls, 1_000)
    assert.deepEqual(result.surfaces, [{ surface: 'api', calls: 1 }])
    assert.equal(result.tokens.total, 30)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('fetchDashboardMetrics surfaces retained-observability failure without discarding durable usage', async () => {
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    if (body.action === 'gateway.usage.metrics') {
      return new Response(JSON.stringify(metrics({ total_calls: 37, window_total_calls: 37 })), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }
    return new Response(JSON.stringify({ message: 'server log store unavailable' }), {
      status: 503,
      headers: { 'content-type': 'application/json' },
    })
  }

  try {
    const { fetchDashboardMetrics } = await import('./metrics-client.ts')
    const result = await fetchDashboardMetrics('24h')
    assert.equal(result.tool_calls.total, 37)
    assert.equal(result.collected.tokens, false)
    assert.equal(result.collected.surfaces, false)
    assert.equal(result.collected.fan_out, false)
    assert.equal(result.warnings?.length, 1)
    assert.match(result.warnings?.[0] ?? '', /Retained observability is unavailable/)
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

test('fetchAgentDetail sends exact client filters and accepts only echoed filters', async () => {
  const requests: Array<{ action: string; params?: Record<string, unknown> }> = []
  const exact = { client_name: 'Codex CLI', client_version: '2.0' }
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params?: Record<string, unknown> }
    requests.push(body)
    const payload = body.action === 'gateway.usage.metrics'
      ? metrics({ attribution_filters: exact, window_total_calls: 8, total_calls: 1, error_calls: 0 })
      : { attribution_filters: exact, calls: [], total_matching: 1 }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchAgentDetail } = await import('./metrics-client.ts')
    const detail = await fetchAgentDetail({
      type: 'agent',
      filter: { actor: 'unattributed', ...exact },
      label: 'Codex CLI',
      kind: 'client',
    }, '24h')
    for (const request of requests) {
      assert.equal(request.params?.actor, 'unattributed')
      assert.equal(request.params?.client_name, 'Codex CLI')
      assert.equal(request.params?.client_version, '2.0')
    }
    assert.equal(detail.calls, 1)
    assert.equal(detail.label, 'Codex CLI')
    assert.equal(detail.kind, 'client')
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('fetchAgentDetail rejects an older gateway that ignores precise filters', async () => {
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    const payload = body.action === 'gateway.usage.metrics'
      ? metrics({ window_total_calls: 8, total_calls: 8 })
      : { calls: [], total_matching: 8 }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchAgentDetail, MetricsApiError } = await import('./metrics-client.ts')
    await assert.rejects(
      fetchAgentDetail({
        type: 'agent',
        filter: { actor: 'unattributed', client_name: 'Codex CLI', client_version: '2.0' },
        label: 'Codex CLI',
        kind: 'client',
      }, '24h'),
      (error: unknown) => error instanceof MetricsApiError
        && error.code === 'usage_attribution_filters_unsupported',
    )
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('usage requests are canceled and late responses rejected after an authority switch', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'operator' },
    expiresAt: Date.now() + 60_000,
    csrfToken: 'csrf',
    authority,
  })
  const pendingResponses: Array<{
    action: string
    signal: AbortSignal | null | undefined
    resolve: (response: Response) => void
  }> = []
  const originalFetch = globalThis.fetch
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body)) as { action: string }
    return new Promise<Response>((resolve) => {
      pendingResponses.push({ action: request.action, signal: init?.signal, resolve })
    })
  }

  try {
    const { fetchAgentDetail } = await import('./metrics-client.ts')
    const request = fetchAgentDetail({
      type: 'agent',
      filter: { actor: 'unattributed', client_name: 'Codex CLI', client_version: '2.0' },
      label: 'Codex CLI',
      kind: 'client',
    }, '24h')
    await Promise.resolve()
    assert.equal(pendingResponses.length, 2)

    selectSessionWorkspace({ teamId: 'team-b' })
    assert.deepEqual(
      pendingResponses.map((entry) => entry.signal?.aborted),
      [true, true],
      'the authority generation must cancel both aggregate and call-list requests',
    )

    const exact = { client_name: 'Codex CLI', client_version: '2.0' }
    for (const pending of pendingResponses) {
      const payload = pending.action === 'gateway.usage.metrics'
        ? metrics({ attribution_filters: exact })
        : { attribution_filters: exact, calls: [], total_matching: 0 }
      pending.resolve(Response.json(payload))
    }
    await assert.rejects(
      request,
      (error: unknown) => error instanceof DOMException && error.name === 'AbortError',
      'a transport that ignores cancellation must still have its late response rejected',
    )
  } finally {
    globalThis.fetch = originalFetch
    __resetAuthorityContextForTests()
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
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
      : { calls: [{ ts_unix: 1_800_000_001, upstream: 'github', tool: 'create', capability: 'resources', operation: 'resource.read', subject_scoped: true, actor: 'codex', outcome: 'timeout', elapsed_ms: 5 }], total_matching: 73, next_cursor: 'next-cursor' }
    return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } })
  }

  try {
    const { fetchToolCalls } = await import('./metrics-client.ts')
    const page = await fetchToolCalls({ window: '24h', upstream: 'github', tool: 'github::create', capability: 'resources', operation: 'resource.read', subject_scoped: true, agent: 'codex', outcome: 'failed', error_kind: 'timeout', search: 'create', cursor: 'prev-cursor', limit: 50 })
    const aggregate = requests.find((request) => request.action === 'gateway.usage.metrics')?.params
    const calls = requests.find((request) => request.action === 'gateway.usage.calls')?.params
    assert.equal(aggregate?.upstream, 'github')
    assert.equal(aggregate?.tool, 'github::create')
    assert.equal(aggregate?.capability, 'resources')
    assert.equal(aggregate?.operation, 'resource.read')
    assert.equal(aggregate?.subject_scoped, true)
    assert.equal(aggregate?.actor, 'codex')
    assert.equal(aggregate?.outcome, 'timeout')
    assert.equal(aggregate?.search, 'create')
    assert.equal(aggregate?.include_facets, true)
    assert.equal(calls?.cursor, 'prev-cursor')
    assert.equal(calls?.capability, 'resources')
    assert.equal(calls?.operation, 'resource.read')
    assert.equal(calls?.subject_scoped, true)
    assert.equal(calls?.limit, 50)
    assert.equal(page.total, 5_000)
    assert.equal(page.filtered, 73)
    assert.equal(page.next_cursor, 'next-cursor')
    assert.equal(page.calls[0].error_kind, 'timeout')
    assert.equal(page.calls[0].capability, 'resources')
    assert.equal(page.calls[0].action, 'resource.read')
    assert.equal(page.calls[0].subject_scoped, true)
    assert.equal(page.analytics.failed, 73)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('upstream summary preserves fixed window, full bucket counts and exact upstream authority', async () => {
  const originalFetch = globalThis.fetch
  let request: { action: string; params: Record<string, unknown> } | undefined
  globalThis.fetch = async (_input, init) => {
    request = JSON.parse(String(init?.body))
    return new Response(JSON.stringify(metrics()), { status: 200, headers: { 'content-type': 'application/json' } })
  }
  try {
    const { fetchGatewayUsageMetrics } = await import('./metrics-client.ts')
    const result = await fetchGatewayUsageMetrics('24h', 'github', 1_800_086_400_000)
    assert.equal(request?.action, 'gateway.usage.metrics')
    assert.equal(request?.params.upstream, 'github')
    assert.equal(request?.params.since_unix, 1_800_000_000)
    assert.equal(request?.params.until_unix, 1_800_086_400)
    assert.equal(request?.params.bucket_count, 24)
    assert.deepEqual(result.timeseries, [{ ts_unix: 1_800_000_000, calls: 2, failed: 1 }])
  } finally {
    globalThis.fetch = originalFetch
  }
})
