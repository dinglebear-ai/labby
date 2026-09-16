import { getSessionCsrfToken } from '@/lib/auth/session-store'
import { assertGatewayAuthorityCurrent, captureGatewayAuthority } from '@/lib/api/gateway-request'

export type OwnerKind = 'personal' | 'team' | 'project'

export type AgentView = {
  agent_id: string
  owner_kind: string
  owner_id: string
  version: number
  state: string
  catalog_generation: string
}

export type AgentRunResult = {
  agent_id: string
  agent_version: number
  session_id: string
  status: string
  output_digest: string
  output?: string | null
  authority_expires_at: number
}

export type AgentSessionStatus = {
  agent_id: string
  session_id: string
  status: unknown
}

export type TaskView = {
  task_id: string
  owner_kind: string
  owner_id: string
  agent_id: string
  agent_version: number
  state: string
  attempt: number
  output_digest?: string | null
  error_code?: string | null
}

export type TaskResult = TaskView & { output?: string | null }

export type CreateAgentInput = {
  agentId: string
  ownerKind: OwnerKind
  ownerId: string
  instructions: string
  model?: string
}

export type UpdateAgentInput = {
  agentId: string
  instructions?: string
  model?: string
}

export type CreateTaskInput = {
  taskId: string
  idempotencyKey: string
  ownerKind: OwnerKind
  ownerId: string
  agentId: string
  input: string
}

async function responseError(service: 'agents' | 'tasks', response: Response): Promise<Error> {
  const prefix = service + ' request failed (' + response.status + ')'
  let message = prefix
  try {
    const value = await response.json() as { message?: unknown; error?: { message?: unknown } }
    if (typeof value.message === 'string' && value.message.trim()) message = prefix + ': ' + value.message
    else if (typeof value.error?.message === 'string' && value.error.message.trim()) message = prefix + ': ' + value.error.message
  } catch {
    // Preserve the status-shaped fallback when the response is not JSON.
  }
  return new Error(message)
}

async function action<T>(
  service: 'agents' | 'tasks',
  name: string,
  params: Record<string, unknown> = {},
  signal?: AbortSignal,
): Promise<T> {
  const authority = captureGatewayAuthority(signal)
  const headers = new Headers({ 'content-type': 'application/json' })
  const csrf = getSessionCsrfToken()
  if (csrf) headers.set('x-csrf-token', csrf)
  try {
    const response = await fetch('/v1/' + service + '/', {
      method: 'POST',
      credentials: 'include',
      cache: 'no-store',
      headers,
      body: JSON.stringify({ action: name, params }),
      signal: authority.signal,
    })
    if (!response.ok) throw await responseError(service, response)
    const value = await response.json() as T
    assertGatewayAuthorityCurrent(authority.generation)
    return value
  } finally {
    authority.finish()
  }
}

export async function listAgents(signal?: AbortSignal): Promise<AgentView[]> {
  return (await action<{ agents: AgentView[] }>('agents', 'agents.list', {}, signal)).agents
}

export async function getAgent(agentId: string, signal?: AbortSignal): Promise<AgentView> {
  return action<AgentView>('agents', 'agents.get', { agent_id: agentId }, signal)
}

export async function createAgent(input: CreateAgentInput, signal?: AbortSignal): Promise<AgentView> {
  return action<AgentView>('agents', 'agents.create', {
    agent_id: input.agentId,
    owner_kind: input.ownerKind,
    owner_id: input.ownerId,
    instructions: input.instructions,
    ...(input.model?.trim() ? { model: input.model.trim() } : {}),
  }, signal)
}

export async function updateAgent(input: UpdateAgentInput, signal?: AbortSignal): Promise<AgentView> {
  return action<AgentView>('agents', 'agents.update', {
    agent_id: input.agentId,
    ...(input.instructions?.trim() ? { instructions: input.instructions } : {}),
    ...(input.model?.trim() ? { model: input.model.trim() } : {}),
  }, signal)
}

export async function suspendAgent(agentId: string, signal?: AbortSignal): Promise<{ agent_id: string; state: string }> {
  return action('agents', 'agents.suspend', { agent_id: agentId }, signal)
}

export async function deleteAgent(agentId: string, signal?: AbortSignal): Promise<{ agent_id: string; state: string }> {
  return action('agents', 'agents.delete', { agent_id: agentId }, signal)
}

export async function runAgent(agentId: string, input = '', signal?: AbortSignal): Promise<AgentRunResult> {
  return action<AgentRunResult>('agents', 'agents.run', { agent_id: agentId, ...(input ? { input } : {}) }, signal)
}

export async function getAgentSessionStatus(agentId: string, sessionId: string, signal?: AbortSignal): Promise<AgentSessionStatus> {
  return action('agents', 'agents.session.status', { agent_id: agentId, session_id: sessionId }, signal)
}

export async function listTasks(signal?: AbortSignal): Promise<TaskView[]> {
  return (await action<{ tasks: TaskView[] }>('tasks', 'tasks.list', {}, signal)).tasks
}

export async function getTask(taskId: string, signal?: AbortSignal): Promise<TaskView> {
  return action<TaskView>('tasks', 'tasks.get', { task_id: taskId }, signal)
}

export async function createTask(input: CreateTaskInput, signal?: AbortSignal): Promise<{ task_id: string; state: string }> {
  return action('tasks', 'tasks.create', {
    task_id: input.taskId,
    idempotency_key: input.idempotencyKey,
    owner_kind: input.ownerKind,
    owner_id: input.ownerId,
    agent_id: input.agentId,
    input: input.input,
  }, signal)
}

export async function queueTask(taskId: string, signal?: AbortSignal): Promise<{ task_id: string; state: string }> {
  return action('tasks', 'tasks.queue', { task_id: taskId }, signal)
}

export async function cancelTask(taskId: string, signal?: AbortSignal): Promise<{ task_id: string; state: string }> {
  return action('tasks', 'tasks.cancel', { task_id: taskId }, signal)
}

export async function getTaskResult(taskId: string, signal?: AbortSignal): Promise<TaskResult> {
  return action<TaskResult>('tasks', 'tasks.result', { task_id: taskId }, signal)
}
