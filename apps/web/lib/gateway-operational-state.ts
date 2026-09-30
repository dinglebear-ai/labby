export type GatewayOperationalKind =
  | 'disabled'
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
  }
  warnings?: ReadonlyArray<{ code: string; message: string }>
}

export type GatewayOperationalState = {
  kind: GatewayOperationalKind
  label: 'Disabled' | 'Disconnected' | 'Discovering' | 'Needs attention' | 'Healthy'
  connectionLabel: 'Disabled' | 'Disconnected' | 'Connected'
  connected: boolean
  needsAttention: boolean
  reason: string
}

function firstWarningMessage(gateway: GatewayOperationalInput): string | undefined {
  return gateway.warnings?.find((warning) => warning.message.trim().length > 0)?.message.trim()
}

function degradedReason(gateway: GatewayOperationalInput): string {
  const warning = firstWarningMessage(gateway)
  if (warning) return warning

  const staleCount = gateway.status.likely_stale_count ?? 0
  if (staleCount > 0) {
    return String(staleCount) + ' likely stale runtime process' + (staleCount === 1 ? '' : 'es') + '.'
  }

  if (gateway.status.last_error?.trim()) return gateway.status.last_error.trim()
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
  const hasNonWarmingHealthFailure =
    !gateway.status.healthy && gateway.status.catalog_warming !== true

  if (hasWarnings || hasStaleRuntime || hasLastError || hasNonWarmingHealthFailure) {
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
