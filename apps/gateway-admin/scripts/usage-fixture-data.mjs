/** Synthetic local-only Usage data; no production telemetry or credentials. */
function validate(params) {
  if (!params || !Number.isSafeInteger(params.since_unix) || !Number.isSafeInteger(params.until_unix) || params.since_unix < 0 || params.until_unix <= params.since_unix || params.until_unix > 253402300799) throw new RangeError('Bounded increasing Unix seconds required')
  for (const key of ['upstream', 'tool', 'capability', 'operation', 'actor', 'outcome', 'search']) {
    if (params[key] !== undefined && (typeof params[key] !== 'string' || params[key].length > 512)) throw new TypeError('Invalid fixture filter')
  }
  if (params.subject_scoped !== undefined && typeof params.subject_scoped !== 'boolean') throw new TypeError('subject_scoped must be boolean')
}
function dataset(params) {
  validate(params)
  const surfaces = ['mcp', 'api', 'cli', 'web']
  const targets = [
    ['Fixture Corpus', 'search', 'tools', 'tool.call'],
    ['Fixture Labby', 'artifacts.list', 'tools', 'tool.call'],
    ['Fixture Cortex', 'logs.search', 'tools', 'tool.call'],
    ['Fixture Corpus', 'documents.get', 'resources', 'resource.read'],
    ['Fixture Labby', 'review', 'prompts', 'prompt.get'],
    ['Fixture Cortex', 'sessions.get', 'tools', 'tool.call'],
  ]
  return Array.from({ length: 240 }, (_, index) => {
    const [upstream, tool, capability, operation] = targets[index % targets.length]
    return { ts_unix: params.since_unix + Math.floor(index * (params.until_unix - params.since_unix) / 240), upstream, tool, capability, operation,
      subject_scoped: Math.floor(index / 6) % 2 === 0, actor: ['Fixture researcher', 'Fixture operator', 'Fixture auditor'][Math.floor(index / 2) % 3],
      surface: surfaces[index % surfaces.length],
      outcome: index % 11 === 0 ? 'timeout' : index % 17 === 0 ? 'upstream_error' : 'ok',
      elapsed_ms: 40 + (index * 31) % 600, response_bytes: 100 + index * 17 }
  })
}
function matches(row, params) {
  for (const key of ['upstream', 'capability', 'operation', 'subject_scoped', 'actor']) if (params[key] !== undefined && row[key] !== params[key]) return false
  if (params.tool !== undefined && params.tool !== row.tool && params.tool !== row.upstream + '::' + row.tool) return false
  if (params.outcome !== undefined && (params.outcome === 'failed' ? row.outcome === 'ok' : row.outcome !== params.outcome)) return false
  return !params.search || [row.upstream, row.tool, row.actor, row.capability, row.operation, row.outcome].join(' ').toLowerCase().includes(params.search.trim().toLowerCase())
}
function integer(value, fallback, min, max) {
  if (value === undefined) return fallback
  if (!Number.isSafeInteger(value) || value < min || value > max) throw new RangeError('Fixture numeric parameter out of bounds')
  return value
}
export function createUsageFixtureCalls(params) {
  const rows = dataset(params).filter(row => matches(row, params)).reverse()
  const limit = integer(params.limit, 50, 1, 100)
  if (params.cursor !== undefined && params.cursor !== null && (typeof params.cursor !== 'string' || !/^fixture:(0|[1-9][0-9]{0,2})$/.test(params.cursor))) throw new TypeError('Invalid fixture cursor')
  const offset = params.cursor ? Number(params.cursor.slice(8)) : 0
  if (offset > 240) throw new RangeError('Fixture cursor out of bounds')
  return { calls: rows.slice(offset, offset + limit), total_matching: rows.length, next_cursor: offset + limit < rows.length ? `fixture:${offset + limit}` : null }
}
export function createUsageFixtureMetrics(params) {
  const all = dataset(params)
  const rows = all.filter(row => matches(row, params))
  const buckets = Math.min(integer(params.bucket_count, 24, 0, 24), params.until_unix - params.since_unix)
  const offset = integer(params.timezone_offset_minutes, 0, -840, 840)
  const timeseries = Array.from({ length: buckets }, (_, index) => ({ ts_unix: params.since_unix + Math.floor(index * (params.until_unix - params.since_unix) / buckets), calls: 0, failed: 0 }))
  const hourly = Array.from({ length: 24 }, (_, hour) => ({ hour, calls: 0 }))
  const tools = new Map(), upstreams = new Map(), actors = new Map(), errors = new Map(), minutes = new Map()
  for (const row of rows) {
    const failed = Number(row.outcome !== 'ok')
    const key = JSON.stringify([row.upstream, row.tool, row.capability, row.operation, row.subject_scoped])
    const tool = tools.get(key) ?? { upstream: row.upstream, tool: row.tool, capability: row.capability, operation: row.operation, subject_scoped: row.subject_scoped, calls: 0, failed: 0, elapsed: 0 }
    tool.calls++; tool.failed += failed; tool.elapsed += row.elapsed_ms; tools.set(key, tool)
    const upstream = upstreams.get(row.upstream) ?? { upstream: row.upstream, calls: 0, failed: 0 }
    upstream.calls++; upstream.failed += failed; upstreams.set(row.upstream, upstream)
    actors.set(row.actor, (actors.get(row.actor) ?? 0) + 1)
    if (failed) errors.set(row.outcome, (errors.get(row.outcome) ?? 0) + 1)
    hourly[((Math.floor((row.ts_unix + offset * 60) / 3600) % 24) + 24) % 24].calls++
    minutes.set(Math.floor(row.ts_unix / 60), (minutes.get(Math.floor(row.ts_unix / 60)) ?? 0) + 1)
    if (buckets) { const bucket = timeseries[Math.min(buckets - 1, Math.floor((row.ts_unix - params.since_unix) * buckets / (params.until_unix - params.since_unix)))]; bucket.calls++; bucket.failed += failed }
  }
  const dimension = tool => ({ upstream: tool.upstream, tool: tool.tool, capability: tool.capability, operation: tool.operation, subject_scoped: tool.subject_scoped })
  const counts = [...tools.values()].map(tool => ({ ...dimension(tool), calls: tool.calls, failed: tool.failed }))
  const latencies = rows.map(row => row.elapsed_ms).sort((a, b) => a - b)
  const percentile = fraction => latencies[Math.max(0, Math.ceil(latencies.length * fraction) - 1)] ?? 0
  const facetRows = params.include_facets === false ? [] : all
  const unique = key => [...new Set(facetRows.map(row => row[key]))].sort()
  return {
    window_total_calls: all.length, total_calls: rows.length, error_calls: rows.filter(row => row.outcome !== 'ok').length,
    avg_elapsed_ms: rows.length ? rows.reduce((sum, row) => sum + row.elapsed_ms, 0) / rows.length : 0,
    p50_elapsed_ms: percentile(.5), p95_elapsed_ms: percentile(.95), p99_elapsed_ms: percentile(.99),
    distinct_tools: new Set(rows.map(row => row.upstream + '::' + row.tool)).size, distinct_actors: actors.size,
    peak_per_min: Math.max(0, ...minutes.values()),
    top_tools: [...counts].sort((a, b) => b.calls - a.calls), least_tools: [...counts].sort((a, b) => a.calls - b.calls),
    top_actors: [...actors].map(([actor, calls]) => ({ actor, calls })).sort((a, b) => b.calls - a.calls),
    slowest_tools: [...tools.values()].map(tool => ({ ...dimension(tool), avg_elapsed_ms: tool.elapsed / tool.calls })).sort((a, b) => b.avg_elapsed_ms - a.avg_elapsed_ms),
    errors: [...errors].map(([kind, calls]) => ({ kind, calls })), upstreams: [...upstreams.values()], hourly, timeseries,
    facets: { tools: [...new Map(facetRows.map(row => [row.upstream + '::' + row.tool, { upstream: row.upstream, tool: row.tool }])).values()], capabilities: unique('capability'), operations: unique('operation'), subject_scopes: unique('subject_scoped'), actors: unique('actor'), upstreams: unique('upstream'), outcomes: unique('outcome') },
  }
}
