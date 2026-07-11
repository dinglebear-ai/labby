import { z } from 'zod'

export type CodeModeTrace =
  | CodeModeExecuteTrace
  | CodeModeSearchTrace
  | CodeModeHistoryTrace

export interface CodeModeTraceWarning {
  kind: 'dropped_rows'
  message: string
}

const finiteNumberSchema = z.number().finite()
const optionalFiniteNumberSchema = z.preprocess(
  (value) => (typeof value === 'number' && Number.isFinite(value) ? value : undefined),
  finiteNumberSchema.optional(),
)
const optionalBooleanSchema = z.preprocess(
  (value) => (typeof value === 'boolean' ? value : undefined),
  z.boolean().optional(),
)
const booleanValueSchema = z.preprocess((value) => value === true, z.boolean())
const stringArraySchema = z.preprocess(
  (value) => (Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : undefined),
  z.array(z.string()).optional(),
)

const resultShapeSchema = z.object({
  type: z.string().catch('unknown'),
  size_bytes: optionalFiniteNumberSchema,
  length: optionalFiniteNumberSchema,
  key_count: optionalFiniteNumberSchema,
  keys: stringArraySchema,
  item_types: stringArraySchema,
  truncated: optionalBooleanSchema,
  content_block_kinds: stringArraySchema,
})

const callTraceSchema = z
  .object({
    id: z.string().catch(''),
    namespace: z.string().optional(),
    upstream: z.string().optional(),
    tool: z.string().catch(''),
    ok: booleanValueSchema,
    elapsed_ms: finiteNumberSchema.catch(0),
    params: z.unknown().optional(),
    error_kind: z.string().optional(),
  })
  .transform(({ upstream, namespace, ...trace }) => ({
    ...trace,
    namespace: namespace ?? upstream ?? '',
  }))

const searchMatchSchema = z
  .object({
    id: z.string().catch(''),
    namespace: z.string().optional(),
    upstream: z.string().optional(),
    tool: z.string().catch(''),
    description: z.string().catch(''),
    has_schema: booleanValueSchema,
    has_output_schema: booleanValueSchema,
  })
  .transform(({ upstream, namespace, ...match }) => ({
    ...match,
    namespace: namespace ?? upstream ?? '',
  }))

const historyEntryBaseSchema = z.object({
  seq: finiteNumberSchema.catch(0),
  kind: z.enum(['search', 'execute']),
  ok: booleanValueSchema,
  elapsed_ms: finiteNumberSchema.catch(0),
  error_kind: z.string().optional(),
  calls: z.array(z.unknown()).optional(),
  match_count: optionalFiniteNumberSchema,
})

export type ResultShape = z.infer<typeof resultShapeSchema>
export type CodeModeCallTrace = z.infer<typeof callTraceSchema>
export type CodeModeSearchMatch = z.infer<typeof searchMatchSchema>
export interface CodeModeHistoryEntry {
  seq: number
  kind: 'search' | 'execute'
  ok: boolean
  elapsed_ms: number
  error_kind?: string
  calls?: CodeModeCallTrace[]
  match_count?: number
}
export interface CodeModeExecuteTrace {
  kind: 'code_mode_execute_trace'
  call_count: number
  calls: CodeModeCallTrace[]
  result_shape?: ResultShape
  result?: unknown
  logs_count?: number
  warnings?: CodeModeTraceWarning[]
}
export interface CodeModeSearchTrace {
  kind: 'code_mode_search_trace'
  query_kind: string
  match_count: number
  displayed_count?: number
  truncated?: boolean
  matches: CodeModeSearchMatch[]
  result_shape?: ResultShape
  result?: unknown
  warnings?: CodeModeTraceWarning[]
}
export interface CodeModeHistoryTrace {
  kind: 'code_mode_history'
  entries: CodeModeHistoryEntry[]
  warnings?: CodeModeTraceWarning[]
}

export function parseCodeModeTrace(value: unknown): CodeModeTrace | null {
  if (!isRecord(value)) return null
  if (value.kind === 'code_mode_execute_trace') return parseExecuteTrace(value)
  if (value.kind === 'code_mode_search_trace') return parseSearchTrace(value)
  if (value.kind === 'code_mode_history') return parseHistoryTrace(value)
  return null
}

export function stringifyRedactedParams(value: unknown): string {
  if (value === undefined || value === null) return ''
  try {
    return JSON.stringify(value, null, 2)
  } catch (error) {
    const reason = error instanceof Error && error.message ? error.message : 'unsupported value'
    return `[unsupported params: ${truncateText(reason, 96)}]`
  }
}

export function flattenTraceRows(trace: CodeModeTrace | null) {
  if (!trace) return { calls: [], matches: [], history: [] }
  if (trace.kind === 'code_mode_execute_trace') {
    return { calls: trace.calls, matches: [], history: [] }
  }
  if (trace.kind === 'code_mode_search_trace') {
    return { calls: [], matches: trace.matches, history: [] }
  }
  return {
    calls: trace.entries.flatMap((entry) => entry.calls ?? []),
    matches: [],
    history: trace.entries,
  }
}

