import type { Gateway } from '../types/gateway'

export const GATEWAY_DEGRADED_WARNING_CODES = [
  'unknown_service',
  'service_catalog_unavailable',
  'gateway_discovery_tools_unavailable',
  'gateway_discovery_resources_unavailable',
  'gateway_discovery_prompts_unavailable',
] as const

export type GatewayDegradedWarningCode = typeof GATEWAY_DEGRADED_WARNING_CODES[number]

export function gatewayDegradedWarningCounts(gateways: Gateway[]): Partial<Record<GatewayDegradedWarningCode, number>> {
  const counts: Partial<Record<GatewayDegradedWarningCode, number>> = {}
  for (const gateway of gateways) {
    for (const warning of gateway.warnings) {
      if (!GATEWAY_DEGRADED_WARNING_CODES.includes(warning.code as GatewayDegradedWarningCode)) {
        continue
      }
      const code = warning.code as GatewayDegradedWarningCode
      counts[code] = (counts[code] ?? 0) + 1
    }
  }
  return counts
}

export function hasGatewayDegradedWarnings(counts: Partial<Record<GatewayDegradedWarningCode, number>>): boolean {
  return Object.values(counts).some((count) => (count ?? 0) > 0)
}
