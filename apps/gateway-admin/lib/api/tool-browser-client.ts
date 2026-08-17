import { getBrowserSessionEpoch, getSessionCsrfToken } from '@/lib/auth/session-store'
import { GatewayApiError } from './gateway-client-core'

export type ToolSafety = { read_only?: boolean; destructive?: boolean }
export type ToolSearchHit = {
  path: string; id: string; kind: 'tool'; namespace: string; name: string; description: string
  signature: string; tags: string[]; score: number; safety?: ToolSafety
}
export type ToolSearchResponse = { results: ToolSearchHit[]; total: number; truncated: boolean; hint?: string }
export type ToolDescription = {
  path: string; id: string; namespace: string; name: string; description: string
  helper: string; signature: string; tags: string[]; safety?: ToolSafety
  typescript?: string; typescript_omitted?: string
}

async function post<T>(path: string, body: object, signal?: AbortSignal): Promise<T> {
  const epoch = getBrowserSessionEpoch()
  const csrfToken = getSessionCsrfToken()
  const response = await fetch(path, {
    method: 'POST', credentials: 'include', cache: 'no-store', signal,
    headers: {
      'content-type': 'application/json',
      ...(csrfToken ? { 'x-csrf-token': csrfToken } : {}),
    }, body: JSON.stringify(body),
  })
  if (epoch !== getBrowserSessionEpoch()) throw new DOMException('Session changed', 'AbortError')
  if (!response.ok) {
    const payload = await response.json().catch(() => ({ message: 'Tools unavailable' }))
    throw new GatewayApiError(payload.message ?? 'Tools unavailable', response.status, payload.kind, response.headers.get('x-request-id') ?? undefined)
  }
  return response.json()
}

export const searchCodeModeTools = (query: string, signal?: AbortSignal) =>
  post<ToolSearchResponse>('/v1/gateway/codemode/tools/search', { query, limit: 50 }, signal)
export const describeCodeModeTool = (target: string, signal?: AbortSignal) =>
  post<ToolDescription>('/v1/gateway/codemode/tools/describe', { target }, signal)
