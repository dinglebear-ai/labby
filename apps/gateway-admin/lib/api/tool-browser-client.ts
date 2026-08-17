import { getBrowserSessionEpoch, getSessionCsrfToken } from '@/lib/auth/session-store'
import { refreshBrowserSession } from './service-action-client'
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

const SEARCH_RESPONSE_MAX_BYTES = 256 * 1024
const DESCRIBE_RESPONSE_MAX_BYTES = 128 * 1024

async function boundedJson(response: Response, maxBytes: number) {
  const declared = Number(response.headers.get('content-length'))
  if (Number.isFinite(declared) && declared > maxBytes) {
    throw new GatewayApiError('Tools response exceeds the browser safety limit', 502, 'response_too_large', response.headers.get('x-request-id') ?? undefined)
  }
  const bytes = new Uint8Array(await response.arrayBuffer())
  if (bytes.byteLength > maxBytes) {
    throw new GatewayApiError('Tools response exceeds the browser safety limit', 502, 'response_too_large', response.headers.get('x-request-id') ?? undefined)
  }
  try {
    return JSON.parse(new TextDecoder().decode(bytes))
  } catch {
    throw new GatewayApiError('Tools returned an invalid response', 502, 'invalid_response', response.headers.get('x-request-id') ?? undefined)
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === 'string')
}

function isSearchHit(value: unknown): value is ToolSearchHit {
  if (!isRecord(value)) return false
  return value.kind === 'tool' &&
    ['path', 'id', 'namespace', 'name', 'description', 'signature'].every((key) => typeof value[key] === 'string') &&
    isStringArray(value.tags) && typeof value.score === 'number' && Number.isFinite(value.score)
}

function isSearchResponse(value: unknown): value is ToolSearchResponse {
  return isRecord(value) && Array.isArray(value.results) && value.results.every(isSearchHit) &&
    typeof value.total === 'number' && Number.isSafeInteger(value.total) && value.total >= 0 &&
    typeof value.truncated === 'boolean' && (value.hint === undefined || typeof value.hint === 'string')
}

function isDescription(value: unknown): value is ToolDescription {
  if (!isRecord(value)) return false
  return ['path', 'id', 'namespace', 'name', 'description', 'helper', 'signature'].every((key) => typeof value[key] === 'string') &&
    isStringArray(value.tags) && (value.typescript === undefined || typeof value.typescript === 'string') &&
    (value.typescript_omitted === undefined || typeof value.typescript_omitted === 'string')
}

async function post<T>(path: string, body: object, maxBytes: number, validate: (value: unknown) => value is T, signal?: AbortSignal): Promise<T> {
  const request = async () => {
    const epoch = getBrowserSessionEpoch()
    const csrfToken = getSessionCsrfToken()
    const response = await fetch(path, {
      method: 'POST', credentials: 'include', cache: 'no-store', signal,
      headers: {
        'content-type': 'application/json',
        ...(csrfToken ? { 'x-csrf-token': csrfToken } : {}),
      }, body: JSON.stringify(body),
    })
    const payload = await boundedJson(response, maxBytes)
    if (epoch !== getBrowserSessionEpoch()) throw new DOMException('Session changed', 'AbortError')
    if (!response.ok) {
      throw new GatewayApiError(payload.message ?? 'Tools unavailable', response.status, payload.kind, response.headers.get('x-request-id') ?? undefined)
    }
    if (!validate(payload)) {
      throw new GatewayApiError('Tools returned an invalid response', 502, 'invalid_response', response.headers.get('x-request-id') ?? undefined)
    }
    return payload
  }

  try {
    return await request()
  } catch (error) {
    const staleSession = error instanceof GatewayApiError &&
      [401, 403, 422].includes(error.status) &&
      (error.code === 'auth_failed' || error.message.toLowerCase().includes('csrf'))
    if (!staleSession) throw error
    const session = await refreshBrowserSession()
    if (session.status !== 'authenticated') throw error
    return request()
  }
}

export const searchCodeModeTools = (query: string, signal?: AbortSignal) =>
  post<ToolSearchResponse>('/v1/gateway/codemode/tools/search', { query, limit: 50 }, SEARCH_RESPONSE_MAX_BYTES, isSearchResponse, signal)
export const describeCodeModeTool = (target: string, signal?: AbortSignal) =>
  post<ToolDescription>('/v1/gateway/codemode/tools/describe', { target }, DESCRIBE_RESPONSE_MAX_BYTES, isDescription, signal)
