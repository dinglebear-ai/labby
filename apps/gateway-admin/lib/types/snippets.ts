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
}

export interface ResolvedSnippet extends SnippetInfo {
  body: string
}

export interface SnippetListResponse {
  snippets: SnippetInfo[]
}

export interface CreateSnippetInput {
  name: string
  body: string
  description?: string
  force?: boolean
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
  result?: unknown
  calls?: unknown[]
  logs?: unknown[]
}
