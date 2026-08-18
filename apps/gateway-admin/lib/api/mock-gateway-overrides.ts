import type { ExposurePolicy, Gateway } from '../types/gateway.ts'
import { EXPOSE_NONE_PATTERN } from './tool-exposure-draft.ts'

export interface MockGatewayOverride {
  exposurePolicy?: ExposurePolicy
  proxyResources?: boolean
  config?: Partial<Gateway['config']>
}

const STORAGE_KEY = 'labby.mock.gateway-overrides.v1'

function hasWindowStorage(): boolean {
  return typeof window !== 'undefined' && typeof window.localStorage !== 'undefined'
}

function readOverridesRecord(): Record<string, MockGatewayOverride> {
  if (!hasWindowStorage()) {
    return {}
  }

  try {
    const raw = window.localStorage.getItem(STORAGE_KEY)
    return raw ? (JSON.parse(raw) as Record<string, MockGatewayOverride>) : {}
  } catch {
    return {}
  }
}

function writeOverridesRecord(record: Record<string, MockGatewayOverride>) {
  if (!hasWindowStorage()) {
    return
  }

  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(record))
}

function patternMatchesTool(pattern: string, toolName: string): boolean {
  if (pattern === '*') {
    return true
  }

  if (pattern.endsWith('*')) {
    return toolName.startsWith(pattern.slice(0, -1))
  }

  return pattern === toolName
}

export function getMockGatewayOverride(id: string): MockGatewayOverride | null {
  return readOverridesRecord()[id] ?? null
}

export function setMockGatewayOverride(id: string, override: MockGatewayOverride) {
  const record = readOverridesRecord()
  record[id] = {
    ...record[id],
    ...override,
    config: {
      ...record[id]?.config,
      ...override.config,
    },
  }
  writeOverridesRecord(record)
}

export function applyMockGatewayOverride(
  gateway: Gateway,
  override: MockGatewayOverride | null | undefined,
): Gateway {
  if (!override) {
    return gateway
  }

  const overriddenConfig = { ...gateway.config, ...override.config }
  const exposePatterns = override.exposurePolicy?.mode === 'allowlist'
    ? override.exposurePolicy.patterns
    : []
  const exposeAll = override.exposurePolicy?.mode === 'expose_all'
  const exposeNone = exposePatterns.includes(EXPOSE_NONE_PATTERN)
  const proxyResources = override.proxyResources ?? overriddenConfig.proxy_resources ?? true

  const tools = override.exposurePolicy
    ? gateway.discovery.tools.map((tool) => {
        const matchingPattern = exposePatterns.find((pattern) => patternMatchesTool(pattern, tool.name))
        const exposed = exposeAll ? true : exposeNone ? false : Boolean(matchingPattern)

        return {
          ...tool,
          exposed,
          matched_by: exposed ? (exposeAll ? '*' : matchingPattern ?? tool.name) : null,
        }
      })
    : gateway.discovery.tools

  return {
    ...gateway,
    config: {
      ...overriddenConfig,
      proxy_resources: proxyResources,
      expose_tools:
        override.exposurePolicy?.mode === 'allowlist'
          ? override.exposurePolicy.patterns
          : overriddenConfig.expose_tools,
    },
    status: {
      ...gateway.status,
      exposed_tool_count: tools.filter((tool) => tool.exposed).length,
      exposed_resource_count: proxyResources ? gateway.discovery.resources.length : 0,
    },
    discovery: {
      ...gateway.discovery,
      tools,
    },
  }
}
