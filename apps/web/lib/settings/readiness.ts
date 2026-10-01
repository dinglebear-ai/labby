import { setupActionUrl } from '@/lib/api/gateway-config'
import { performServiceAction } from '@/lib/api/service-action-client'
import { SetupApiError } from '@/lib/api/setup-client'

export const READINESS_CHECKS = ['gateway_authenticated', 'agent_provider', 'agent_run', 'selected_clients', 'catalog_search', 'mcp_tool_call'] as const
export type ReadinessCheckName = typeof READINESS_CHECKS[number]
export type ReadinessCheck = {
  check: ReadinessCheckName
  status: 'pending' | 'verified' | 'needs_recheck' | 'deferred'
  verified_at: number | null
  resource_id: string | null
}
export type ReadinessState = { ready: boolean; checks: ReadinessCheck[]; evidence_max_age_seconds: number }

export function parseReadinessState(value: unknown): ReadinessState {
  const state = value as Partial<ReadinessState> | null
  if (!state || typeof state.ready !== 'boolean' || !Array.isArray(state.checks)
    || !Number.isSafeInteger(state.evidence_max_age_seconds) || (state.evidence_max_age_seconds ?? 0) <= 0) {
    throw new Error('Labby returned an invalid first-use status. Refresh to try again.')
  }
  const names = new Set<string>()
  for (const item of state.checks) {
    if (!item || !READINESS_CHECKS.includes(item.check) || names.has(item.check)
      || !['pending', 'verified', 'needs_recheck', 'deferred'].includes(item.status)
      || (item.verified_at !== null && (!Number.isSafeInteger(item.verified_at) || item.verified_at < 0))
      || (item.resource_id !== null && typeof item.resource_id !== 'string')
      || (item.status === 'deferred' && item.check !== 'selected_clients')
      || (item.status === 'verified' && (item.verified_at === null || !item.resource_id?.trim()))) {
      throw new Error('Labby returned invalid first-use evidence. Refresh to try again.')
    }
    names.add(item.check)
  }
  if (names.size !== READINESS_CHECKS.length || state.ready !== state.checks.every((item) => item.status === 'verified' || (item.check === 'selected_clients' && item.status === 'deferred'))) {
    throw new Error('Labby returned incomplete first-use evidence. Refresh to try again.')
  }
  return state as ReadinessState
}

export const readinessApi = {
  async deferClients(signal?: AbortSignal): Promise<ReadinessState> {
    const value = await performServiceAction<unknown, SetupApiError>({
      action: 'readiness.clients.defer', params: {}, signal, serviceLabel: 'Application selection', url: setupActionUrl(),
      createError: (message, status, code, param) => new SetupApiError(message, status, code, param),
    })
    return parseReadinessState(value)
  },
  async state(signal?: AbortSignal): Promise<ReadinessState> {
    const value = await performServiceAction<unknown, SetupApiError>({
      action: 'readiness.state', params: {}, signal, serviceLabel: 'First-use status', url: setupActionUrl(),
      createError: (message, status, code, param) => new SetupApiError(message, status, code, param),
    })
    return parseReadinessState(value)
  },
}
