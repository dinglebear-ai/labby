import { performServiceAction, type ServiceActionError } from './service-action-client'

export interface PhoenixStatus {
  enabled: boolean
  available: boolean
  runtime: 'container_local'
  service: 'codex-app-server'
  sandbox: 'read-only'
  protocol?: {
    schema: string
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
    unsupported: string[]
  }
}

export interface PhoenixMessage {
  role: 'user' | 'assistant'
  text: string
}

export interface PhoenixSession {
  session_id: string
  status: 'ready'
  messages: PhoenixMessage[]
  events?: PhoenixEvent[]
}

export interface PhoenixModel {
  id: string
  model: string
  displayName: string
  description: string
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
  start: (model?: string, effort?: string, signal?: AbortSignal) => action<PhoenixSession>('phoenix.session.start', { model, effort }, signal),
  read: (sessionId: string, signal?: AbortSignal) => action<PhoenixSession>('phoenix.session.read', { session_id: sessionId }, signal),
  close: (sessionId: string, signal?: AbortSignal) => action<{ session_id: string; status: 'closed' }>('phoenix.session.close', { session_id: sessionId }, signal),
  send: (sessionId: string, input: string, attachments: PhoenixAttachment[] = [], signal?: AbortSignal) =>
    action<PhoenixSession>('phoenix.turn.send', { session_id: sessionId, input, attachments }, signal),
  interrupt: (sessionId: string, signal?: AbortSignal) =>
    action<PhoenixSession>('phoenix.turn.interrupt', { session_id: sessionId }, signal),
}
