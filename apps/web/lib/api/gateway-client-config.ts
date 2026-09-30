import type { Gateway } from '../types/gateway.ts'

export interface GatewayClientConfig {
  name: string
  type: 'http' | 'stdio'
  url?: string
  command?: string
  args?: string[]
  env?: Record<string, string>
}

export function buildGatewayClientConfig(gateway: Gateway): GatewayClientConfig {
  if (gateway.transport === 'http' || gateway.transport === 'in_process') {
    return {
      name: gateway.name,
      type: 'http',
      url: gateway.config.url ?? '',
    }
  }

  const env = gateway.config.bearer_token_env
    ? { [gateway.config.bearer_token_env]: `$${gateway.config.bearer_token_env}` }
    : undefined

  return {
    name: gateway.name,
    type: 'stdio',
    command: gateway.config.command ?? '',
    args: gateway.config.args ?? [],
    ...(env ? { env } : {}),
  }
}
