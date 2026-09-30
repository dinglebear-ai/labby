import type { AgentRunResult, AgentView } from '@/lib/agent-tasks/client'
import type { DiscoveryPage } from '@/lib/api/depot-client'
import type { Gateway } from '@/lib/types/gateway'

/** Definitions and HTTP 200 responses alone are not execution evidence. */
export function agentRunVerified(agent: AgentView | null, result: AgentRunResult | null): boolean {
  return Boolean(agent && result && result.status === 'completed' &&
    result.agent_id === agent.agent_id && result.agent_version === agent.version &&
    result.session_id && result.output_digest)
}

/** An imported artifact, disabled server, warming catalog, or hidden tools is not usable MCP. */
export function mcpConnectionVerified(gateway: Gateway | null): boolean {
  return Boolean(gateway && gateway.transport !== 'in_process' && gateway.enabled !== false &&
    gateway.status.connected && gateway.status.healthy && !gateway.status.catalog_warming &&
    gateway.status.exposed_tool_count > 0)
}

export function discoveryVerified(page: DiscoveryPage | null, mockMode = false): boolean {
  return Boolean(!mockMode && page && page.items.length > 0 &&
    page.providerOutcomes.some(outcome => outcome.state === 'participating' || outcome.state === 'exhausted') &&
    page.state !== 'all_failed' && page.state !== 'all_disabled')
}

/** Never interpret a copied client configuration as a successful downstream tool call. */
export function firstUseSummary(provider: boolean, agent: boolean, discovery: boolean, mcp: boolean) {
  const verified = [provider, agent, discovery, mcp].filter(Boolean).length
  return { verified, total: 4, coreReady: verified === 4, toolCallVerified: false as const }
}
