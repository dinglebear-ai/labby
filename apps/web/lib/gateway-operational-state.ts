import type { CapabilityKind, CapabilityObservation, CapabilityObservationValue } from './types/gateway'

export type GatewayOperationalKind =
  | 'disabled'
  | 'idle'
  | 'disconnected'
  | 'discovering'
  | 'degraded'
  | 'healthy'

export type GatewayOperationalInput = {
  enabled?: boolean
  status: {
    connected: boolean
    healthy: boolean
    catalog_warming?: boolean
    last_error?: string
    likely_stale_count?: number
    capability_observation?: Pick<CapabilityObservation, 'scope'> & Partial<Record<CapabilityKind, Pick<CapabilityObservationValue, 'state' | 'error'>>>
  }
  warnings?: ReadonlyArray<{ code: string; message: string }>
}

export type GatewayOperationalState = {
  kind: GatewayOperationalKind
  label: 'Disabled' | 'Disconnected' | 'Discovering' | 'Needs attention' | 'Healthy' | 'Not checked' | 'Idle'
  connectionLabel: 'Disabled' | 'Disconnected' | 'Connected' | 'Not checked' | 'Idle'
  connected: boolean
  needsAttention: boolean
  reason: string
}

function firstWarningMessage(gateway: GatewayOperationalInput): string | undefined {
  return gateway.warnings?.find((warning) => warning.message.trim().length > 0)?.message.trim()
}

function capabilityFailureReason(gateway: GatewayOperationalInput): string | undefined {
  const observation = gateway.status.capability_observation
  if (!observation) return undefined
  for (const kind of ['tools', 'resources', 'prompts', 'skills'] as const) {
    const family = observation[kind]
    if (family?.error?.trim()) return family.error.trim()
    if (family?.state === 'failed') {
      return kind[0].toUpperCase() + kind.slice(1) + ' capability discovery failed; refresh to retry.'
    }
  }
  return undefined
}

function degradedReason(gateway: GatewayOperationalInput): string {
  const warning = firstWarningMessage(gateway)
  if (warning) return warning

  const staleCount = gateway.status.likely_stale_count ?? 0
  if (staleCount > 0) {
    return String(staleCount) + ' likely stale runtime process' + (staleCount === 1 ? '' : 'es') + '.'
  }

  if (gateway.status.last_error?.trim()) return gateway.status.last_error.trim()
  const capabilityFailure = capabilityFailureReason(gateway)
  if (capabilityFailure) return capabilityFailure
  return 'Connected, but one or more health checks need attention.'
}

/**
 * Presentation model for the gateway's two independent dimensions:
 * connectivity and operator health.
 *
 * A connected server may still need attention because capability discovery or
 * runtime cleanup is degraded. Keeping that distinction explicit prevents the
 * UI from treating "connected" and "healthy" as interchangeable states.
 */
export function describeGatewayOperationalState(
  gateway: GatewayOperationalInput,
): GatewayOperationalState {
  const enabled = gateway.enabled !== false
  if (!enabled) {
    return {
      kind: 'disabled',
      label: 'Disabled',
      connectionLabel: 'Disabled',
      connected: false,
      needsAttention: false,
      reason: 'Server is disabled.',
    }
  }

  if (!gateway.status.connected) {
    const observation = gateway.status.capability_observation
    const families = observation ? [observation.tools, observation.resources, observation.prompts, observation.skills] : []
    const hasObservedFailure = gateway.status.last_error?.trim()
      || (gateway.warnings?.length ?? 0) > 0
      || (gateway.status.likely_stale_count ?? 0) > 0
      || capabilityFailureReason(gateway)
    // Credential-scoped status is cache-only: absence or idle expiry is not a
    // failed connection attempt and must not become an outage incident.
    if (observation?.scope === 'credential' && !hasObservedFailure) {
      const label = families.some((family) => family && family.state !== 'unknown') ? 'Idle' : 'Not checked'
      return {
        kind: 'idle', label, connectionLabel: label, connected: false, needsAttention: false,
        reason: label === 'Idle'
          ? 'No active connection for your credentials. Test this server to check its current status.'
          : 'This server has not been checked with your credentials. Test it to check its status.',
      }
    }
    return {
      kind: 'disconnected',
      label: 'Disconnected',
      connectionLabel: 'Disconnected',
      connected: false,
      needsAttention: true,
      reason:
        gateway.status.last_error?.trim()
        || firstWarningMessage(gateway)
        || 'Server is not connected.',
    }
  }

  const hasWarnings = (gateway.warnings?.length ?? 0) > 0
  const hasStaleRuntime = (gateway.status.likely_stale_count ?? 0) > 0
  const hasLastError = Boolean(gateway.status.last_error?.trim())
  const hasCapabilityFailure = Boolean(capabilityFailureReason(gateway))
  const hasNonWarmingHealthFailure =
    !gateway.status.healthy && gateway.status.catalog_warming !== true

  if (hasWarnings || hasStaleRuntime || hasLastError || hasCapabilityFailure || hasNonWarmingHealthFailure) {
    return {
      kind: 'degraded',
      label: 'Needs attention',
      connectionLabel: 'Connected',
      connected: true,
      needsAttention: true,
      reason: degradedReason(gateway),
    }
  }

  if (gateway.status.catalog_warming) {
    return {
      kind: 'discovering',
      label: 'Discovering',
      connectionLabel: 'Connected',
      connected: true,
      needsAttention: false,
      reason: 'Connected; capability discovery is still being populated.',
    }
  }

  return {
    kind: 'healthy',
    label: 'Healthy',
    connectionLabel: 'Connected',
    connected: true,
    needsAttention: false,
    reason: 'Connected and healthy.',
  }
}

export function gatewayNeedsAttention(gateway: GatewayOperationalInput): boolean {
  return describeGatewayOperationalState(gateway).needsAttention
}

/** Calm recovery copy; raw backend diagnostics stay available in the inspector. */
export function gatewayRecoverySummary(state: GatewayOperationalState): string {
  if (!state.needsAttention) return state.reason
  if (/\b401\b|unauthorized|invalid[_ ]token/i.test(state.reason)) {
    return 'Upstream authentication failed. Review the credentials, then test the connection.'
  }
  if (/\b403\b|forbidden/i.test(state.reason)) {
    return 'Upstream access was denied. Review the account permissions, then test the connection.'
  }
  if (/timeout|timed out/i.test(state.reason)) {
    return 'The upstream did not respond in time. Check its availability, then test the connection.'
  }
  return 'The server needs attention. Review its connection settings and technical diagnostics.'
}
