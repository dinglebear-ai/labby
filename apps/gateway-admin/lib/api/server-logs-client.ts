import { serverLogsActionUrl } from './gateway-config'
import { performServiceAction, type ServiceActionError } from './service-action-client'
import type { ServerLogsResult } from '@/lib/types/traces'

export class ServerLogsApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'ServerLogsApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

export interface ServerLogsQuery {
  limit?: number
  level?: string
  service?: string
  action?: string
  kind?: string
  query?: string
  max_scan_bytes?: number
  stop_after_limit?: boolean
  correlated_only?: boolean
}

export interface ServerLogsRequestOptions {
  baseUrl?: string
  signal?: AbortSignal
}

export function queryServerLogs(params: ServerLogsQuery = {}, options?: ServerLogsRequestOptions) {
  if (process.env.NEXT_PUBLIC_MOCK_DATA === 'true') {
    const now = Date.now()
    const entries = [
      mockEntry(now - 1480, 'req-74b2', 'start', 'INFO', { surface: 'mcp', service: 'gateway', action: 'tool.call', actor_key: 'actor-8f21' }),
      mockEntry(now - 1412, 'req-74b2', 'start', 'INFO', { surface: 'dispatch', service: 'upstream.pool', action: 'upstream.request', upstream: 'github', operation: 'tool.call' }),
      mockEntry(now - 1260, 'req-74b2', 'finish', 'INFO', { surface: 'dispatch', service: 'upstream.pool', action: 'upstream.request', upstream: 'github', operation: 'tool.call', elapsed_ms: 152, response_bytes: 18420 }),
      mockEntry(now - 1244, 'req-74b2', 'finish', 'INFO', { surface: 'mcp', service: 'gateway', action: 'tool.call', elapsed_ms: 236, input_tokens: 84, output_tokens: 310 }),
      mockEntry(now - 4200, 'req-19ca', 'start', 'INFO', { surface: 'api', service: 'gateway', action: 'gateway.list', actor_key: 'actor-11d0' }),
      mockEntry(now - 4168, 'req-19ca', 'finish', 'INFO', { surface: 'api', service: 'gateway', action: 'gateway.list', elapsed_ms: 32 }),
      mockEntry(now - 8900, 'req-c030', 'start', 'INFO', { surface: 'mcp', service: 'gateway', action: 'tool.call', actor_key: 'actor-8f21' }),
      mockEntry(now - 8650, 'req-c030', 'error', 'WARN', { surface: 'dispatch', service: 'upstream.pool', action: 'upstream.request', upstream: 'slack', operation: 'tool.call', elapsed_ms: 250, kind: 'timeout' }),
      mockEntry(now - 8640, 'req-c030', 'error', 'WARN', { surface: 'mcp', service: 'gateway', action: 'tool.call', elapsed_ms: 260, kind: 'timeout' }),
    ]
    return Promise.resolve({
      kind: 'server_logs' as const,
      entries,
      matched: entries.length,
      scanned_lines: entries.length,
      malformed_lines: 0,
      scanned_bytes: 4096,
      max_scan_bytes: params.max_scan_bytes ?? 8 * 1024 * 1024,
      truncated: false,
    })
  }
  return performServiceAction<ServerLogsResult, ServerLogsApiError>({
    action: 'server_logs.query',
    params,
    signal: options?.signal,
    serviceLabel: 'Server logs',
    url: serverLogsActionUrl(options?.baseUrl),
    createError: (message, status, code, param) => new ServerLogsApiError(message, status, code, param),
  })
}

function mockEntry(
  timestamp: number,
  requestId: string,
  event: string,
  level: string,
  fields: Record<string, unknown>,
) {
  return {
    timestamp: new Date(timestamp).toISOString(),
    level,
    target: 'labby::observability',
    message: `${String(fields.action)} ${event}`,
    service: String(fields.service),
    action: String(fields.action),
    kind: typeof fields.kind === 'string' ? fields.kind : null,
    file: 'labby.jsonl',
    fields: { ...fields, request_id: requestId, event },
  }
}
