import { performServiceAction, type ServiceActionError } from './service-action-client'

export interface PhoenixStatus {
  enabled: boolean
  available: boolean
  runtime: 'container_local' | 'remote_http'
  service: 'codex-app-server' | 'openai-compatible'
  sandbox: 'read-only' | 'remote'
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
  turn_status?: 'ready' | 'in_progress' | 'closing'
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
  type: 'image' | 'audio' | 'text'
  url: string
  name: string
}

export interface PhoenixMcpAppResourceContent {
  text?: string
  mimeType?: string
  mime_type?: string
  uri?: string
}

export interface PhoenixMcpApp {
  id: string
  sequence: number
  callId: string
  resourceUri: string
  toolResult?: unknown
  resource?: { contents?: PhoenixMcpAppResourceContent[] }
  errorKind?: string
}

export interface PhoenixEvent {
  method: string
  params: unknown
  sequence?: number
  received_at_ms?: number
  mcp_apps?: PhoenixMcpApp[]
}

export type PhoenixSnapshotVersion = readonly [maxEventSequence: number, messageCount: number, lastMessageTime: number]

export function phoenixSnapshotVersion(session: Pick<PhoenixSession, 'messages' | 'events'>): PhoenixSnapshotVersion {
  const maxEventSequence = (session.events ?? []).reduce((max, event) => Math.max(max, event.sequence ?? 0), 0)
  const lastMessageTime = session.messages.reduce((max, message) => Math.max(max, message.created_at_ms ?? 0), 0)
  return [maxEventSequence, session.messages.length, lastMessageTime]
}

export function comparePhoenixSnapshotVersions(left: PhoenixSnapshotVersion, right: PhoenixSnapshotVersion): number {
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return left[index] > right[index] ? 1 : -1
  }
  return 0
}

export function mergePhoenixSnapshot(previous: PhoenixSession | undefined, next: PhoenixSession): PhoenixSession {
  if (!previous?.events?.length || !next.events?.length) return next
  const successfulApps = new Map<string, PhoenixMcpApp>()
  for (const event of previous.events) {
    for (const app of event.mcp_apps ?? []) {
      if (app.resource) successfulApps.set(app.id, app)
    }
  }
  if (successfulApps.size === 0) return next
  return {
    ...next,
    events: next.events.map((event) => ({
      ...event,
      mcp_apps: event.mcp_apps?.map((app) => app.resource ? app : successfulApps.get(app.id) ?? app),
    })),
  }
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
