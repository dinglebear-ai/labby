import { mcpConnectionSchema, type FederatedArtifact } from '@/lib/api/depot-client'
import { gatewayApi } from '@/lib/api/gateway-client'
import { performServiceAction, type ServiceActionError } from '@/lib/api/service-action-client'
import { setupActionUrl } from '@/lib/api/gateway-config'
import type { CreateGatewayInput, Gateway, TestGatewayResult } from '@/lib/types/gateway'

export function supportedMcpConnection(artifact: FederatedArtifact) {
  const parsed = mcpConnectionSchema.safeParse(artifact.mcpConnection)
  const revision = artifact.currentRevisionId ?? artifact.currentRevision?.id
  return parsed.success && parsed.data.revisionId === revision ? parsed.data : undefined
}

export function catalogGatewayInput(artifact: FederatedArtifact, name: string, token: string): CreateGatewayInput {
  const connection = supportedMcpConnection(artifact)
  if (!connection) throw new Error('This revision has no supported MCP connection metadata.')
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/.test(name)) throw new Error('Use 1–64 letters, numbers, hyphens, or underscores for the server name.')
  if (connection.authentication === 'bearer' && (!token.trim() || /[\r\n\0]/.test(token))) throw new Error('Enter a bearer token without line breaks.')
  return {
    name, transport: 'http',
    config: {
      url: connection.url,
      ...(connection.authentication === 'bearer' ? { bearer_token_value: token } : {}),
      // The reviewed first-use scope is tools only. Broader capabilities remain explicit settings.
      proxy_resources: false, proxy_prompts: false, proxy_mcp_ui: false, proxy_skills: false,
    },
  }
}

export async function activateCatalogMcp(input: CreateGatewayInput, onSaved: (gateway: Gateway) => void, api: Pick<typeof gatewayApi, 'create' | 'test'> = gatewayApi): Promise<TestGatewayResult> {
  const gateway = await api.create(input)
  onSaved(gateway)
  // Saving survives a failed probe. Callers retain this identity for retry and must not create again.
  return api.test(gateway.id)
}

export type FirstUseTool = { name: string; description: string; reviewFingerprint: string; inputSchema: { properties?: Record<string, import('@/components/depot/operation-form').OperationProperty>; required?: string[] } }
export async function listFirstUseTools(name: string, expectedUrl: string, signal?: AbortSignal): Promise<FirstUseTool[]> {
  const result = await verificationAction<{ tools: FirstUseTool[] }>('mcp.verification.tools', { name, expected_url: expectedUrl }, signal)
  if (!Array.isArray(result.tools) || result.tools.length > 20 || result.tools.some(tool => typeof tool.name !== 'string' || typeof tool.description !== 'string' || typeof tool.reviewFingerprint !== 'string' || !/^[a-f0-9]{64}$/.test(tool.reviewFingerprint))) throw new Error('Labby returned an invalid verification tool list.')
  return result.tools
}
export async function verifyFirstUseTool(name: string, expectedUrl: string, tool: string, fingerprint: string, arguments_: Record<string, unknown> = {}) {
  const result = await verificationAction<{ verified: boolean; server: string; tool: string }>('mcp.verification.call', { name, expected_url: expectedUrl, tool, expected_fingerprint: fingerprint, arguments: arguments_, approved: true })
  if (result.verified !== true || result.server !== name || result.tool !== tool) throw new Error('The tool call did not return verified first-use evidence.')
  return result
}

function verificationAction<T>(action: string, params: object, signal?: AbortSignal) {
  return performServiceAction<T, ServiceActionError>({
    serviceLabel: 'MCP verification', url: setupActionUrl(), action, params, signal,
    createError: (message, status, code, param) => Object.assign(new Error(message), { status, code, param }),
  })
}
