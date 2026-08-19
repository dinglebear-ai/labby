export interface ServerLogEntry {
  timestamp: string | null
  level: string | null
  target: string | null
  message: string | null
  service: string | null
  action: string | null
  kind: string | null
  file: string
  fields: Record<string, unknown>
}

export interface ServerLogsResult {
  kind: 'server_logs'
  entries: ServerLogEntry[]
  matched: number
  scanned_lines: number
  malformed_lines: number
  scanned_bytes: number
  max_scan_bytes: number
  truncated: boolean
}

export interface RequestTrace {
  id: string
  started_at: number
  elapsed_ms: number
  surface: string
  service: string
  action: string
  actor_key: string | null
  outcome: 'ok' | 'failed' | 'incomplete'
  error_kind: string | null
  upstreams: string[]
  response_bytes: number
  input_tokens: number
  output_tokens: number
  events: ServerLogEntry[]
}

export interface TraceSummary {
  traces: RequestTrace[]
  total: number
  failed: number
  incomplete: number
  p50_ms: number
  p95_ms: number
  surfaces: Array<{ name: string; count: number }>
  upstreams: Array<{ name: string; count: number }>
}
