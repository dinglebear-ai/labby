import type { DiscoveredTool, Gateway } from '@/lib/types/gateway'

export function buildGatewayEndpointPreview(gateway: Gateway): string {
  if (gateway.transport === 'http') {
    return gateway.config.url ?? 'Not configured'
  }

  if (gateway.transport === 'in_process') {
    return `labby mcp --services ${gateway.name}`
  }

  return [gateway.config.command, ...(gateway.config.args ?? [])].filter(Boolean).join(' ')
}

export function filterGatewayTools(tools: DiscoveredTool[], search: string): DiscoveredTool[] {
  const normalizedSearch = search.trim().toLowerCase()
  if (!normalizedSearch) {
    return tools
  }

  return tools.filter((tool) =>
    tool.name.toLowerCase().includes(normalizedSearch) ||
    tool.description?.toLowerCase().includes(normalizedSearch),
  )
}

export function summarizeGatewayTools(tools: DiscoveredTool[]) {
  const exposedCount = tools.filter((tool) => tool.exposed).length

  return {
    exposedCount,
    filteredCount: tools.length - exposedCount,
  }
}
