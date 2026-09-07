import { getSessionAuthority, getSessionCsrfToken } from '@/lib/auth/session-store'
import { resolveStashOwner, stashOwnerUnsupportedMessage } from './owner'
import type { GrantPage, StashFile, StashGrant, StashPage, StashStats } from './types'

export class StashError extends Error {
  constructor(message: string, public readonly status: number, public readonly kind?: string) {
    super(message)
    this.name = 'StashError'
  }
}

/** Raised before any request is sent when the active workspace has no File Stash. */
export const STASH_WORKSPACE_UNSUPPORTED = 'workspace_unsupported'

async function throwForStatus(response: Response): Promise<void> {
  if (response.ok) return
  const body = await response.json().catch(() => ({})) as { message?: string; kind?: string; code?: string }
  throw new StashError(body.message || `File Stash request failed (${response.status})`, response.status, body.kind || body.code)
}

async function parse<T>(response: Response): Promise<T> {
  await throwForStatus(response)
  if (response.status === 204) return undefined as T
  return response.json() as Promise<T>
}

function csrfHeaders(json = false): Headers {
  const headers = new Headers(json ? { 'content-type': 'application/json' } : undefined)
  const token = getSessionCsrfToken()
  if (token) headers.set('x-csrf-token', token)
  return headers
}

/**
 * Every File Stash request selects its owner through the
 * `x-labby-owner-kind` / `x-labby-owner-id` headers. Owner identifiers never
 * appear in a URL, so they cannot leak through history, referrers, or logs.
 */
function request(path: string, init: RequestInit = {}) {
  const headers = new Headers(init.headers)
  const resolution = resolveStashOwner(getSessionAuthority())
  if (!resolution.ok) return Promise.reject(new StashError(stashOwnerUnsupportedMessage(resolution.reason), 0, STASH_WORKSPACE_UNSUPPORTED))
  if (resolution.owner) { headers.set('x-labby-owner-kind', resolution.owner.kind); headers.set('x-labby-owner-id', resolution.owner.id) }
  return fetch(`/v1/stash${path}`, { credentials: 'include', cache: 'no-store', ...init, headers })
}

export async function listFiles(cursor?: string, signal?: AbortSignal, search?: string): Promise<StashPage> {
  const query = new URLSearchParams()
  if (cursor) query.set('cursor', cursor)
  if (search) query.set('query', search)
  return parse(await request(`/?${query}`, { signal }))
}

export async function getStats(signal?: AbortSignal): Promise<StashStats> {
  return parse(await request('/stats', { signal }))
}

export async function searchRecipients(query: string, signal?: AbortSignal): Promise<Array<{ principal_id: string; display_name: string }>> {
  const response = await parse<{ recipients: Array<{ principal_id: string; display_name: string }> }>(await request('/recipients', { method: 'POST', headers: csrfHeaders(true), body: JSON.stringify({ query }), signal }))
  return response.recipients
}

export async function uploadFile(file: File, signal?: AbortSignal): Promise<{ file_id: string; uri: string }> {
  const headers = csrfHeaders()
  headers.set('x-labby-stash-filename', encodeURIComponent(file.name))
  return parse(await request('/uploads', {
    method: 'POST', headers, body: file, signal,
  }))
}

/**
 * Same-origin content path for a file. It carries no owner selection: the
 * page intercepts the anchor and downloads through `downloadFile`, which sends
 * the owner headers, so the href only exists as an accessible link target.
 */
export function downloadUrl(fileId: string): string {
  return `/v1/stash/files/${encodeURIComponent(fileId)}/content`
}

/** Fetch a file's bytes under the active workspace's owner selection. */
export async function downloadFile(fileId: string, signal?: AbortSignal): Promise<Blob> {
  const response = await request(`/files/${encodeURIComponent(fileId)}/content`, { signal })
  await throwForStatus(response)
  return response.blob()
}

export async function renameFile(fileId: string, displayName: string): Promise<StashFile> {
  return parse(await request(`/files/${encodeURIComponent(fileId)}`, {
    method: 'PATCH', headers: csrfHeaders(true), body: JSON.stringify({ display_name: displayName }),
  }))
}

export async function deleteFile(fileId: string): Promise<void> {
  await parse(await request(`/files/${encodeURIComponent(fileId)}`, { method: 'DELETE', headers: csrfHeaders() }))
}

export async function listGrants(fileId: string, signal?: AbortSignal, cursor?: string): Promise<GrantPage> {
  const query = new URLSearchParams()
  if (cursor) query.set('cursor', cursor)
  return parse(await request(`/files/${encodeURIComponent(fileId)}/grants?${query}`, { signal }))
}

export async function createGrant(fileId: string, principalId: string): Promise<StashGrant> {
  return parse(await request(`/files/${encodeURIComponent(fileId)}/grants`, {
    method: 'POST', headers: csrfHeaders(true), body: JSON.stringify({ grantee_principal_id: principalId }),
  }))
}

export async function revokeGrant(fileId: string, grantId: string): Promise<void> {
  await parse(await request(`/files/${encodeURIComponent(fileId)}/grants/${encodeURIComponent(grantId)}`, {
    method: 'DELETE', headers: csrfHeaders(),
  }))
}
