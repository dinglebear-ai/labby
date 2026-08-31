import { gatewayRequestInit } from './gateway-request'

export type DepotStatus = {
  configured: boolean
  enabled: boolean
  mutationAuthority: boolean
  maxResponseBytes: number
}

export type DepotArtifact = {
  id?: string
  kind?: string
  namespace?: string
  name?: string
  title?: string
  description?: string
  currentRevisionId?: string
  publication?: { state?: string; visibility?: string; distribution?: string }
  license?: { reviewState?: string; takedownState?: string }
  lineage?: { following?: boolean; upstreamArtifactId?: string }
}

async function parse<T>(response: Response): Promise<T> {
  const body = await response.json().catch(() => ({ error: 'invalid_response' }))
  if (!response.ok) throw new Error(body.error ?? body.message ?? `Depot request failed (${response.status})`)
  return body as T
}

export async function depotStatus(signal?: AbortSignal): Promise<DepotStatus> {
  const response = await fetch('/v1/depot/status', { credentials: 'same-origin', signal })
  return (await parse<{ depot: DepotStatus }>(response)).depot
}

export async function depotCall<T>(operation: string, params: Record<string, unknown>, signal?: AbortSignal): Promise<T> {
  const init = gatewayRequestInit(operation, params, undefined, signal)
  init.body = JSON.stringify({ operation, params })
  return parse<T>(await fetch('/v1/depot/operations', init))
}
