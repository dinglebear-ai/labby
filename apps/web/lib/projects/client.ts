import { getSessionCsrfToken } from '@/lib/auth/session-store'
import { assertGatewayAuthorityCurrent, captureGatewayAuthority } from '@/lib/api/gateway-request'

export type ProjectView = {
  project_id: string
  team_id: string
  name: string
  status: string
  role: string
  policy_epoch: number
  /** Server-derived lifecycle authority; the UI renders it and never re-derives it from local roles. */
  can_manage: boolean
}

async function action<T>(name: string, params: Record<string, unknown> = {}, mutation = false, signal?: AbortSignal): Promise<T> {
  const authority = captureGatewayAuthority(signal)
  const headers = new Headers({ 'content-type': 'application/json' })
  if (mutation) {
    const token = getSessionCsrfToken()
    if (token) headers.set('x-csrf-token', token)
  }
  try {
    const response = await fetch('/v1/projects/', { method: 'POST', credentials: 'include', cache: 'no-store', headers, body: JSON.stringify({ action: name, params }), signal: authority.signal })
    if (!response.ok) throw new Error(`Project request failed (${response.status})`)
    const value = await response.json() as T
    assertGatewayAuthorityCurrent(authority.generation)
    return value
  } finally {
    authority.finish()
  }
}

export const listProjects = (signal?: AbortSignal) => action<ProjectView[]>('projects.list', {}, false, signal)
export const createProject = (team_id: string, project_id: string, name: string) => action<ProjectView>('projects.create', { team_id, project_id, name }, true)
export const updateProject = (team_id: string, project_id: string, name: string) => action<ProjectView>('projects.update', { team_id, project_id, name }, true)
export const archiveProject = (team_id: string, project_id: string) => action<ProjectView>('projects.archive', { team_id, project_id }, true)
