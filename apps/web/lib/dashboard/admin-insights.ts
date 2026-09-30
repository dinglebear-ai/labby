import type { Gateway } from '@/lib/types/gateway'

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
