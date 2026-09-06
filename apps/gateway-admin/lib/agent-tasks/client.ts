import { getSessionCsrfToken } from '@/lib/auth/session-store'

export type AgentView = { agent_id: string; owner_kind: string; owner_id: string; version: number; state: string; catalog_generation: string }
export type TaskView = { task_id: string; owner_kind: string; owner_id: string; agent_id: string; agent_version: number; state: string; attempt: number; output_digest?: string | null; error_code?: string | null }

async function action<T>(service: 'agents' | 'tasks', name: string, params: Record<string, unknown> = {}, signal?: AbortSignal): Promise<T> {
  const headers = new Headers({ 'content-type': 'application/json' })
  const csrf = getSessionCsrfToken()
  if (csrf) headers.set('x-csrf-token', csrf)
  const response = await fetch(`/v1/${service}/`, { method: 'POST', credentials: 'include', cache: 'no-store', headers, body: JSON.stringify({ action: name, params }), signal })
  if (!response.ok) throw new Error(`${service} request failed (${response.status})`)
  return response.json() as Promise<T>
}

export async function listAgents(signal?: AbortSignal): Promise<AgentView[]> {
  return (await action<{ agents: AgentView[] }>('agents', 'agents.list', {}, signal)).agents
}
export async function listTasks(signal?: AbortSignal): Promise<TaskView[]> {
  return (await action<{ tasks: TaskView[] }>('tasks', 'tasks.list', {}, signal)).tasks
}
