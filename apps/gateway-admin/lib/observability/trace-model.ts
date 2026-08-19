import type { RequestTrace, ServerLogEntry, TraceSummary } from '@/lib/types/traces'

function text(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null
}

function number(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function timestamp(entry: ServerLogEntry): number | null {
  const parsed = entry.timestamp ? Date.parse(entry.timestamp) : Number.NaN
  return Number.isFinite(parsed) ? parsed : null
}

function correlationId(entry: ServerLogEntry): string | null {
  return text(entry.fields.trace_id) ?? text(entry.fields.request_id) ?? text(entry.fields.execution_id)
}

type EventPhase = 'start' | 'finish' | 'error' | null

const SUCCESS_MESSAGES = new Set(['dispatch ok', 'upstream dispatch ok', 'gateway codemode ok'])
const ERROR_MESSAGES = new Set(['dispatch error', 'upstream dispatch error', 'gateway codemode failed'])
const START_MESSAGES = new Set(['dispatch start', 'upstream dispatch start', 'gateway codemode start'])

function eventPhase(entry: ServerLogEntry): EventPhase {
  const explicit = text(entry.fields.event)?.toLowerCase()
  if (explicit === 'start' || explicit === 'finish' || explicit === 'error') return explicit
  const message = entry.message?.toLowerCase()
  if (!message) return null
  if (SUCCESS_MESSAGES.has(message)) return 'finish'
  if (ERROR_MESSAGES.has(message)) return 'error'
  if (START_MESSAGES.has(message)) return 'start'
  return null
}

function isChildSpan(entry: ServerLogEntry): boolean {
  return text(entry.fields.surface) === 'dispatch'
    || entry.service === 'upstream.pool'
    || text(entry.fields.upstream) !== null
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

function trimTruncatedBoundary(entries: ServerLogEntry[], truncated: boolean): ServerLogEntry[] {
  if (!truncated || entries.length === 0) return entries
  const oldest = entries.at(-1)
  if (!oldest) return entries
  const boundaryId = correlationId(oldest)
  if (!boundaryId) return entries.slice(0, -1)
  return entries.filter((entry) => correlationId(entry) !== boundaryId)
}

export function buildTraceSummary(
  inputEntries: ServerLogEntry[],
  options: { truncated?: boolean } = {},
): TraceSummary {
  const entries = trimTruncatedBoundary(inputEntries, options.truncated ?? false)
  const groups = new Map<string, ServerLogEntry[]>()
  entries.forEach((entry, index) => {
    const correlation = correlationId(entry)
    const fallback = `${entry.file}:${entry.timestamp ?? 'unknown'}:${index}`
    const id = correlation ?? fallback
    groups.set(id, [...(groups.get(id) ?? []), entry])
  })

  const traces = [...groups.entries()].map(([id, events]): RequestTrace => {
    const ordered = [...events].sort((a, b) => {
      const left = timestamp(a)
      const right = timestamp(b)
      if (left === null && right === null) return 0
      if (left === null) return 1
      if (right === null) return -1
      return left - right
    })
    const fields = ordered.map((entry) => entry.fields)
    const rootEvents = ordered.filter((entry) => !isChildSpan(entry))
    const rootStart = rootEvents.find((entry) => eventPhase(entry) === 'start')
      ?? rootEvents[0]
      ?? ordered[0]
    const rootTerminal = [...rootEvents].reverse().find((entry) => {
      const phase = eventPhase(entry)
      return phase === 'finish' || phase === 'error'
    })
    const terminalPhase = rootTerminal ? eventPhase(rootTerminal) : null
    const startTimestamp = rootStart ? timestamp(rootStart) : null
    const terminalTimestamp = rootTerminal ? timestamp(rootTerminal) : null
    const wallElapsed = startTimestamp !== null && terminalTimestamp !== null
      ? Math.max(0, terminalTimestamp - startTimestamp)
      : 0
    const explicitElapsed = rootTerminal ? number(rootTerminal.fields.elapsed_ms) : 0
    const upstreams = [...new Set(fields.map((value) => text(value.upstream)).filter((value): value is string => value !== null))]
    const rootFields = rootEvents.map((entry) => entry.fields)
    const startedAt = startTimestamp
      ?? ordered.map(timestamp).find((value): value is number => value !== null)
      ?? 0

    return {
      id,
      started_at: startedAt,
      elapsed_ms: Math.round(Math.max(explicitElapsed, wallElapsed)),
      surface: rootFields.map((value) => text(value.surface)).find(Boolean)
        ?? fields.map((value) => text(value.surface)).find(Boolean)
        ?? 'internal',
      service: rootStart?.service
        ?? rootEvents.map((entry) => entry.service).find(Boolean)
        ?? ordered[0]?.service
        ?? 'runtime',
      action: rootStart?.action
        ?? rootEvents.map((entry) => entry.action).find(Boolean)
        ?? ordered[0]?.action
        ?? rootStart?.message
        ?? 'event',
      actor_key: rootFields.map((value) => text(value.actor_key)).find(Boolean)
        ?? fields.map((value) => text(value.actor_key)).find(Boolean)
        ?? null,
      outcome: terminalPhase === 'error' ? 'failed' : terminalPhase === 'finish' ? 'ok' : 'incomplete',
      error_kind: terminalPhase === 'error'
        ? rootTerminal?.kind ?? text(rootTerminal?.fields.kind) ?? null
        : null,
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
