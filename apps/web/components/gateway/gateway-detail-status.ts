export type GatewayDetailTone = 'connected' | 'disconnected' | 'disabled'

export function gatewayDetailStatus({
  enabled,
  connected,
}: {
  enabled: boolean
  connected: boolean
  healthy: boolean
}): { label: string; tone: GatewayDetailTone } {
  if (!enabled) return { label: 'Disabled', tone: 'disabled' }
  if (connected) return { label: 'Connected', tone: 'connected' }
  return { label: 'Disconnected', tone: 'disconnected' }
}
