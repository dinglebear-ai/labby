import { performServiceAction, type ServiceActionError } from './service-action-client'

export interface PhoenixStatus {
  enabled: boolean
  available: boolean
  runtime: 'container_local'
  service: 'codex-app-server'
  sandbox: 'read-only'
}

export interface PhoenixMessage {
  role: 'user' | 'assistant'
  text: string
}

export interface PhoenixSession {
  session_id: string
  status: 'ready'
  messages: PhoenixMessage[]
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
  start: (signal?: AbortSignal) => action<PhoenixSession>('phoenix.session.start', {}, signal),
  send: (sessionId: string, input: string, signal?: AbortSignal) =>
    action<PhoenixSession>('phoenix.turn.send', { session_id: sessionId, input }, signal),
}
