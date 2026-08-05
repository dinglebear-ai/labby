export type CodeModeTrace = CodeModeExecuteTrace | CodeModeHistoryTrace

export interface CodeModeExecuteTrace {
  kind: 'code_mode_execute_trace'
  call_count: number
  calls: CodeModeCallTrace[]
  result_shape?: ResultShape
  result?: unknown
  execution_id?: string
  /** Wall-clock run time, injected by the MCP handler. */
  elapsed_ms?: number
  /** Present on failed runs — the ToolError kind that aborted the snippet. */
  error_kind?: string
  /** Complete model-actionable contract emitted for an uncaught failure. */
  error?: CodeModeErrorContract
  input_tokens?: number
  output_tokens?: number
  logs_count?: number
  artifacts?: CodeModeArtifactReceipt[]
  warnings?: CodeModeTraceWarning[]
}

/**
 * Highest error-contract version this UI understands. A numerically newer
 * contract is dropped with a warning instead of being misrendered as v1.
 */
const SUPPORTED_CONTRACT_VERSION = 1

// The closed vocabularies below mirror docs/contracts/schemas/
// code-mode-call-error.schema.json. `(string & {})` keeps them open enums:
// documented values autocomplete, but a forward-compatible token still parses.

export type CodeModeErrorOrigin =
  | 'code_mode'
  | 'tool_execution'
  | 'upstream_transport'
  | 'validation'
  | 'policy'
  | 'budget'
  | (string & {})

export type CodeModeRecoveryAction =
  | 'revise_and_retry'
  | 'retry_later'
  | 'reauthenticate'
  | 'confirm'
  | 'rediscover'
  | 'reduce_work'
  | 'start_dependency'
  | 'inspect_and_escalate'
  | 'do_not_retry'
  | (string & {})

export type CodeModeRecoverySameArguments =
  | 'safe'
  | 'conditional'
  | 'discouraged'
  | 'never'
  | (string & {})

export type CodeModeErrorSideEffects = 'none_expected' | 'possible' | 'unknown' | (string & {})

export interface CodeModeRecoveryAdvice {
  action: CodeModeRecoveryAction
  same_arguments: CodeModeRecoverySameArguments
  guidance: string
  retry_after_ms?: number
}

/** MCP tool-annotation hints echoed for the failed tool (schema `$defs/safety`). */
export interface CodeModeErrorSafety {
  read_only_hint?: boolean
  destructive_hint?: boolean
  idempotent_hint?: boolean
  open_world_hint?: boolean
}

/** Raw failure evidence from the upstream result (schema `$defs/evidence`). */
export interface CodeModeErrorEvidence {
  content?: unknown[]
  structured_content?: unknown
  parsed_error?: unknown
  omitted_content_blocks?: number
}

export interface CodeModeErrorContract {
  contract_version: number
  kind: string
  message: string
  tool?: string
  origin: CodeModeErrorOrigin
  recovery: CodeModeRecoveryAdvice
  side_effects: CodeModeErrorSideEffects
  original_kind?: string
  cause?: string
  safety?: CodeModeErrorSafety
  evidence?: CodeModeErrorEvidence
}

export interface CodeModeArtifactReceipt {
  path: string
  content_type?: string
  bytes?: number
  sha256?: string
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
  /** Offset from execution start to dispatch, in ms — enables the waterfall. */
  start_ms?: number
  params?: unknown
  error_kind?: string
  ui?: CodeModeCallUi
}

export interface CodeModeCallUi {
  resourceUri: string
  [key: string]: unknown
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
  const payload = unwrapCodeModeTracePayload(value)
  if (!isRecord(payload)) return null
  if (payload.kind === 'code_mode_execute_trace') return parseExecuteTrace(payload)
  if (payload.kind === 'code_mode_history') return parseHistoryTrace(payload)
  return null
}

function unwrapCodeModeTracePayload(value: unknown, depth = 0): unknown {
  if (depth > 4 || !isRecord(value)) return value
  if (value.kind === 'code_mode_execute_trace' || value.kind === 'code_mode_history') return value

  for (const key of ['structuredContent', 'structured_content', 'toolOutput', 'output', 'result']) {
    if (!Object.prototype.hasOwnProperty.call(value, key)) continue
    const nested = unwrapCodeModeTracePayload(value[key], depth + 1)
    if (isRecord(nested) && (nested.kind === 'code_mode_execute_trace' || nested.kind === 'code_mode_history')) {
      return nested
    }
  }

  if (Array.isArray(value.content)) {
    for (const block of value.content) {
      if (!isRecord(block) || typeof block.text !== 'string') continue
      try {
        const nested = unwrapCodeModeTracePayload(JSON.parse(block.text), depth + 1)
        if (isRecord(nested) && (nested.kind === 'code_mode_execute_trace' || nested.kind === 'code_mode_history')) {
          return nested
        }
      } catch {
        // Text content is allowed to be non-JSON.
      }
    }
  }

  return value
}

export interface DiscoveryHit {
  id: string
  namespace?: string
  name?: string
  description?: string
  path?: string
  kind?: string
  signature?: string
  score?: number
}

export interface DiscoveryResult {
  hits: DiscoveryHit[]
  total: number
  truncated: boolean
  hint?: string
}

/**
 * Detect the in-sandbox `codemode.search()` closure's return shape
 * (`{ results, total, truncated, hint? }` — see labby-codemode preamble.rs).
 * Discovery runs make zero broker calls, so the hits arrive only as the
 * execute trace's `result`; this lets the inspector render them as match rows
 * instead of a bare "no calls" line.
 */
