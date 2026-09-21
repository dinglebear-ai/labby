import type { PhoenixEvent } from '@/lib/api/phoenix-client'
import { parseCodeModeTrace } from '@/lib/code-mode-app/trace'

export interface McpAppRenderDescriptor {
  key: string
  resourceUri: string
  itemId: string
  callId?: string
  appName?: string
  toolInput?: Record<string, unknown>
  toolResult?: unknown
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
}

function validUiResourceUri(value: unknown): value is string {
  if (typeof value !== 'string' || !value) return false
  try {
    const parsed = new URL(value)
    return parsed.protocol === 'ui:' && Boolean(parsed.hostname) && !['', '/'].includes(parsed.pathname)
  } catch {
    return false
  }
}

function directResourceUri(item: Record<string, unknown>): string | undefined {
  const appContext = record(item.appContext)
  const resultMeta = record(record(item.result)._meta)
  const modern = record(resultMeta.ui).resourceUri
  for (const candidate of [appContext.resourceUri, item.mcpAppResourceUri, modern, resultMeta['ui/resourceUri']]) {
    if (validUiResourceUri(candidate)) return candidate
  }
}

function toolArguments(value: unknown): Record<string, unknown> {
  return record(record(value).arguments)
}

export function mcpAppsForPhoenixEvent(event: PhoenixEvent): McpAppRenderDescriptor[] {
  if (event.method !== 'item/completed') return []
  const item = record(record(event.params).item)
  if (item.type !== 'mcpToolCall' || item.status !== 'completed' || typeof item.id !== 'string' || !item.id) return []
  const itemId = item.id
  const trace = parseCodeModeTrace(item.result)
  if (trace?.kind === 'code_mode_execute_trace') {
    const seen = new Set<string>()
    return trace.calls.flatMap((call, callIndex) => {
      const resourceUri = call.ui?.resourceUri
      if (!validUiResourceUri(resourceUri)) return []
      const key = `${itemId}:${callIndex}:${call.id}:${resourceUri}`
      if (seen.has(key)) return []
      seen.add(key)
      return [{ key, resourceUri, itemId, callId: call.id, toolInput: record(call.params) }]
    })
  }
  const resourceUri = directResourceUri(item)
  if (!resourceUri) return []
  const appName = typeof record(item.appContext).appName === 'string' ? record(item.appContext).appName as string : undefined
  return [{ key: `${itemId}:${resourceUri}`, resourceUri, itemId, appName, toolInput: toolArguments(item), toolResult: item.result }]
}
