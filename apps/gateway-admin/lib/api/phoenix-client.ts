import { performServiceAction, type ServiceActionError } from './service-action-client'

export interface PhoenixStatus {
  enabled: boolean
  available: boolean
  runtime: 'container_local'
  service: 'codex-app-server'
  sandbox: 'read-only'
  protocol?: {
    schema: string
    runtime_version?: string | null
    adapter: number
    experimental_api: boolean
  }
  mcp?: {
    name: string
    transport: string
    scope: string
    authentication: string
    configured: boolean
  }
  capabilities?: {
    session_lifecycle: string[]
    turn_lifecycle: string[]
    preserved_events: string[]
    inputs: string[]
    operations?: string[]
    diagnostics?: string[]
    unsupported: string[]
  }
}

export interface PhoenixMessage {
  role: 'user' | 'assistant'
  text: string
  created_at_ms?: number
}

export interface PhoenixSession {
  session_id: string
  status: 'ready'
  messages: PhoenixMessage[]
  events?: PhoenixEvent[]
}

export interface PhoenixSessionSummary {
  session_id: string
  title: string
  preview: string
  model?: string | null
  effort?: string | null
  message_count: number
  turn_status: 'ready' | 'in_progress'
}

export interface PhoenixSessionList {
  sessions: PhoenixSessionSummary[]
}

export interface PhoenixModel {
  id: string
  model: string
  displayName: string
  description: string
  contextWindow?: number
  isDefault: boolean
  inputModalities?: Array<'text' | 'image' | 'audio'>
  defaultReasoningEffort: string
  supportedReasoningEfforts: Array<{ reasoningEffort: string; description: string }>
}

export interface PhoenixAttachment {
  type: 'image' | 'audio'
  url: string
  name: string
}

export interface PhoenixEvent {
  method: string
  params: unknown
}

export interface PhoenixDiagnostics {
  account?: unknown
  rate_limits?: unknown
  usage?: unknown
  config?: unknown
  mcp_servers?: unknown
}

export interface PhoenixTurnControl {
  session_id: string
  status: 'interrupting' | 'steered'
  turn_id?: string
  turn?: unknown
}

export interface PhoenixReview {
  session_id: string
  status: 'reviewing'
  review: unknown
}

export function phoenixSupports(status: PhoenixStatus | undefined, capability: string): boolean {
  if (!status?.capabilities) return false
  return Object.values(status.capabilities).some((values) =>
    Array.isArray(values) && values.includes(capability),
  ) && !status.capabilities.unsupported?.includes(capability)
}

export class PhoenixApiError extends Error implements ServiceActionError {
  constructor(public status: number, message: string, public code?: string) {
    super(message)
    this.name = 'PhoenixApiError'
  }
}

function action<T>(name: string, params: object, signal?: AbortSignal) {
  return performServiceAction<T, PhoenixApiError>({
    action: name,
    params,
    signal,
    serviceLabel: 'Phoenix',
    url: '/v1/phoenix',
    source: 'phoenix',
    createError: (message, status, code) => new PhoenixApiError(status, message, code),
  })
}

export const phoenixApi = {
  status: (signal?: AbortSignal) => action<PhoenixStatus>('phoenix.status', {}, signal),
  models: (signal?: AbortSignal) => action<{ models: PhoenixModel[] }>('phoenix.models.list', {}, signal),
  list: (signal?: AbortSignal) => action<PhoenixSessionList>('phoenix.session.list', {}, signal),
  start: (model?: string, effort?: string, signal?: AbortSignal) => action<PhoenixSession>('phoenix.session.start', { model, effort }, signal),
  read: (sessionId: string, signal?: AbortSignal) => action<PhoenixSession>('phoenix.session.read', { session_id: sessionId }, signal),
  rename: (sessionId: string, title: string, signal?: AbortSignal) =>
    action<PhoenixSessionSummary>('phoenix.session.rename', { session_id: sessionId, title }, signal),
  close: (sessionId: string, signal?: AbortSignal) => action<{ session_id: string; status: 'closed' }>('phoenix.session.close', { session_id: sessionId }, signal),
  send: (sessionId: string, input: string, attachments: PhoenixAttachment[] = [], signal?: AbortSignal) =>
    action<PhoenixSession>('phoenix.turn.send', { session_id: sessionId, input, attachments }, signal),
  interrupt: (sessionId: string, signal?: AbortSignal) =>
    action<PhoenixTurnControl>('phoenix.turn.interrupt', { session_id: sessionId }, signal),
  steer: (sessionId: string, input: string, attachments: PhoenixAttachment[] = [], signal?: AbortSignal) =>
    action<PhoenixTurnControl>('phoenix.turn.steer', { session_id: sessionId, input, attachments }, signal),
  review: (sessionId: string, targetType: string, target?: string, signal?: AbortSignal) =>
    action<PhoenixReview>('phoenix.review.start', { session_id: sessionId, target_type: targetType, target }, signal),
  diagnostics: (signal?: AbortSignal) => action<PhoenixDiagnostics>('phoenix.diagnostics.read', {}, signal),
}
