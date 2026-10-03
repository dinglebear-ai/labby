import { describeGatewayOperationalState, type GatewayOperationalInput } from '@/lib/gateway-operational-state'

export type GatewayDetailTone = 'connected' | 'disconnected' | 'disabled' | 'idle'

export function gatewayDetailStatus({
  enabled,
  warnings,
  ...status
}: {
  enabled: boolean
  warnings?: GatewayOperationalInput['warnings']
} & GatewayOperationalInput['status']): { label: string; tone: GatewayDetailTone } {
  const operational = describeGatewayOperationalState({ enabled, status, warnings })
  return {
    label: operational.connectionLabel,
    tone: operational.connected ? 'connected' : operational.kind === 'idle' ? 'idle' : operational.kind === 'disabled' ? 'disabled' : 'disconnected',
  }
}
