import type { Gateway } from '@/lib/types/gateway'

export interface GatewayActivityItem {
  id: string
  gatewayId: string
  gatewayName: string
  kind: 'status' | 'warning'
  tone: 'success' | 'warning' | 'danger'
  title: string
  detail: string
  timestamp: string
}

export interface GatewaySettingsSnapshot {
  authModeLabel: 'Browser session' | 'API token'
  runtimeLabel: 'Live control plane' | 'Mock preview'
  totalGateways: number
  connectedGateways: number
  disconnectedGateways: number
  warningCount: number
  proxyResourceGateways: number
  bearerTokenGateways: number
}

export interface GatewayDocsSnapshot {
  totalGateways: number
  connectedGateways: number
  warningCount: number
  httpGateways: number
  stdioGateways: number
  supportedServices: number
  exposedTools: number
}

interface SettingsOptions {
  hasStandaloneBearerAuth: boolean
  hasMockData: boolean
}

export function buildGatewayActivityFeed(gateways: Gateway[]): GatewayActivityItem[] {
  const parseTimestamp = (value: string) => {
    const parsed = Date.parse(value)
    return Number.isNaN(parsed) ? 0 : parsed
  }

  return gateways
    .flatMap((gateway) => {
      const statusItem: GatewayActivityItem = gateway.status.connected && gateway.status.healthy
        ? {
            id: `${gateway.id}-status`,
            gatewayId: gateway.id,
            gatewayName: gateway.name,
            kind: 'status',
            tone: 'success',
            title: `${gateway.name} is healthy`,
            detail: `Probe completed successfully with ${gateway.status.discovered_tool_count} discovered tools over ${gateway.transport.toUpperCase()}.`,
            timestamp: gateway.updated_at ?? '',
          }
        : {
            id: `${gateway.id}-status`,
            gatewayId: gateway.id,
            gatewayName: gateway.name,
            kind: 'status',
            tone: 'danger',
            title: `${gateway.name} needs attention`,
            detail: gateway.status.last_error || 'Gateway is disconnected or not yet configured.',
            timestamp: gateway.updated_at ?? '',
          }

      const warningItems = gateway.warnings.map<GatewayActivityItem>((warning, index) => ({
        id: `${gateway.id}-warning-${index}`,
        gatewayId: gateway.id,
        gatewayName: gateway.name,
        kind: 'warning',
        tone: 'warning',
        title: `${warning.code} on ${gateway.name}`,
        detail: warning.message,
        timestamp: warning.timestamp,
      }))

      return [statusItem, ...warningItems]
    })
    .sort((left, right) => {
      const timestampDelta = parseTimestamp(right.timestamp) - parseTimestamp(left.timestamp)
      if (timestampDelta !== 0) {
        return timestampDelta
      }

      if (left.kind === right.kind) {
        return left.title.localeCompare(right.title)
      }

      return left.kind === 'status' ? -1 : 1
    })
}

export function buildGatewaySettingsSnapshot(
  gateways: Gateway[],
  options: SettingsOptions,
): GatewaySettingsSnapshot {
  return {
    authModeLabel: options.hasStandaloneBearerAuth ? 'API token' : 'Browser session',
    runtimeLabel: options.hasMockData ? 'Mock preview' : 'Live control plane',
    totalGateways: gateways.length,
    connectedGateways: gateways.filter((gateway) => gateway.status.connected).length,
    disconnectedGateways: gateways.filter((gateway) => !gateway.status.connected).length,
    warningCount: gateways.reduce((count, gateway) => count + gateway.warnings.length, 0),
    proxyResourceGateways: gateways.filter((gateway) => gateway.config.proxy_resources !== false).length,
    bearerTokenGateways: gateways.filter((gateway) => Boolean(gateway.config.bearer_token_env)).length,
  }
}

export function buildGatewayDocsSnapshot(
  gateways: Gateway[],
  supportedServices: number,
): GatewayDocsSnapshot {
  return {
    totalGateways: gateways.length,
    connectedGateways: gateways.filter((gateway) => gateway.status.connected).length,
    warningCount: gateways.reduce((count, gateway) => count + gateway.warnings.length, 0),
    httpGateways: gateways.filter((gateway) => gateway.transport === 'http').length,
    stdioGateways: gateways.filter((gateway) => gateway.transport === 'stdio').length,
    supportedServices,
    exposedTools: gateways.reduce((count, gateway) => count + gateway.status.exposed_tool_count, 0),
  }
}
