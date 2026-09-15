import { performServiceAction, type ServiceActionError } from '@/lib/api/service-action-client'
import { captureGatewayAuthority } from '@/lib/api/gateway-request'
import { getSessionAuthority } from '@/lib/auth/session-store'

export type AgentView = {
  agent_id: string
  owner_kind: string
  owner_id: string
  version: number
  state: string
  content_digest: string
  repository_digest: string
  image_digest: string
  harness_digest: string
  harness_id?: string | null
  loadout_digest: string
  catalog_generation: string
}
export type TaskView = { task_id: string; owner_kind: string; owner_id: string; agent_id: string; agent_version: number; state: string; attempt: number; output_digest?: string | null; error_code?: string | null }

export type AgentHarnessView = {
  id: string
  digest: string
  available: boolean
  content_digest: string
  repository_digest: string
  image_digest: string
  loadout_digest: string
  catalog_generation: string
}

export type AgentSessionView = {
  agent_id: string
  agent_version: number
  session_id: string
  status: string
  input_digest: string
  output_digest?: string | null
  error_code?: string | null
  resumed_from_session_id?: string | null
  authority_expires_at: number
  created_at: number
  updated_at: number
  completed_at?: number | null
}

export type AgentTranscriptView = {
  agent_id: string
  session_id: string
  status: string
  input: string
  transcript?: string | null
  truncated: boolean
}

export type AgentRunResult = Pick<AgentSessionView, 'agent_id' | 'agent_version' | 'session_id' | 'status' | 'input_digest' | 'authority_expires_at'> & {
  resumed_from_session_id?: string | null
}

export type AgentSessionStatusResult = Pick<AgentSessionView, 'agent_id' | 'session_id' | 'status'>

export class AgentTaskClientError extends Error implements ServiceActionError {
  constructor(
    message: string,
    public readonly status: number,
    public readonly code?: string,
    public readonly param?: string,
  ) {
    super(message)
    this.name = 'AgentTaskClientError'
  }
}

async function action<T>(service: 'agents' | 'tasks', name: string, params: Record<string, unknown> = {}, signal?: AbortSignal): Promise<T> {
  const authority = getSessionAuthority() ? captureGatewayAuthority(signal) : undefined
  try {
    return await performServiceAction<T, AgentTaskClientError>({
    action: name,
    params,
    signal: authority?.signal ?? signal,
    serviceLabel: service === 'agents' ? 'Agent' : 'Task',
    url: `/v1/${service}`,
    createError: (message, status, code, param) => new AgentTaskClientError(message === 'An error occurred' ? `${service === 'agents' ? 'Agent' : 'Task'} request failed (${status})` : message, status, code, param),
  })
  } finally {
    authority?.finish()
  }
}

const AGENT_PAGE_LIMIT = '100'
const MAX_AGENT_PAGES = 100

async function allAgentPages<T>(
  name: 'agents.list' | 'agents.sessions.list',
  key: 'agents' | 'sessions',
  params: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<T[]> {
  const rows: T[] = []
  const visited = new Set<string>()
  let cursor: string | undefined
  for (let page = 0; page < MAX_AGENT_PAGES; page++) {
    const result = await action<Partial<Record<'agents' | 'sessions', T[]>> & { next_cursor?: string | null }>(
      'agents',
      name,
      { ...params, limit: AGENT_PAGE_LIMIT, ...(cursor ? { cursor } : {}) },
      signal,
    )
    rows.push(...(result[key] ?? []))
    if (!result.next_cursor) return rows
    if (visited.has(result.next_cursor)) {
      throw new Error('The Agent server repeated a pagination cursor. Refresh to retry.')
    }
    cursor = result.next_cursor
    visited.add(cursor)
  }
  throw new Error('The Agent inventory exceeds the bounded listing limit.')
}

export async function listAgents(signal?: AbortSignal): Promise<AgentView[]> {
  return allAgentPages<AgentView>('agents.list', 'agents', {}, signal)
}
export async function listTasks(signal?: AbortSignal): Promise<TaskView[]> {
  return (await action<{ tasks: TaskView[] }>('tasks', 'tasks.list', {}, signal)).tasks
}

export async function listAgentHarnesses(signal?: AbortSignal): Promise<AgentHarnessView[]> {
  return (await action<{ harnesses: AgentHarnessView[] }>('agents', 'agents.harnesses', {}, signal)).harnesses
}

export async function createAgentFromHarness(agentId: string, harness: AgentHarnessView, signal?: AbortSignal): Promise<AgentView> {
  const owner = getSessionAuthority()?.activeOwner
  if (!owner || owner.kind === 'installation') throw new DOMException('An Agent owner workspace is unavailable', 'InvalidStateError')
  return action<AgentView>('agents', 'agents.create', {
    agent_id: agentId,
    owner_kind: owner.kind,
    owner_id: owner.id,
    content_digest: harness.content_digest,
    repository_digest: harness.repository_digest,
    image_digest: harness.image_digest,
    harness_digest: harness.digest,
    loadout_digest: harness.loadout_digest,
    catalog_generation: harness.catalog_generation,
  }, signal)
}

export async function listAgentSessions(agentId: string, signal?: AbortSignal): Promise<AgentSessionView[]> {
  return allAgentPages<AgentSessionView>('agents.sessions.list', 'sessions', { agent_id: agentId }, signal)
}

export async function listVisibleAgentSessions(agents: readonly AgentView[], signal?: AbortSignal): Promise<AgentSessionView[]> {
  const pages = await Promise.all(agents.map(agent => listAgentSessions(agent.agent_id, signal)))
  return pages.flat().sort((left, right) => right.updated_at - left.updated_at)
}

export async function runAgent(agentId: string, input: string, idempotencyKey: string, signal?: AbortSignal): Promise<AgentRunResult> {
  return action<AgentRunResult>('agents', 'agents.run', { agent_id: agentId, input, idempotency_key: idempotencyKey }, signal)
}

export async function getAgentSession(agentId: string, sessionId: string, signal?: AbortSignal): Promise<AgentSessionView> {
  return action<AgentSessionView>('agents', 'agents.session.get', { agent_id: agentId, session_id: sessionId }, signal)
}

export async function getAgentTranscript(agentId: string, sessionId: string, signal?: AbortSignal): Promise<AgentTranscriptView> {
  return action<AgentTranscriptView>('agents', 'agents.session.transcript', { agent_id: agentId, session_id: sessionId }, signal)
}

export async function stopAgentSession(agentId: string, sessionId: string, signal?: AbortSignal): Promise<AgentSessionStatusResult> {
  return action<AgentSessionStatusResult>('agents', 'agents.session.stop', { agent_id: agentId, session_id: sessionId }, signal)
}

const pendingResumeKeys = new Map<string, string>()

function resumeIntent(agentId: string, sessionId: string) {
  const authority = getSessionAuthority()
  const owner = authority?.activeOwner
  return [authority?.principalId ?? '', owner?.kind ?? '', owner?.id ?? '', agentId, sessionId].join('\u0000')
}

export async function resumeAgentSession(agentId: string, sessionId: string, signal?: AbortSignal): Promise<AgentRunResult> {
  const intent = resumeIntent(agentId, sessionId)
  const idempotencyKey = pendingResumeKeys.get(intent) ?? crypto.randomUUID()
  pendingResumeKeys.set(intent, idempotencyKey)
  const result = await action<AgentRunResult>('agents', 'agents.session.resume', {
    agent_id: agentId,
    session_id: sessionId,
    idempotency_key: idempotencyKey,
  }, signal)
  pendingResumeKeys.delete(intent)
  return result
}