function parseExecuteTrace(value: Record<string, unknown>): CodeModeExecuteTrace | null {
  const calls = arrayOfWithDropped(value.calls, parseCallTrace)
  if (!calls) return null
  return {
    kind: 'code_mode_execute_trace',
    call_count: numberValue(value.call_count, calls.items.length),
    calls: calls.items,
    result_shape: parseResultShape(value.result_shape),
    result: 'result' in value ? value.result : undefined,
    logs_count: optionalNumber(value.logs_count),
    warnings: droppedWarning(calls.dropped, 'execute call'),
  }
}

function parseSearchTrace(value: Record<string, unknown>): CodeModeSearchTrace | null {
  const matches = arrayOfWithDropped(value.matches, parseSearchMatch)
  if (!matches) return null
  return {
    kind: 'code_mode_search_trace',
    query_kind: stringValue(value.query_kind, 'catalog_filter'),
    match_count: numberValue(value.match_count, matches.items.length),
    displayed_count: optionalNumber(value.displayed_count),
    truncated: booleanOptional(value.truncated),
    matches: matches.items,
    result_shape: parseResultShape(value.result_shape),
    result: 'result' in value ? value.result : undefined,
    warnings: droppedWarning(matches.dropped, 'search match'),
  }
}

/**
 * Human-readable one-line description of a result shape, used by the inspector
 * when a search returned a value with no summarizable tool rows. Returns an
 * empty string when no shape is available.
 */
export function describeResultShape(shape: ResultShape | undefined): string {
  if (!shape?.type) return ''
  const parts: string[] = [shape.type]
  if (shape.type === 'object' && shape.key_count !== undefined) {
    parts.push(`${shape.key_count} key${shape.key_count === 1 ? '' : 's'}`)
  }
  if (shape.type === 'array' && shape.length !== undefined) {
    parts.push(`${shape.length} item${shape.length === 1 ? '' : 's'}`)
  }
  if (shape.type === 'string' && shape.length !== undefined) {
    parts.push(`${shape.length} chars`)
  }
  if (shape.size_bytes !== undefined) parts.push(`${shape.size_bytes} B`)
  let label = parts.join(' · ')
  if (shape.type === 'object' && shape.keys?.length) {
    label += ` — keys: ${shape.keys.join(', ')}`
  }
  if (shape.type === 'array' && shape.item_types?.length) {
    label += ` — items: ${shape.item_types.join(', ')}`
  }
  return label
}

function parseHistoryTrace(value: Record<string, unknown>): CodeModeHistoryTrace | null {
  const entries = arrayOfWithDropped(value.entries, parseHistoryEntry)
  if (!entries) return null
  return {
    kind: 'code_mode_history',
    entries: entries.items,
    warnings: droppedWarning(entries.dropped, 'history entry'),
  }
}

function parseHistoryEntry(value: unknown): CodeModeHistoryEntry | null {
  const parsed = historyEntryBaseSchema.safeParse(value)
  if (!parsed.success) return null
  return {
    ...parsed.data,
    calls: arrayOf(parsed.data.calls, parseCallTrace) ?? [],
  }
}

function parseCallTrace(value: unknown): CodeModeCallTrace | null {
  const parsed = callTraceSchema.safeParse(value)
  return parsed.success ? parsed.data : null
}

function parseSearchMatch(value: unknown): CodeModeSearchMatch | null {
  const parsed = searchMatchSchema.safeParse(value)
  return parsed.success ? parsed.data : null
}

function parseResultShape(value: unknown): ResultShape | undefined {
  const parsed = resultShapeSchema.safeParse(value)
  return parsed.success ? parsed.data : undefined
}

function arrayOf<T>(value: unknown, parse: (item: unknown) => T | null): T[] | null {
  const result = arrayOfWithDropped(value, parse)
  return result?.items ?? null
}

function arrayOfWithDropped<T>(
  value: unknown,
  parse: (item: unknown) => T | null,
): { items: T[]; dropped: number } | null {
  if (!Array.isArray(value)) return null
  const items: T[] = []
  let dropped = 0
  for (const item of value) {
    const parsed = parse(item)
    if (parsed) {
      items.push(parsed)
    } else {
      dropped += 1
    }
  }
  return { items, dropped }
}

function stringValue(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback
}

function numberValue(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function booleanOptional(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined
}

function droppedWarning(count: number, label: string): CodeModeTraceWarning[] | undefined {
  if (count <= 0) return undefined
  return [
    {
      kind: 'dropped_rows',
      message: `Dropped ${count} malformed ${label}${count === 1 ? '' : 's'}.`,
    },
  ]
}

function truncateText(value: string, maxLength: number): string {
  return value.length <= maxLength ? value : `${value.slice(0, maxLength - 3)}...`
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
