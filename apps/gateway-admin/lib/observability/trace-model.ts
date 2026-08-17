import type { RequestTrace, ServerLogEntry, TraceSummary } from '@/lib/types/traces'

function text(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null
}

function number(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function timestamp(entry: ServerLogEntry): number {
  const parsed = entry.timestamp ? Date.parse(entry.timestamp) : Number.NaN
  return Number.isFinite(parsed) ? parsed : 0
}

function percentile(values: number[], fraction: number): number {
  if (values.length === 0) return 0
  const sorted = [...values].sort((a, b) => a - b)
  return Math.round(sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)] ?? 0)
}

function ranked(values: string[]) {
  const counts = new Map<string, number>()
  for (const value of values.filter(Boolean)) counts.set(value, (counts.get(value) ?? 0) + 1)
  return [...counts.entries()]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name))
}

export function buildTraceSummary(entries: ServerLogEntry[]): TraceSummary {
  const groups = new Map<string, ServerLogEntry[]>()
  entries.forEach((entry, index) => {
    const fields = entry.fields
    const correlation = text(fields.trace_id) ?? text(fields.request_id) ?? text(fields.execution_id)
    const fallback = `${entry.file}:${entry.timestamp ?? 'unknown'}:${index}`
    const id = correlation ?? fallback
    groups.set(id, [...(groups.get(id) ?? []), entry])
  })

  const traces = [...groups.entries()].map(([id, events]): RequestTrace => {
    const ordered = [...events].sort((a, b) => timestamp(a) - timestamp(b))
    const fields = ordered.map((entry) => entry.fields)
    const first = ordered[0]
    const last = ordered.at(-1) ?? first
    const error = ordered.find((entry) =>
      entry.level === 'ERROR' || entry.level === 'WARN' || text(entry.fields.event) === 'error' || entry.kind,
    )
    const hasFinish = ordered.some((entry) =>
      ['finish', 'error'].includes(text(entry.fields.event) ?? '') || number(entry.fields.elapsed_ms) > 0,
    )
    const elapsed = Math.max(
      ...fields.map((value) => number(value.elapsed_ms)),
      timestamp(last) - timestamp(first),
      0,
    )
    const upstreams = [...new Set(fields.map((value) => text(value.upstream)).filter((value): value is string => value !== null))]
    return {
      id,
      started_at: timestamp(first),
      elapsed_ms: Math.round(elapsed),
      surface: fields.map((value) => text(value.surface)).find(Boolean) ?? 'internal',
      service: first.service ?? fields.map((value) => text(value.service)).find(Boolean) ?? 'runtime',
      action: first.action ?? fields.map((value) => text(value.action)).find(Boolean) ?? first.message ?? 'event',
      actor_key: fields.map((value) => text(value.actor_key)).find(Boolean) ?? null,
      outcome: error ? 'failed' : hasFinish ? 'ok' : 'incomplete',
      error_kind: error?.kind ?? text(error?.fields.kind) ?? null,
      upstreams,
      response_bytes: Math.max(...fields.map((value) => number(value.response_bytes)), 0),
      input_tokens: Math.max(...fields.map((value) => number(value.input_tokens)), 0),
      output_tokens: Math.max(...fields.map((value) => number(value.output_tokens)), 0),
      events: ordered,
    }
  }).sort((a, b) => b.started_at - a.started_at)

  const complete = traces.filter((trace) => trace.outcome !== 'incomplete')
  return {
    traces,
    total: traces.length,
    failed: traces.filter((trace) => trace.outcome === 'failed').length,
    incomplete: traces.filter((trace) => trace.outcome === 'incomplete').length,
    p50_ms: percentile(complete.map((trace) => trace.elapsed_ms), 0.5),
    p95_ms: percentile(complete.map((trace) => trace.elapsed_ms), 0.95),
    surfaces: ranked(traces.map((trace) => trace.surface)),
    upstreams: ranked(traces.flatMap((trace) => trace.upstreams)),
  }
}
