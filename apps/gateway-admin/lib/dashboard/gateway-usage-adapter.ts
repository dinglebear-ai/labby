import type {
  ActorUsageEntry,
  DashboardMetrics,
  ErrorKindCount,
  HourBucket,
  LatencyStat,
  MetricsWindow,
  ToolUsageEntry,
  UpstreamUsage,
} from '@/lib/types/metrics'

export interface GatewayUsageToolCount {
  upstream: string
  tool: string
  capability?: string
  operation?: string
  subject_scoped?: boolean
  calls: number
  failed: number
}

export interface GatewayUsageMetrics {
  window_total_calls: number
  total_calls: number
  error_calls: number
  avg_elapsed_ms: number
  p50_elapsed_ms: number
  p95_elapsed_ms: number
  p99_elapsed_ms: number
  distinct_tools: number
  distinct_actors: number
  peak_per_min: number
  top_tools: GatewayUsageToolCount[]
  least_tools: GatewayUsageToolCount[]
  top_actors: Array<{ actor: string; calls: number }>
  slowest_tools: Array<{ upstream: string; tool: string; avg_elapsed_ms: number }>
  errors: Array<{ kind: string; calls: number }>
  upstreams: Array<{ upstream: string; calls: number; failed: number }>
  hourly: Array<{ hour: number; calls: number }>
  timeseries: Array<{ ts_unix: number; calls: number; failed: number }>
  facets: {
    tools: Array<{ upstream: string; tool: string }>
    actors: string[]
    upstreams: string[]
    outcomes: string[]
  }
}

export interface GatewayUsageCall {
  ts_unix: number
  upstream: string
  tool: string
  capability?: string
  operation?: string
  subject_scoped?: boolean
  actor: string
  outcome: string
  elapsed_ms: number
  response_bytes?: number | null
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

type UsageDimension = Pick<GatewayUsageToolCount, 'upstream' | 'tool' | 'capability' | 'operation' | 'subject_scoped'>

function toolName(call: Pick<GatewayUsageToolCount, 'upstream' | 'tool'>) {
  return `${call.upstream}::${call.tool}`
}

function usageDimensionKey(call: UsageDimension) {
  return [
    call.upstream,
    call.tool,
    call.capability ?? 'tools',
    call.operation ?? 'tool.call',
    call.subject_scoped ? 'subject' : 'shared',
  ].join('\u0000')
}

function usageDimensionName(call: UsageDimension) {
  const capability = call.capability ?? 'tools'
  const operation = call.operation ?? 'tool.call'
  const base = call.tool ? `${call.upstream}::${call.tool}` : `${call.upstream}::${capability}`
  const qualifiers = [
    ...(operation !== 'tool.call' ? [operation] : []),
    ...(call.subject_scoped ? ['OAuth'] : []),
  ]
  return qualifiers.length > 0 ? `${base} · ${qualifiers.join(' · ')}` : base
}

function mapTool(tool: GatewayUsageToolCount): ToolUsageEntry {
  return {
    name: toolName(tool),
    id: usageDimensionKey(tool),
    label: usageDimensionName(tool),
    calls: tool.calls,
    failed: tool.failed,
  }
}

export function aggregateGatewayUsage(
  window: MetricsWindow,
  now: number,
  summary: GatewayUsageMetrics,
): DashboardMetrics {
  const tools = summary.top_tools.map(mapTool)
  const least = summary.least_tools.map(mapTool)
  const actors: ActorUsageEntry[] = summary.top_actors.map((actor) => ({
    id: actor.actor,
    label: actor.actor,
    kind: 'agent',
    calls: actor.calls,
  }))
  const errors: ErrorKindCount[] = summary.errors.map((error) => ({
    kind: error.kind,
    count: error.calls,
  }))
  const upstreams: UpstreamUsage[] = summary.upstreams.map((upstream) => ({
    name: upstream.upstream,
    calls: upstream.calls,
    failed: upstream.failed,
  }))
  const slowest: LatencyStat[] = summary.slowest_tools.map((tool) => ({
    // Drill-down identity stays the stable upstream::tool target even when the
    // aggregate row itself is dimensioned by OAuth/capability/operation.
    name: toolName(tool),
    avg_ms: Math.round(tool.avg_elapsed_ms),
  }))
  const hourlyByHour = new Map(summary.hourly.map((entry) => [entry.hour, entry.calls]))
  const hourly: HourBucket[] = Array.from({ length: 24 }, (_, hour) => ({
    hour,
    calls: hourlyByHour.get(hour) ?? 0,
  }))
  const busiestHour = hourly.reduce(
    (best, entry) => entry.calls > best.calls ? entry : best,
    hourly[0],
  ).hour

  return {
    window,
    since_ms: now - WINDOW_MS[window],
    until_ms: now,
    collected: {
      tokens: false,
      surfaces: false,
      fan_out: false,
      actor_kinds: false,
      complete_window_analytics: true,
    },
    tool_calls: {
      total: summary.total_calls,
      failed: summary.error_calls,
      succeeded: Math.max(0, summary.total_calls - summary.error_calls),
    },
    tools: { top: tools, least, distinct: summary.distinct_tools },
    tokens: { input: 0, output: 0, total: 0, avg_per_call: 0 },
    actors: {
      agent: { active: summary.distinct_actors, top: actors },
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
      p50: summary.p50_elapsed_ms,
      p95: summary.p95_elapsed_ms,
      p99: summary.p99_elapsed_ms,
      avg: summary.avg_elapsed_ms,
      slowest,
    },
    errors: { total: summary.error_calls, by_kind: errors },
    surfaces: [],
    tokens_by_tool: [],
    upstreams,
    throughput: {
      peak_per_min: summary.peak_per_min,
      avg_per_min: Math.round((summary.total_calls / (WINDOW_MS[window] / 60_000)) * 100) / 100,
      busiest_hour: busiestHour,
    },
    hourly,
    agents_seen: { new: 0, returning: summary.distinct_actors },
    timeseries: summary.timeseries.map((bucket) => ({
      ts: bucket.ts_unix * 1000,
      calls: bucket.calls,
      failed: bucket.failed,
    })),
  }
}
