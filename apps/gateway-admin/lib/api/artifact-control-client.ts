import { normalizeGatewayApiBase } from './gateway-config'
import { gatewayHeaders } from './gateway-request'
import { getBrowserSessionState, getSessionCsrfToken } from '../auth/session-store'
import { performServiceAction, refreshBrowserSession, type ServiceActionError } from './service-action-client'

export type ArtifactControlError = ServiceActionError

function createError(message: string, status: number, code?: string, param?: string): ArtifactControlError {
  return Object.assign(new Error(message), { name: 'ArtifactControlError', status, code, param })
}

export function controlPlaneAction<T>(service: 'artifacts' | 'sources' | 'jobs' | 'uploads' | 'bundles', action: string, params: object = {}, signal?: AbortSignal) {
  return performServiceAction<T, ArtifactControlError>({
    serviceLabel: 'Artifact control plane',
    url: `${normalizeGatewayApiBase()}/${service}`,
    action,
    params,
    signal,
    createError,
  })
}

export async function uploadArtifactBytes(uploadId: string, file: File, connectionId?: string) {
  const query = connectionId ? `?connection_id=${encodeURIComponent(connectionId)}` : ''
  const initialCsrfToken = getSessionCsrfToken()

  const request = async () => {
    const headers = new Headers(gatewayHeaders())
    headers.set('Content-Type', file.type || 'application/octet-stream')
    const response = await fetch(`${normalizeGatewayApiBase()}/uploads/${encodeURIComponent(uploadId)}${query}`, {
      method: 'PUT', headers, body: file, credentials: 'include', cache: 'no-store',
    })
    if (response.ok) return response.json() as Promise<Record<string, unknown>>
    const body = await response.json().catch(() => ({ message: 'Upload failed' })) as { message?: string; kind?: string; code?: string; param?: string }
    throw createError(body.message || 'Upload failed', response.status, body.kind || body.code, body.param)
  }

  try {
    return await request()
  } catch (error) {
    const uploadError = error as ArtifactControlError
    const authFailure = [401, 403, 422].includes(uploadError.status) && (
      uploadError.code === 'auth_failed' ||
      (Boolean(initialCsrfToken) && uploadError.code === 'validation_failed' && uploadError.message.toLowerCase().includes('csrf'))
    )
    if (!authFailure) throw error
    const current = getBrowserSessionState()
    if (current.status !== 'authenticated' || current.csrfToken === initialCsrfToken) {
      const refreshed = await refreshBrowserSession()
      if (refreshed.status !== 'authenticated') throw error
    }
    return request()
  }
}
