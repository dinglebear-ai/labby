export type CodeModeTrace = CodeModeExecuteTrace | CodeModeHistoryTrace

export interface CodeModeExecuteTrace {
  kind: 'code_mode_execute_trace'
  call_count: number
  calls: CodeModeCallTrace[]
  result_shape?: ResultShape
  result?: unknown
  execution_id?: string
  input_tokens?: number
  output_tokens?: number
  logs_count?: number
  warnings?: CodeModeTraceWarning[]
}

export interface CodeModeHistoryTrace {
  kind: 'code_mode_history'
  entries: CodeModeHistoryEntry[]
  warnings?: CodeModeTraceWarning[]
}

export interface CodeModeTraceWarning {
  kind: 'dropped_rows'
  message: string
}

export interface CodeModeHistoryEntry {
  seq: number
  execution_id?: string
  kind: 'search' | 'execute'
  ok: boolean
  elapsed_ms: number
  input_tokens?: number
  output_tokens?: number
  error_kind?: string
  calls?: CodeModeCallTrace[]
  match_count?: number
}

export interface CodeModeCallTrace {
  id: string
  upstream: string
  tool: string
  ok: boolean
  elapsed_ms: number
  params?: unknown
  error_kind?: string
}

export interface ResultShape {
  type: string
  size_bytes?: number
  length?: number
  key_count?: number
  keys?: string[]
  item_types?: string[]
  truncated?: boolean
  content_block_kinds?: string[]
}

export function parseCodeModeTrace(value: unknown): CodeModeTrace | null {
  if (!isRecord(value)) return null
  if (value.kind === 'code_mode_execute_trace') return parseExecuteTrace(value)
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

function parseExecuteTrace(value: Record<string, unknown>): CodeModeExecuteTrace | null {
  const calls = arrayOfWithDropped(value.calls, parseCallTrace)
  if (!calls) return null
  return {
    kind: 'code_mode_execute_trace',
    call_count: numberValue(value.call_count, calls.items.length),
    calls: calls.items,
    result_shape: parseResultShape(value.result_shape),
    result: 'result' in value ? value.result : undefined,
    execution_id: optionalString(value.execution_id),
    input_tokens: optionalNumber(value.input_tokens),
    output_tokens: optionalNumber(value.output_tokens),
    logs_count: optionalNumber(value.logs_count),
    warnings: droppedWarning(calls.dropped, 'execute call'),
  }
}

/**
 * Human-readable one-line description of a result shape, e.g.
 * `object · 3 keys · 212 B — keys: containers, unhealthy, notified`.
 * Returns an empty string when no shape is available.
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
  if (!isRecord(value)) return null
  let kind: CodeModeHistoryEntry['kind']
  switch (value.kind) {
    case 'execute':
      kind = 'execute'
      break
    case 'search':
      kind = 'search'
      break
    default:
      return null
  }
  return {
    seq: numberValue(value.seq, 0),
    execution_id: optionalString(value.execution_id),
    kind,
    ok: booleanValue(value.ok),
    elapsed_ms: numberValue(value.elapsed_ms, 0),
    input_tokens: optionalNumber(value.input_tokens),
    output_tokens: optionalNumber(value.output_tokens),
    error_kind: optionalString(value.error_kind),
    calls: arrayOf(value.calls, parseCallTrace) ?? [],
    match_count: optionalNumber(value.match_count),
  }
}

function parseCallTrace(value: unknown): CodeModeCallTrace | null {
  if (!isRecord(value)) return null
  const id = stringValue(value.id, '')
  // The gateway emits `namespace` for the upstream segment
  // (crates/labby-codemode/src/trace.rs); history entries carry only `id`.
  const fromId = splitCallId(id)
  return {
    id,
    upstream: stringValue(value.namespace, stringValue(value.upstream, fromId.upstream)),
    tool: stringValue(value.tool, fromId.tool),
    ok: booleanValue(value.ok),
    elapsed_ms: numberValue(value.elapsed_ms, 0),
    params: value.params,
    error_kind: optionalString(value.error_kind),
  }
}

function splitCallId(id: string): { upstream: string; tool: string } {
  const separator = id.indexOf('::')
  if (separator < 0) return { upstream: '', tool: id }
  return { upstream: id.slice(0, separator), tool: id.slice(separator + 2) }
}

function parseResultShape(value: unknown): ResultShape | undefined {
  if (!isRecord(value)) return undefined
  return {
    type: stringValue(value.type, 'unknown'),
    size_bytes: optionalNumber(value.size_bytes),
    length: optionalNumber(value.length),
    key_count: optionalNumber(value.key_count),
    keys: stringArray(value.keys),
    item_types: stringArray(value.item_types),
    truncated: booleanOptional(value.truncated),
    content_block_kinds: stringArray(value.content_block_kinds),
  }
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

function stringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) return undefined
  return value.filter((item): item is string => typeof item === 'string')
}

function stringValue(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback
}

function optionalString(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined
}

function numberValue(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function booleanValue(value: unknown): boolean {
  return value === true
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
