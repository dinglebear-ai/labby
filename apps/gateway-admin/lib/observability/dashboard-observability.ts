import type { DashboardMetrics, MetricsWindow } from '@/lib/types/metrics'
import type { ServerLogEntry } from '@/lib/types/traces'

const WINDOW_MS: Record<MetricsWindow, number> = {
  '1h': 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
}

const TERMINAL_DISPATCH_MESSAGES = new Set([
  'dispatch ok',
  'dispatch error',
  'upstream dispatch ok',
  'upstream dispatch error',
])
const CODE_MODE_MESSAGES = new Set(['gateway codemode ok', 'gateway codemode failed'])

function numberField(entry: ServerLogEntry, name: string): number | null {
  const value = entry.fields[name]
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function stringField(entry: ServerLogEntry, name: string): string | null {
  const value = entry.fields[name]
  return typeof value === 'string' && value.length > 0 ? value : null
}

function toolLabel(entry: ServerLogEntry): string {
  const serviceAction = [entry.service, entry.action].filter(Boolean).join('::')
  return stringField(entry, 'tool') ?? (serviceAction || 'unattributed')
}

export function enrichDashboardWithObservability(
  metrics: DashboardMetrics,
  entries: ServerLogEntry[],
  window: MetricsWindow,
  now: number,
): DashboardMetrics {
  const since = now - WINDOW_MS[window]
  const retained = entries.filter((entry) => {
    if (!entry.timestamp) return false
    const timestamp = Date.parse(entry.timestamp)
    return Number.isFinite(timestamp) && timestamp >= since && timestamp <= now
  })
  const terminal = retained.filter((entry) =>
    entry.message
    && TERMINAL_DISPATCH_MESSAGES.has(entry.message)
    && !(entry.service === 'server_logs' && entry.action === 'server_logs.query'),
  )

  const surfaceCounts = new Map<string, number>()
  const tokensByTool = new Map<string, number>()
  let inputTokens = 0
  let outputTokens = 0
  let tokenEvents = 0
  for (const entry of terminal) {
    const surface = stringField(entry, 'surface')
    if (surface) surfaceCounts.set(surface, (surfaceCounts.get(surface) ?? 0) + 1)

    const input = numberField(entry, 'input_tokens')
    const output = numberField(entry, 'output_tokens')
    if (input === null && output === null) continue
    const total = (input ?? 0) + (output ?? 0)
    inputTokens += input ?? 0
    outputTokens += output ?? 0
    tokenEvents += 1
    const label = toolLabel(entry)
    tokensByTool.set(label, (tokensByTool.get(label) ?? 0) + total)
  }

  const codeModeRuns = retained.filter((entry) => entry.message && CODE_MODE_MESSAGES.has(entry.message))
  const calls = codeModeRuns.map((entry) => numberField(entry, 'call_count') ?? 0)
  const totalCalls = calls.reduce((total, count) => total + count, 0)
  const artifactWrites = codeModeRuns.reduce(
    (total, entry) => total + (numberField(entry, 'artifact_writes') ?? 0),
    0,
  )
  const timedOut = codeModeRuns.filter((entry) => entry.kind === 'timeout' || entry.fields.kind === 'timeout').length
  const truncated = codeModeRuns.filter((entry) => entry.fields.truncated === true).length

  return {
    ...metrics,
    collected: {
      ...metrics.collected,
      surfaces: true,
      tokens: true,
      fan_out: true,
    },
    surfaces: [...surfaceCounts]
      .map(([surface, calls]) => ({ surface, calls }))
      .sort((left, right) => right.calls - left.calls),
    tokens: {
      input: inputTokens,
      output: outputTokens,
      total: inputTokens + outputTokens,
      avg_per_call: tokenEvents > 0 ? Math.round((inputTokens + outputTokens) / tokenEvents) : 0,
    },
    tokens_by_tool: [...tokensByTool]
      .map(([name, tokens]) => ({ name, tokens }))
      .sort((left, right) => right.tokens - left.tokens),
    fan_out: {
      runs: codeModeRuns.length,
      total_calls: totalCalls,
      avg_calls_per_run: codeModeRuns.length > 0 ? totalCalls / codeModeRuns.length : 0,
      max_calls_in_run: Math.max(0, ...calls),
      timeout_rate: codeModeRuns.length > 0 ? timedOut / codeModeRuns.length : 0,
      truncation_rate: codeModeRuns.length > 0 ? truncated / codeModeRuns.length : 0,
      artifact_writes: artifactWrites,
    },
  }
}
