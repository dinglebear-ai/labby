/** Local preview data only. Every identity is synthetic, never production telemetry. */
export function createOverviewFixtureMetrics({ since_unix, until_unix, bucket_count = 24 }) {
  if (!Number.isSafeInteger(since_unix) || !Number.isSafeInteger(until_unix) ||
      since_unix < 0 || until_unix <= since_unix || until_unix > 253402300799) {
    throw new RangeError('Fixture metrics require bounded increasing Unix seconds')
  }
  if (!Number.isSafeInteger(bucket_count)) throw new RangeError('Fixture bucket count must be an integer')
  const count = Math.min(24, Math.max(1, bucket_count), until_unix - since_unix)
  const tools = [
    ['Fixture Corpus', 'search'], ['Fixture Labby', 'artifacts.list'],
    ['Fixture Cortex', 'logs.search'], ['Fixture Corpus', 'documents.get'],
    ['Fixture Labby', 'gateways.list'], ['Fixture Cortex', 'sessions.get'],
  ].map(([upstream, tool]) => ({ upstream, tool, capability: 'tools', operation: 'tool.call', subject_scoped: false, calls: 0, failed: 0 }))
  const actors = ['Fixture research agent', 'Fixture operator agent', 'Fixture audit agent'].map(actor => ({ actor, calls: 0 }))
  const timeseries = Array.from({ length: count }, (_, index) => {
    let calls = 0
    let failed = 0
    for (let toolIndex = 0; toolIndex < tools.length; toolIndex++) {
      const amount = 8 + ((index * 7 + toolIndex * 11) % 29)
      const errors = (index + toolIndex) % 5 === 0 ? 2 : 0
      tools[toolIndex].calls += amount
      tools[toolIndex].failed += errors
      actors[(index + toolIndex) % actors.length].calls += amount
      calls += amount
      failed += errors
    }
    return { ts_unix: since_unix + Math.floor(index * (until_unix - since_unix) / count), calls, failed }
  })
  const total = tools.reduce((sum, tool) => sum + tool.calls, 0)
  const failed = tools.reduce((sum, tool) => sum + tool.failed, 0)
  const upstreams = [...new Set(tools.map(tool => tool.upstream))].map(upstream => ({
    upstream,
    calls: tools.filter(tool => tool.upstream === upstream).reduce((sum, tool) => sum + tool.calls, 0),
    failed: tools.filter(tool => tool.upstream === upstream).reduce((sum, tool) => sum + tool.failed, 0),
  }))
  const hourly = Array.from({ length: 24 }, (_, hour) => ({ hour, calls: 0 }))
  for (const bucket of timeseries) hourly[Math.floor(bucket.ts_unix / 3600) % 24].calls += bucket.calls
  return {
    window_total_calls: total, total_calls: total, error_calls: failed,
    avg_elapsed_ms: 180, p50_elapsed_ms: 140, p95_elapsed_ms: 420, p99_elapsed_ms: 680,
    distinct_tools: tools.length, distinct_actors: actors.length,
    peak_per_min: Math.ceil(Math.max(...timeseries.map(bucket => bucket.calls)) * 60 / ((until_unix - since_unix) / count)),
    top_tools: [...tools].sort((a, b) => b.calls - a.calls),
    least_tools: [...tools].sort((a, b) => a.calls - b.calls),
    top_actors: [...actors].sort((a, b) => b.calls - a.calls),
    slowest_tools: tools.map((tool, index) => ({ upstream: tool.upstream, tool: tool.tool, avg_elapsed_ms: 305 - index * 50 })),
    errors: [{ kind: 'timeout', calls: Math.floor(failed / 2) }, { kind: 'upstream_error', calls: failed - Math.floor(failed / 2) }],
    upstreams, hourly, timeseries,
    facets: {
      tools: tools.map(({ upstream, tool }) => ({ upstream, tool })),
      capabilities: ['tools'], operations: ['tool.call'], subject_scopes: [false],
      actors: actors.map(actor => actor.actor), upstreams: upstreams.map(upstream => upstream.upstream),
      outcomes: ['success', 'error'],
    },
  }
}
