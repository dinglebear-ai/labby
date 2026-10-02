export type SnippetSource = 'builtin' | 'user'

export type SnippetInputType =
  | 'string'
  | 'integer'
  | 'number'
  | 'boolean'
  | 'object'
  | 'array'
  | 'json'

export interface SnippetInputSpec {
  ty: SnippetInputType
  nullable?: boolean
  required?: boolean
  default?: unknown
  description?: string
}

export interface SnippetInfo {
  name: string
  description?: string | null
  tags: string[]
  inputs?: Record<string, SnippetInputSpec>
  source: SnippetSource
  path: string
  shadowed: boolean
  content_digest?: string | null
}

export interface ResolvedSnippet extends SnippetInfo {
  body: string
}

export interface SnippetListResponse {
  snippets: SnippetInfo[]
  diagnostics_omitted?: number
  diagnostics?: Array<{ name: string; path: string; message: string; source: SnippetSource }>
}

export interface CreateSnippetInput {
  name: string
  body: string
  description?: string
  force?: boolean
  expected_digest?: string
}

export interface SnippetRemoveResult {
  name: string
  removed: boolean
}

export interface SnippetValidation {
  valid: boolean
  name: string
  mode: 'body' | 'existing'
  source?: SnippetSource
  path?: string
}

export interface SnippetTestMetrics {
  wall_clock_ms: number
  tool_calls: number
  output_bytes: number
  estimated_tokens: number
  max_in_flight?: number
  calls_by_tool?: Record<string, number>
  failed_calls?: number
  truncated?: boolean
}

export interface SnippetTestResult {
  receipt_status?: string
  name?: string
  passed: boolean
  mode?: 'mock' | 'live'
  metrics?: SnippetTestMetrics
  failures?: string[]
  result?: unknown
  calls?: unknown[]
  trace_truncated?: boolean
  response?: unknown
  results?: Array<{
    name: string
    passed: boolean
    mode?: 'mock' | 'live'
    metrics?: SnippetTestMetrics
    failures?: string[]
    trace_truncated?: boolean
    response?: unknown
    error?: unknown
  }>
}

export interface CodeModeExecutionResponse {
  execution_id?: string
  receipt_status?: string
  result?: unknown
  calls?: unknown[]
  logs?: unknown[]
}

export interface SnippetReceiptCall {
  tool: string
  params_digest: string | null
  ok: boolean
  elapsed_ms: number
  error_kind: string | null
}

export interface SnippetReceiptArtifact {
  path: string
  sha256: string
  bytes: number
  content_type: string
}

export interface SnippetExecutionReceipt {
  execution_id: string
  snippet_name: string
  snippet_digest: string
  input_digest: string
  effective_scope_fingerprint: string
  runtime_version: string
  tool_schema_digests?: Record<string,string|null>
  surface: string
  created_at_ms: number
  elapsed_ms: number
  status: string
  error_kind: string | null
  result_digest: string | null
  result_bytes: number | null
  calls: SnippetReceiptCall[]
  tool_calls: number
  omitted_calls: number
  artifacts: SnippetReceiptArtifact[]
}

export interface SnippetHistoryResponse {
  receipts: SnippetExecutionReceipt[]
  next_cursor: string | null
  receipt_status: 'persisted' | 'disabled'
}

export interface SnippetArtifactResponse {
  path: string
  sha256: string
  bytes: number
  content_type: string
  content_base64: string
}

export type SnippetDriftField = 'snippet' | 'input' | 'effective_scope' | 'runtime' | 'tool_schema'
export interface SnippetPreview {
 name: string; execution_id?: string; mode: 'metadata'; dynamic_unknown: true
 coverage: 'declared_tools_only' | 'unrestricted_dynamic'; can_execute: boolean
 input_summary: {keys: string[]; provided_keys: string[]; defaulted_keys: string[]}
 declared_tools: Array<{id: string; status: 'allowed' | 'denied' | 'unavailable'; annotations: {read_only?: boolean; destructive?: boolean} | null; schema_digest: string | null; parameters: 'unresolved'}>
 fingerprints: {snippet_digest: string; input_digest: string; effective_scope_fingerprint: string; runtime_version: string; tool_schema_digest: string}
 preview_fingerprint: string; drift: Array<{field: SnippetDriftField; status: 'changed' | 'unverifiable'}>; warnings: string[]
}