export function parseDiscoveryResult(result: unknown): DiscoveryResult | null {
  if (!isRecord(result)) return null
  if (!Array.isArray(result.results) || typeof result.total !== 'number') return null
  const hits: DiscoveryHit[] = []
  for (const item of result.results) {
    if (!isRecord(item)) return null
    const id = typeof item.id === 'string' ? item.id : typeof item.path === 'string' ? item.path : null
    if (id === null) return null
    hits.push({
      id,
      namespace: optionalString(item.namespace),
      name: optionalString(item.name),
      description: optionalString(item.description),
      path: optionalString(item.path),
      kind: optionalString(item.kind),
      signature: optionalString(item.signature),
      score: optionalNumber(item.score),
    })
  }
  return {
    hits,
    total: result.total,
    truncated: result.truncated === true,
    hint: optionalString(result.hint),
  }
}

/**
 * Detect the `codemode.describe()` closure's return shape
 * (`{ path, id, kind, markdown }`) so the inspector can show the markdown doc
 * instead of a JSON-escaped blob.
 */
export function describeMarkdown(result: unknown): string | null {
  if (!isRecord(result)) return null
  if (typeof result.markdown !== 'string' || typeof result.id !== 'string') return null
  return result.markdown
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
  const error = parseCodeModeError(value.error)
  // An `error` key that fails to parse means "contract sent but broken" — warn
  // like dropped calls do, rather than rendering as if no contract was sent.
  const droppedErrors = value.error !== undefined && error === undefined ? 1 : 0
  const warnings = [
    ...(droppedWarning(calls.dropped, 'execute call') ?? []),
    ...(droppedWarning(droppedErrors, 'error contract') ?? []),
  ]
  return {
    kind: 'code_mode_execute_trace',
    call_count: numberValue(value.call_count, calls.items.length),
    calls: calls.items,
    result_shape: parseResultShape(value.result_shape),
    result: 'result' in value ? value.result : undefined,
    execution_id: optionalString(value.execution_id),
    elapsed_ms: optionalNumber(value.elapsed_ms),
    error_kind: optionalString(value.error_kind),
    error,
    input_tokens: optionalNumber(value.input_tokens),
    output_tokens: optionalNumber(value.output_tokens),
    logs_count: optionalNumber(value.logs_count),
    artifacts: parseArtifacts(value.artifacts),
    warnings: warnings.length > 0 ? warnings : undefined,
  }
}

function parseCodeModeError(value: unknown): CodeModeErrorContract | undefined {
  if (!isRecord(value)) return undefined
  // A numerically newer contract may have changed field semantics — fail loud
  // instead of rendering it as v1. Absent/non-numeric versions keep the
  // lenient v1 default below.
  if (
    typeof value.contract_version === 'number' &&
    value.contract_version > SUPPORTED_CONTRACT_VERSION
  ) {
    return undefined
  }
  const recovery = isRecord(value.recovery) ? value.recovery : null
  if (
    typeof value.kind !== 'string' ||
    typeof value.message !== 'string' ||
    recovery === null ||
    typeof recovery.action !== 'string' ||
    typeof recovery.same_arguments !== 'string' ||
    typeof recovery.guidance !== 'string'
  ) {
    return undefined
  }
  return {
    contract_version: numberValue(value.contract_version, 1),
    kind: value.kind,
    message: value.message,
    tool: optionalString(value.tool),
    origin: stringValue(value.origin, 'code_mode'),
    recovery: {
      action: recovery.action,
      same_arguments: recovery.same_arguments,
      guidance: recovery.guidance,
      retry_after_ms: optionalNumber(recovery.retry_after_ms),
    },
    side_effects: stringValue(value.side_effects, 'unknown'),
    original_kind: optionalString(value.original_kind),
    cause: optionalString(value.cause),
    safety: parseErrorSafety(value.safety),
    evidence: parseErrorEvidence(value.evidence),
  }
}

function parseErrorSafety(value: unknown): CodeModeErrorSafety | undefined {
  if (!isRecord(value)) return undefined
  return {
    read_only_hint: booleanOptional(value.read_only_hint),
    destructive_hint: booleanOptional(value.destructive_hint),
    idempotent_hint: booleanOptional(value.idempotent_hint),
    open_world_hint: booleanOptional(value.open_world_hint),
  }
}

function parseErrorEvidence(value: unknown): CodeModeErrorEvidence | undefined {
  if (!isRecord(value)) return undefined
  return {
    content: Array.isArray(value.content) ? value.content : undefined,
    structured_content: 'structured_content' in value ? value.structured_content : undefined,
    parsed_error: 'parsed_error' in value ? value.parsed_error : undefined,
    omitted_content_blocks: optionalNumber(value.omitted_content_blocks),
  }
}

function parseArtifacts(value: unknown): CodeModeArtifactReceipt[] | undefined {
  if (!Array.isArray(value)) return undefined
  const receipts: CodeModeArtifactReceipt[] = []
  for (const item of value) {
    if (!isRecord(item) || typeof item.path !== 'string') continue
    receipts.push({
      path: item.path,
      content_type: optionalString(item.content_type),
      bytes: optionalNumber(item.bytes),
      sha256: optionalString(item.sha256),
    })
  }
  return receipts.length > 0 ? receipts : undefined
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
    start_ms: optionalNumber(value.start_ms),
    params: value.params,
    error_kind: optionalString(value.error_kind),
    ui: parseCallUi(value.ui),
  }
}

function parseCallUi(value: unknown): CodeModeCallUi | undefined {
  if (!isRecord(value)) return undefined
  const resourceUri = optionalString(value.resourceUri)
  if (!resourceUri) return undefined
  return { ...value, resourceUri }
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
