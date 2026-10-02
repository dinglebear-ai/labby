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
