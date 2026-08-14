import type {
  ActorUsageEntry,
  DashboardMetrics,
  ErrorKindCount,
  HourBucket,
  LatencyStat,
  MetricsBucket,
  MetricsWindow,
  ToolUsageEntry,
  UpstreamUsage,
} from '@/lib/types/metrics'

export interface GatewayUsageMetrics {
  total_calls: number
  error_calls: number
  avg_elapsed_ms: number
  top_tools: Array<{ upstream: string; tool: string; calls: number }>
  top_actors: Array<{ actor: string; calls: number }>
}

export interface GatewayUsageCall {
  ts_unix: number
  upstream: string
  tool: string
  actor: string
  outcome: string
  elapsed_ms: number
}

export interface GatewayUsageCalls {
  calls: GatewayUsageCall[]
  total_matching?: number | null
  next_cursor?: string | null
}

const WINDOW_MS: Record<MetricsWindow, number> = {
  '1h': 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
}
const WINDOW_BUCKETS: Record<MetricsWindow, number> = { '1h': 12, '24h': 24, '7d': 14 }

function toolName(call: Pick<GatewayUsageCall, 'upstream' | 'tool'>) {
  return `${call.upstream}::${call.tool}`
}

function percentile(values: number[], percent: number) {
  if (values.length === 0) return 0
  const sorted = [...values].sort((left, right) => left - right)
  return sorted[Math.min(sorted.length - 1, Math.ceil((percent / 100) * sorted.length) - 1)]
}

function rankedCounts<T>(values: T[], key: (value: T) => string) {
  const counts = new Map<string, number>()
  for (const value of values) counts.set(key(value), (counts.get(key(value)) ?? 0) + 1)
  return counts
}

function buildTimeseries(calls: GatewayUsageCall[], window: MetricsWindow, now: number): MetricsBucket[] {
  const count = WINDOW_BUCKETS[window]
  const width = WINDOW_MS[window] / count
  const since = now - WINDOW_MS[window]
  const buckets = Array.from({ length: count }, (_, index) => ({
    ts: since + index * width,
    calls: 0,
    failed: 0,
  }))
  for (const call of calls) {
    const index = Math.floor((call.ts_unix * 1000 - since) / width)
    if (index < 0 || index >= count) continue
    buckets[index].calls += 1
    if (call.outcome !== 'ok') buckets[index].failed += 1
  }
  return buckets
}

export function aggregateGatewayUsage(
  window: MetricsWindow,
  now: number,
  summary: GatewayUsageMetrics,
  rows: GatewayUsageCalls,
): DashboardMetrics {
  const failedByTool = rankedCounts(rows.calls.filter((call) => call.outcome !== 'ok'), toolName)
  const tools: ToolUsageEntry[] = summary.top_tools.map((tool) => {
    const name = `${tool.upstream}::${tool.tool}`
    return { name, calls: tool.calls, failed: failedByTool.get(name) ?? 0 }
  })
  const actors: ActorUsageEntry[] = summary.top_actors.map((actor) => ({
    id: actor.actor,
    label: actor.actor,
    kind: 'agent',
    calls: actor.calls,
  }))

  const errors: ErrorKindCount[] = [...rankedCounts(
    rows.calls.filter((call) => call.outcome !== 'ok'),
    (call) => call.outcome,
  )].map(([kind, count]) => ({ kind, count })).sort((a, b) => b.count - a.count)

  const upstreamCounts = rankedCounts(rows.calls, (call) => call.upstream)
  const upstreamFailures = rankedCounts(
    rows.calls.filter((call) => call.outcome !== 'ok'),
    (call) => call.upstream,
  )
  const upstreams: UpstreamUsage[] = [...upstreamCounts]
    .map(([name, calls]) => ({ name, calls, failed: upstreamFailures.get(name) ?? 0 }))
    .sort((a, b) => b.calls - a.calls)

  const latencyByTool = new Map<string, { total: number; calls: number }>()
  for (const call of rows.calls) {
    const name = toolName(call)
    const current = latencyByTool.get(name) ?? { total: 0, calls: 0 }
    current.total += call.elapsed_ms
    current.calls += 1
    latencyByTool.set(name, current)
  }
  const slowest: LatencyStat[] = [...latencyByTool]
    .map(([name, value]) => ({ name, avg_ms: Math.round(value.total / value.calls) }))
    .sort((a, b) => b.avg_ms - a.avg_ms)
    .slice(0, 5)

  const hourlyCounts = rankedCounts(rows.calls, (call) => String(new Date(call.ts_unix * 1000).getHours()))
  const hourly: HourBucket[] = Array.from({ length: 24 }, (_, hour) => ({
    hour,
    calls: hourlyCounts.get(String(hour)) ?? 0,
  }))
  const busiestHour = hourly.reduce((best, entry) => entry.calls > best.calls ? entry : best, hourly[0]).hour
  const minuteCounts = rankedCounts(rows.calls, (call) => String(Math.floor(call.ts_unix / 60)))
  const peakPerMin = Math.max(0, ...minuteCounts.values())
  const elapsedValues = rows.calls.map((call) => call.elapsed_ms)
  const distinctTools = new Set(rows.calls.map(toolName)).size
  const least = [...rankedCounts(rows.calls, toolName)]
    .map(([name, calls]) => ({ name, calls, failed: failedByTool.get(name) ?? 0 }))
    .sort((a, b) => a.calls - b.calls)
    .slice(0, 5)
  const totalMatching = rows.total_matching ?? rows.calls.length

  return {
    window,
    since_ms: now - WINDOW_MS[window],
    until_ms: now,
    collected: {
      tokens: false,
      surfaces: false,
      fan_out: false,
      actor_kinds: false,
      complete_call_rows: !rows.next_cursor && rows.calls.length >= totalMatching,
    },
    tool_calls: {
      total: summary.total_calls,
      failed: summary.error_calls,
      succeeded: Math.max(0, summary.total_calls - summary.error_calls),
    },
    tools: { top: tools, least, distinct: Math.max(distinctTools, tools.length) },
    tokens: { input: 0, output: 0, total: 0, avg_per_call: 0 },
    actors: {
      agent: { active: actors.length, top: actors },
      device: { active: 0, top: [] },
      ip: { active: 0, top: [] },
    },
    fan_out: {
      runs: 0,
      total_calls: 0,
      avg_calls_per_run: 0,
      max_calls_in_run: 0,
      timeout_rate: 0,
      truncation_rate: 0,
      artifact_writes: 0,
    },
    latency: {
      p50: percentile(elapsedValues, 50),
      p95: percentile(elapsedValues, 95),
      p99: percentile(elapsedValues, 99),
      avg: summary.avg_elapsed_ms,
      slowest,
    },
    errors: { total: summary.error_calls, by_kind: errors },
    surfaces: [],
    tokens_by_tool: [],
    upstreams,
    throughput: {
      peak_per_min: peakPerMin,
      avg_per_min: Math.round((summary.total_calls / (WINDOW_MS[window] / 60_000)) * 100) / 100,
      busiest_hour: busiestHour,
    },
    hourly,
    agents_seen: { new: 0, returning: actors.length },
    timeseries: buildTimeseries(rows.calls, window, now),
  }
}
