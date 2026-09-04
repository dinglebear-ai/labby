'use client'

import useSWR, { mutate } from 'swr'
import { toast } from 'sonner'
import { gatewayApi } from '@/lib/api/gateway-client'
import {
  getMockGatewayFallback,
  getMockGatewaysFallback,
  getMockServiceActionsFallback,
  getMockServiceConfigFallback,
  getMockSupportedServicesFallback,
} from '@/lib/api/mock-fallback'
import { setMockGatewayOverride } from '@/lib/api/mock-gateway-overrides'
import { upstreamMcpGateways } from '@/lib/api/gateway-list-model'
import {
  mockGateways,
  mockReloadResult,
  mockTestResult,
} from '@/lib/api/mock-data'
import { previewExposurePolicy as sharedPreviewExposurePolicy } from '@/lib/api/exposure-policy-matcher'
import type {
  Gateway,
  CreateGatewayInput,
  UpdateGatewayInput,
  ExposurePolicy,
  TestGatewayResult,
  ReloadGatewayResult,
  GatewayCleanupResult,
  ExposurePolicyPreview,
  GatewayLoadout,
  GatewayLoadoutInput,
  GatewayLoadoutPatch,
  GatewayLoadoutStageResult,
  ServiceConfig,
  ServiceAction,
  SupportedService,
  CodeModeConfig,
  CodeModeConfigInput,
  ProtectedMcpRoute,
  ProtectedMcpRouteInput,
  ProtectedMcpRouteStageResult,
  ProtectedMcpRouteTestResult,
  DiscoveredMcpServer,
  GatewayImportResult,
} from '@/lib/types/gateway'
import { useCallback, useEffect } from 'react'
import { loadGatewayConfiguration, loadGatewayRuntime } from '@/lib/api/gateway-progressive'
import { withRequestTiming } from '@/lib/api/request-timing'

// Set NEXT_PUBLIC_MOCK_DATA=true to use mock data for development
const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'
const DEFAULT_CODE_MODE_CONFIG: CodeModeConfig = {
  enabled: false,
  timeout_ms: 5000,
  max_tool_calls: 8,
  max_response_bytes: 24 * 1024,
  max_response_tokens: 6000,
}
let mockCodeModeConfig: CodeModeConfig = DEFAULT_CODE_MODE_CONFIG
let mockLoadouts: GatewayLoadout[] = [
  {
    name: 'operations',
    description: 'Operations-focused gateway projection',
    upstreams: [],
    services: [],
    expose_code_mode: true,
    expose_tools: false,
    expose_resources: true,
    expose_prompts: true,
    expose_skills: true,
  },
]
let mockProtectedRoutes: ProtectedMcpRoute[] = [
  {
    name: 'tools',
    enabled: true,
    public_host: 'mcp.example.net',
    public_path: '/tools',
    upstream: null,
    backend_url: 'http://localhost:3100/mcp',
    scopes: ['mcp:read'],
    health_path: '/health',
  },
]


let mockRuntimeLoadouts: GatewayLoadout[] = mockLoadouts.map((loadout) => ({
  ...loadout,
  upstreams: [...loadout.upstreams],
  services: [...loadout.services],
}))
function cloneMockProtectedRoutes(routes: ProtectedMcpRoute[]): ProtectedMcpRoute[] {
  return routes.map((route) => ({
    ...route,
    scopes: [...route.scopes],
    target: route.target ? { ...route.target } : route.target,
  }))
}

let mockRuntimeProtectedRoutes: ProtectedMcpRoute[] = cloneMockProtectedRoutes(mockProtectedRoutes)

function sameMockLoadout(left: GatewayLoadout | undefined, right: GatewayLoadout): boolean {
  if (!left) return false
  return left.name === right.name
    && left.description === right.description
    && JSON.stringify(left.upstreams) === JSON.stringify(right.upstreams)
    && JSON.stringify(left.services) === JSON.stringify(right.services)
    && left.expose_tools === right.expose_tools
    && left.expose_resources === right.expose_resources
    && left.expose_prompts === right.expose_prompts
    && left.expose_skills === right.expose_skills
    && left.expose_code_mode === right.expose_code_mode
}

function sameMockProtectedRoute(left: ProtectedMcpRoute | undefined, right: ProtectedMcpRoute): boolean {
  if (!left) return false
  return left.name === right.name
    && left.enabled === right.enabled
    && left.public_host === right.public_host
    && left.public_path === right.public_path
    && left.upstream === right.upstream
    && left.backend_url === right.backend_url
    && left.backend_mcp_path === right.backend_mcp_path
    && JSON.stringify(left.scopes) === JSON.stringify(right.scopes)
    && left.health_path === right.health_path
    && JSON.stringify(left.target ?? null) === JSON.stringify(right.target ?? null)
}


function mockRouteUsesLoadout(route: ProtectedMcpRoute | undefined, loadout: string): boolean {
  return Boolean(
    route?.enabled
    && route.target?.kind === 'gateway_subset'
    && route.target.loadout === loadout
  )
}


function mockLoadoutHasEnabledRoute(name: string): boolean {
  return mockProtectedRoutes.some((route) => mockRouteUsesLoadout(route, name))
    || mockRuntimeProtectedRoutes.some((route) => mockRouteUsesLoadout(route, name))
}

function mockLoadoutBlockingRemoveReference(name: string): ProtectedMcpRoute | undefined {
  return mockProtectedRoutes.find((route) =>
    route.target?.kind === 'gateway_subset' && route.target.loadout === name
  ) ?? mockRuntimeProtectedRoutes.find((route) =>
    route.enabled
    && route.target?.kind === 'gateway_subset'
    && route.target.loadout === name
  )
}

function mockRouteIsSubset(route: ProtectedMcpRoute | undefined): boolean {
  return route?.target?.kind === 'gateway_subset'
}

function mockProtectedRoutesHaveRestartDebt(): boolean {
  for (const runtime of mockRuntimeProtectedRoutes) {
    const desired = mockProtectedRoutes.find((route) => route.name === runtime.name)
    if (!desired) {
      if (mockRouteIsSubset(runtime)) return true
      continue
    }
    if (!sameMockProtectedRoute(runtime, desired)
      && (mockRouteIsSubset(runtime) || mockRouteIsSubset(desired))) {
      return true
    }
  }
  return mockProtectedRoutes.some((desired) =>
    mockRouteIsSubset(desired)
    && !mockRuntimeProtectedRoutes.some((runtime) => runtime.name === desired.name)
  )
}

function mockLoadoutStateRows(): GatewayLoadout[] {
  const names = [...new Set([
    ...mockLoadouts.map((loadout) => loadout.name),
    ...mockRuntimeLoadouts.map((loadout) => loadout.name),
  ])].sort((left, right) => left.localeCompare(right))

  return names.map((name) => {
    const desired = mockLoadouts.find((loadout) => loadout.name === name)
    const runtime = mockRuntimeLoadouts.find((loadout) => loadout.name === name)
    const changed = desired && runtime ? !sameMockLoadout(runtime, desired) : Boolean(desired) !== Boolean(runtime)
    const routeRelated = mockProtectedRoutes.some((route) => mockRouteUsesLoadout(route, name))
      || mockRuntimeProtectedRoutes.some((route) => mockRouteUsesLoadout(route, name))
    const restartRequired = changed && routeRelated
    const pendingOperation = !restartRequired
      ? null
      : runtime == null
        ? 'add'
        : desired == null
          ? 'remove'
          : 'update'
    const display = desired ?? runtime
    if (!display) throw new Error('Mock Loadout state lost both desired and runtime rows')
    return {
      ...display,
      restart_required: restartRequired,
      pending_operation: pendingOperation,
      runtime_present: Boolean(runtime),
      desired_present: Boolean(desired),
    }
  })
}

function mockProtectedRouteStateRows(): ProtectedMcpRoute[] {
  const names = [...new Set([
    ...mockProtectedRoutes.map((route) => route.name),
    ...mockRuntimeProtectedRoutes.map((route) => route.name),
  ])].sort((left, right) => left.localeCompare(right))

  const globalRestartRequired = mockProtectedRoutesHaveRestartDebt()
  return names.map((name) => {
    const desired = mockProtectedRoutes.find((route) => route.name === name)
    const runtime = mockRuntimeProtectedRoutes.find((route) => route.name === name)
    const changed = desired && runtime
      ? !sameMockProtectedRoute(runtime, desired)
      : Boolean(desired) !== Boolean(runtime)
    const restartRequired = changed && globalRestartRequired
    const pendingOperation = !restartRequired
      ? null
      : runtime == null
        ? 'add'
        : desired == null
          ? 'remove'
          : 'update'
    const display = desired ?? runtime
    if (!display) throw new Error('Mock protected-route state lost both desired and runtime rows')
    return {
      ...display,
      restart_required: restartRequired,
      pending_operation: pendingOperation,
      runtime_present: Boolean(runtime),
      desired_present: Boolean(desired),
    }
  })
}

// Simulate network delay for mock data
const mockDelay = (ms: number = 500) => new Promise(resolve => setTimeout(resolve, ms))

function abortableMockDelay(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new DOMException('Aborted', 'AbortError'))
      return
    }

    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort)
      resolve()
    }, ms)

    const onAbort = () => {
      clearTimeout(timer)
      reject(new DOMException('Aborted', 'AbortError'))
    }

    signal?.addEventListener('abort', onAbort, { once: true })
  })
}

// Fetcher functions that handle mock/real data
const fetchGateways = async (): Promise<Gateway[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return upstreamMcpGateways(getMockGatewaysFallback())
  }

  return withRequestTiming('gateway.list', async () =>
    upstreamMcpGateways(await loadGatewayConfiguration(gatewayApi)),
  )
}

const hydrateGatewayRuntime = async (gateways: Gateway[]): Promise<Gateway[]> =>
  withRequestTiming('gateway.runtime', async () =>
    upstreamMcpGateways(await loadGatewayRuntime(gatewayApi, gateways)),
  )

const fetchGateway = async (id: string): Promise<Gateway> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    const gateway = getMockGatewayFallback(id)
    if (!gateway) throw new Error('Gateway not found')
    return gateway
  }
  return gatewayApi.get(id)
}

const fetchExposurePolicy = async (id: string): Promise<ExposurePolicy> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    const gateway = mockGateways.find(g => g.id === id)
    if (!gateway) throw new Error('Gateway not found')
    return {
      mode: gateway.config.expose_tools ? 'allowlist' : 'expose_all',
      patterns: gateway.config.expose_tools || [],
    }
  }
  return gatewayApi.getExposurePolicy(id)
}

const fetchSupportedServices = async (): Promise<SupportedService[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return getMockSupportedServicesFallback()
  }
  return gatewayApi.supportedServices()
}

const fetchServiceConfig = async (service: string): Promise<ServiceConfig> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return getMockServiceConfigFallback(service)
  }
  return gatewayApi.getServiceConfig(service)
}

const fetchServiceActions = async (service: string): Promise<ServiceAction[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return getMockServiceActionsFallback(service)
  }
  return gatewayApi.serviceActions(service)
}

const fetchCodeModeConfig = async (): Promise<CodeModeConfig> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return mockCodeModeConfig
  }
  return gatewayApi.getCodeModeConfig()
}

const fetchLoadouts = async (): Promise<GatewayLoadout[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return mockLoadoutStateRows()
  }
  return gatewayApi.listLoadouts()
}

const fetchProtectedRoutes = async (): Promise<ProtectedMcpRoute[]> => {
  if (USE_MOCK_DATA) {
    await mockDelay()
    return mockProtectedRouteStateRows()
  }
  return gatewayApi.listProtectedRoutes()
}

// SWR Keys
export const GATEWAYS_KEY = '/gateways'
export const gatewayKey = (id: string) => `/gateways/${id}`
export const exposurePolicyKey = (id: string) => `/gateways/${id}/exposure`
export const SUPPORTED_SERVICES_KEY = '/gateway-supported-services'
export const serviceConfigKey = (service: string) => `/gateway-service-config/${service}`
export const serviceActionsKey = (service: string) => `/gateway-service-actions/${service}`
export const CODE_MODE_CONFIG_KEY = '/gateway-code-mode-config'
export const LOADOUTS_KEY = '/gateway-loadouts'
export const PROTECTED_MCP_ROUTES_KEY = '/gateway-protected-mcp-routes'

export function gatewaysRequestKey(enabled: boolean): string | null {
  return enabled ? GATEWAYS_KEY : null
}

export function gatewaysRuntimeRequestKey(
  enabled: boolean,
  includeRuntime: boolean,
  gateways: Gateway[] | undefined,
): [string, string] | null {
  return enabled && includeRuntime && gateways
    ? ['/gateways/runtime', gateways.map((gateway) => gateway.id).join(',')]
    : null
}

async function refreshGatewayCache(id?: string, extraKeys: string[] = []) {
  const keys = [GATEWAYS_KEY, ...(id ? [gatewayKey(id)] : []), ...extraKeys]
  await Promise.all(keys.map((key) => mutate(key)))
}

// Hooks
export function useGatewaySnapshots(enabled = true) {
  return useSWR<Gateway[]>(gatewaysRequestKey(enabled), fetchGateways, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? getMockGatewaysFallback() : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

export function useGateways(enabled = true) {
  const configured = useGatewaySnapshots(enabled)
  const catalogWarm = useSWR(
    enabled && !USE_MOCK_DATA ? '/gateway-catalog-warm' : null,
    () => gatewayApi.refreshStatus(),
    { revalidateOnFocus: false, shouldRetryOnError: true, errorRetryCount: 2, errorRetryInterval: 5_000 },
  )
  const runtimeKey = !USE_MOCK_DATA
    ? gatewaysRuntimeRequestKey(enabled, true, configured.data)
    : null
  const runtimeCacheId = runtimeKey?.[1]
  const runtime = useSWR<Gateway[]>(
    runtimeKey,
    () => hydrateGatewayRuntime(configured.data ?? []),
    { revalidateOnFocus: false, shouldRetryOnError: false },
  )
  const catalogIsStillWarming = (runtime.data ?? configured.data ?? []).some((gateway) => gateway.status?.catalog_warming)

  useEffect(() => {
    if (!enabled || catalogWarm.isLoading || catalogWarm.error) return
    const refreshCatalogView = () => {
      void mutate(GATEWAYS_KEY)
      if (runtimeCacheId) void mutate(['/gateways/runtime', runtimeCacheId])
    }
    refreshCatalogView()
    const interval = window.setInterval(refreshCatalogView, 5_000)
    const stop = window.setTimeout(() => {
      if (catalogIsStillWarming) {
        toast.warning('Tool catalog discovery is still running; updates will continue in the background.')
      } else {
        window.clearInterval(interval)
      }
    }, 60_000)
    return () => {
      window.clearInterval(interval)
      window.clearTimeout(stop)
    }
  }, [catalogIsStillWarming, catalogWarm.error, catalogWarm.isLoading, enabled, runtimeCacheId])

  return {
    ...configured,
    data: runtime.data ?? configured.data,
    error: configured.error,
    runtimeError: runtime.error,
    catalogWarmError: catalogWarm.error,
    retryCatalogWarm: catalogWarm.mutate,
    isLoading: configured.isLoading,
    isValidating: configured.isValidating || runtime.isValidating,
  }
}

export function useGateway(id: string | null) {
  const fallbackGateway = USE_MOCK_DATA && id ? getMockGatewayFallback(id) : undefined

  return useSWR<Gateway>(
    id ? gatewayKey(id) : null,
    id ? () => fetchGateway(id) : null,
    {
      revalidateOnFocus: false,
      fallbackData: fallbackGateway,
      revalidateOnMount: !USE_MOCK_DATA || fallbackGateway === undefined,
    }
  )
}

export function useExposurePolicy(id: string | null) {
  return useSWR<ExposurePolicy>(
    id ? exposurePolicyKey(id) : null,
    id ? () => fetchExposurePolicy(id) : null,
    {
      revalidateOnFocus: false,
    }
  )
}

export function useSupportedServices() {
  return useSWR<SupportedService[]>(SUPPORTED_SERVICES_KEY, fetchSupportedServices, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? getMockSupportedServicesFallback() : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

export function useServiceConfig(service: string | null) {
  return useSWR<ServiceConfig>(
    service ? serviceConfigKey(service) : null,
    service ? () => fetchServiceConfig(service) : null,
    {
      revalidateOnFocus: false,
      fallbackData: USE_MOCK_DATA && service ? getMockServiceConfigFallback(service) : undefined,
      revalidateOnMount: !USE_MOCK_DATA,
    }
  )
}

export function useServiceActions(service: string | null) {
  return useSWR<ServiceAction[]>(
    service ? serviceActionsKey(service) : null,
    service ? () => fetchServiceActions(service) : null,
    {
      revalidateOnFocus: false,
      fallbackData: USE_MOCK_DATA && service ? getMockServiceActionsFallback(service) : undefined,
      revalidateOnMount: !USE_MOCK_DATA,
    }
  )
}

export function useGatewayCodeModeConfig() {
  return useSWR<CodeModeConfig>(CODE_MODE_CONFIG_KEY, fetchCodeModeConfig, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? DEFAULT_CODE_MODE_CONFIG : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

export function useLoadouts() {
  return useSWR<GatewayLoadout[]>(LOADOUTS_KEY, fetchLoadouts, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? mockLoadouts : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

export function useProtectedMcpRoutes() {
  return useSWR<ProtectedMcpRoute[]>(PROTECTED_MCP_ROUTES_KEY, fetchProtectedRoutes, {
    revalidateOnFocus: false,
    fallbackData: USE_MOCK_DATA ? mockProtectedRoutes : undefined,
    revalidateOnMount: !USE_MOCK_DATA,
  })
}

// Mutation hooks
export function useGatewayMutations() {
  const createGateway = useCallback(async (input: CreateGatewayInput): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const newGateway: Gateway = {
        id: `gw-${Date.now()}`,
        name: input.name,
        transport: input.transport,
        config: input.config,
        status: {
          healthy: false,
          connected: false,
          discovered_tool_count: 0,
          exposed_tool_count: 0,
          discovered_resource_count: 0,
          exposed_resource_count: 0,
          discovered_prompt_count: 0,
          exposed_prompt_count: 0,
        },
        discovery: { tools: [], resources: [], prompts: [] },
        warnings: [],
        // created_at / updated_at come from the backend; omit in mock paths.
      }
      await mutate(GATEWAYS_KEY, (current: Gateway[] = []) => [...current, newGateway], false)
      return newGateway
    }
    const gateway = await gatewayApi.create(input)
    await refreshGatewayCache(gateway.id)
    return gateway
  }, [])

  const discoverExternalConfigs = useCallback(async (): Promise<DiscoveredMcpServer[]> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return [
        {
          name: 'local-files',
          source_client: 'claude-code',
          source_path: '~/.claude/settings.json',
          transport: 'stdio',
          command_preview: 'npx',
          env_key_count: 1,
          already_configured: false,
          tombstoned: false,
        },
      ]
    }
    return gatewayApi.discoverExternalConfigs()
  }, [])

  const importExternalConfigs = useCallback(async (names?: string[]): Promise<GatewayImportResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return {
        imported: (names && names.length > 0 ? names : ['local-files']).map((name) => ({
          config: { name, enabled: false },
        })),
        skipped: [],
        errors: [],
      }
    }
    const result = await gatewayApi.importExternalConfigs(names)
    await refreshGatewayCache()
    return result
  }, [])

  const clearImportTombstone = useCallback(async (server: DiscoveredMcpServer): Promise<void> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return
    }
    await gatewayApi.clearImportTombstone(server)
  }, [])

  const restoreImportTombstone = useCallback(async (server: DiscoveredMcpServer): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return {
        id: server.name,
        name: server.name,
        transport: server.transport,
        source: 'custom_gateway',
        configured: true,
        enabled: false,
        config: {},
        status: {
          healthy: false,
          connected: false,
          discovered_tool_count: 0,
          exposed_tool_count: 0,
          discovered_resource_count: 0,
          exposed_resource_count: 0,
          discovered_prompt_count: 0,
          exposed_prompt_count: 0,
        },
        discovery: { tools: [], resources: [], prompts: [] },
        warnings: [],
        // created_at / updated_at come from the backend; omit in mock paths.
      }
    }
    const gateway = await gatewayApi.restoreImportTombstone(server)
    await refreshGatewayCache(gateway.id)
    return gateway
  }, [])

  const updateGateway = useCallback(async (id: string, input: UpdateGatewayInput): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find(g => g.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const updated = {
        ...gateway,
        ...input,
        config: {
          ...gateway.config,
          ...input.config,
        },
        // updated_at comes from the backend; omit in mock paths.
      }
      if (input.config) {
        setMockGatewayOverride(id, {
          config: input.config,
          proxyResources: input.config.proxy_resources,
        })
      }
      await mutate(gatewayKey(id), updated, false)
      await mutate(GATEWAYS_KEY)
      return updated
    }
    const gateway = await gatewayApi.update(id, input)
    await refreshGatewayCache(id)
    return gateway
  }, [])

  const removeGateway = useCallback(async (id: string): Promise<void> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      await mutate(GATEWAYS_KEY, (current: Gateway[] = []) => current.filter(g => g.id !== id), false)
      return
    }
    await gatewayApi.remove(id)
    await refreshGatewayCache()
  }, [])

  const removeVirtualServer = useCallback(async (id: string): Promise<void> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      await mutate(GATEWAYS_KEY, (current: Gateway[] = []) => current.filter(g => g.id !== id), false)
      return
    }
    await gatewayApi.removeVirtualServer(id)
    await refreshGatewayCache()
  }, [])

  const testGateway = useCallback(async (id: string, signal?: AbortSignal): Promise<TestGatewayResult> => {
    if (USE_MOCK_DATA) {
      await abortableMockDelay(1500, signal) // Longer delay for test
      const gateway = mockGateways.find(g => g.id === id)
      if (!gateway) throw new Error('Gateway not found')
      if (!gateway.status.healthy) {
        return {
          success: false,
          message: 'Connection failed',
          error: gateway.status.last_error,
        }
      }
      return mockTestResult
    }
    return await gatewayApi.test(id, signal)
  }, [])

  const reloadGateway = useCallback(async (id: string): Promise<ReloadGatewayResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay(2000) // Longer delay for reload
      return mockReloadResult
    }
    const result = await gatewayApi.reload(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const setExposurePolicy = useCallback(async (id: string, policy: ExposurePolicy): Promise<ExposurePolicy> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      setMockGatewayOverride(id, { exposurePolicy: policy })
      const updatedGateway = getMockGatewayFallback(id)
      if (!updatedGateway) {
        throw new Error('Gateway not found')
      }

      await mutate(
        gatewayKey(id),
        updatedGateway,
        false,
      )
      await mutate(
        GATEWAYS_KEY,
        (current: Gateway[] = []) =>
          current.map((gateway) => (gateway.id === id ? updatedGateway : gateway)),
        false,
      )
      await mutate(exposurePolicyKey(id), policy, false)
      return policy
    }
    const result = await gatewayApi.setExposurePolicy(id, policy)
    await refreshGatewayCache(id, [exposurePolicyKey(id)])
    return result
  }, [])

  const previewExposurePolicy = useCallback(async (id: string, patterns: string[], signal?: AbortSignal): Promise<ExposurePolicyPreview> => {
    if (USE_MOCK_DATA) {
      await abortableMockDelay(300, signal)
      const gateway = mockGateways.find(g => g.id === id)
      if (!gateway) throw new Error('Gateway not found')

      // Use the shared pure matcher so mock and real preview paths are
      // semantically identical (lab-2oec.7).
      const toolNames = gateway.discovery.tools.map((t) => t.name)
      return sharedPreviewExposurePolicy(toolNames, patterns)
    }
    return gatewayApi.previewExposurePolicy(id, patterns, signal)
  }, [])

  const saveServiceConfig = useCallback(async (service: string, values: Record<string, string>): Promise<ServiceConfig> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const fields = Object.entries(values).map(([name, value]) => ({
        name,
        present: value.length > 0,
        secret: name.includes('TOKEN') || name.includes('KEY') || name.includes('PASSWORD'),
        value_preview: name.includes('TOKEN') || name.includes('KEY') || name.includes('PASSWORD') ? null : value,
      }))
      const result = { service, configured: fields.length > 0, fields }
      await mutate(serviceConfigKey(service), result, false)
      return result
    }
    const result = await gatewayApi.setServiceConfig(service, values)
    await refreshGatewayCache(undefined, [serviceConfigKey(service)])
    return result
  }, [])

  const setCodeModeConfig = useCallback(async (input: CodeModeConfigInput): Promise<CodeModeConfig> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      mockCodeModeConfig = {
        ...mockCodeModeConfig,
        ...input,
      }
      await mutate(CODE_MODE_CONFIG_KEY, mockCodeModeConfig, false)
      return mockCodeModeConfig
    }
    const result = await gatewayApi.setCodeModeConfig(input)
    await mutate(CODE_MODE_CONFIG_KEY, result, false)
    await mutate(GATEWAYS_KEY)
    return result
  }, [])

  const addLoadout = useCallback(async (loadout: GatewayLoadoutInput): Promise<GatewayLoadout> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      if (mockLoadouts.some((item) => item.name === loadout.name)) {
        throw new Error(`Loadout ${loadout.name} already exists`)
      }
      mockLoadouts = [...mockLoadouts, loadout]
      mockRuntimeLoadouts = [...mockRuntimeLoadouts, loadout]
      await mutate(LOADOUTS_KEY, mockLoadoutStateRows(), false)
      return loadout
    }
    const result = await gatewayApi.addLoadout(loadout)
    await mutate(LOADOUTS_KEY)
    return result
  }, [])

  const patchLoadout = useCallback(
    async (name: string, patch: GatewayLoadoutPatch): Promise<GatewayLoadout> => {
      if (USE_MOCK_DATA) {
        await mockDelay()
        const current = mockLoadouts.find((item) => item.name === name)
        if (!current) throw new Error(`Loadout ${name} not found`)
        if (mockLoadoutHasEnabledRoute(name)) {
          throw new Error(`Loadout ${name} is mounted by an enabled protected route; stage the update for restart`)
        }
        const updated = { ...current, ...patch, name: patch.name ?? current.name }
        if (updated.name !== name && mockLoadouts.some((item) => item.name === updated.name)) {
          throw new Error(`Loadout ${updated.name} already exists`)
        }
        mockLoadouts = mockLoadouts.map((item) => (item.name === name ? updated : item))
        mockRuntimeLoadouts = mockRuntimeLoadouts.map((item) => (item.name === name ? updated : item))
        if (updated.name !== name) {
          const renameTarget = (route: ProtectedMcpRoute): ProtectedMcpRoute =>
            route.target?.kind === 'gateway_subset' && route.target.loadout === name
              ? { ...route, target: { ...route.target, loadout: updated.name } }
              : route
          mockProtectedRoutes = mockProtectedRoutes.map(renameTarget)
          mockRuntimeProtectedRoutes = mockRuntimeProtectedRoutes.map(renameTarget)
          await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        }
        await mutate(LOADOUTS_KEY, mockLoadoutStateRows(), false)
        return updated
      }
      const result = await gatewayApi.patchLoadout(name, patch)
      await mutate(LOADOUTS_KEY)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const stageLoadoutUpdate = useCallback(
    async (name: string, loadout: GatewayLoadoutInput): Promise<GatewayLoadoutStageResult> => {
      if (USE_MOCK_DATA) {
        await mockDelay()
        const current = mockLoadouts.find((item) => item.name === name)
        if (!current) throw new Error(`Loadout ${name} not found`)
        if (loadout.name !== name && mockLoadouts.some((item) => item.name === loadout.name)) {
          throw new Error(`Loadout ${loadout.name} already exists`)
        }
        const runtime = mockRuntimeLoadouts.find((item) => item.name === loadout.name)
        const restartRequired = !sameMockLoadout(runtime, loadout)
        const staged: GatewayLoadout = {
          ...loadout,
          restart_required: restartRequired,
          pending_operation: restartRequired ? (runtime ? 'update' : 'add') : null,
          runtime_present: Boolean(runtime),
          desired_present: true,
        }
        mockLoadouts = mockLoadouts.map((item) => item.name === name ? staged : item)
        if (loadout.name !== name) {
          mockProtectedRoutes = mockProtectedRoutes.map((route) => {
            if (route.desired_present === false || route.target?.kind !== 'gateway_subset' || route.target.loadout !== name) {
              return route
            }
            return {
              ...route,
              target: { ...route.target, loadout: loadout.name },
              restart_required: true,
              pending_operation: route.runtime_present === false ? 'add' : 'update',
            }
          })
          await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        }
        const view = mockLoadoutStateRows().find((item) => item.name === loadout.name)
        if (!view) throw new Error(`Loadout ${loadout.name} disappeared from mock desired/runtime state`)
        await mutate(LOADOUTS_KEY, mockLoadoutStateRows(), false)
        return {
          loadout: view,
          restart_required: view.restart_required ?? false,
          pending_operation: view.pending_operation ?? null,
          restart_note: view.restart_required
            ? 'Saved for the next Labby restart.'
            : 'Desired Loadout state matches the running projection; no restart is required.',
        }
      }
      const result = await gatewayApi.stageLoadoutUpdate(name, loadout)
      await mutate(LOADOUTS_KEY)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const stageLoadoutRemove = useCallback(async (name: string): Promise<GatewayLoadoutStageResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const current = mockLoadouts.find((item) => item.name === name)
      if (!current) throw new Error('Loadout ' + name + ' not found')
      const desiredReference = mockProtectedRoutes.find((route) =>
        route.desired_present !== false
        && route.target?.kind === 'gateway_subset'
        && route.target.loadout === name
      )
      if (desiredReference) {
        throw new Error('Loadout ' + name + ' is still referenced by protected route ' + desiredReference.name + '; stage that route away from the Loadout first')
      }
      const runtime = mockRuntimeLoadouts.find((item) => item.name === name)
      if (!runtime) {
        mockLoadouts = mockLoadouts.filter((item) => item.name !== name)
        await mutate(LOADOUTS_KEY, mockLoadoutStateRows(), false)
        return {
          loadout: current,
          restart_required: false,
          pending_operation: null,
          restart_note: 'Desired Loadout state matches the running projection; no restart is required.',
        }
      }
      mockLoadouts = mockLoadouts.filter((item) => item.name !== name)
      const view = mockLoadoutStateRows().find((item) => item.name === name)
      if (!view) {
        await mutate(LOADOUTS_KEY, mockLoadoutStateRows(), false)
        return {
          loadout: current,
          restart_required: false,
          pending_operation: null,
          restart_note: 'Desired Loadout state matches the running projection; no restart is required.',
        }
      }
      await mutate(LOADOUTS_KEY, mockLoadoutStateRows(), false)
      return {
        loadout: view,
        restart_required: view.restart_required ?? false,
        pending_operation: view.pending_operation ?? null,
        restart_note: view.restart_required
          ? 'Saved for the next Labby restart.'
          : 'Desired Loadout state matches the running projection; no restart is required.',
      }
    }
    const result = await gatewayApi.stageLoadoutRemove(name)
    await mutate(LOADOUTS_KEY)
    return result
  }, [])

  const removeLoadout = useCallback(async (name: string): Promise<GatewayLoadout> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const removed = mockLoadouts.find((item) => item.name === name)
      if (!removed) throw new Error(`Loadout ${name} not found`)
      const referencedBy = mockLoadoutBlockingRemoveReference(name)
      if (referencedBy) {
        throw new Error(`Loadout ${name} is still referenced by protected route ${referencedBy.name}; update or remove that route first`)
      }
      mockLoadouts = mockLoadouts.filter((item) => item.name !== name)
      mockRuntimeLoadouts = mockRuntimeLoadouts.filter((item) => item.name !== name)
      await mutate(LOADOUTS_KEY, mockLoadoutStateRows(), false)
      return removed
    }
    const result = await gatewayApi.removeLoadout(name)
    await mutate(LOADOUTS_KEY)
    return result
  }, [])

  const stageProtectedRouteAdd = useCallback(
    async (route: ProtectedMcpRouteInput, signal?: AbortSignal): Promise<ProtectedMcpRouteStageResult> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        const runtime = mockRuntimeProtectedRoutes.find((item) => item.name === route.name)
        const localChanged = !sameMockProtectedRoute(runtime, route)
        const staged: ProtectedMcpRoute = {
          ...route,
          restart_required: localChanged,
          pending_operation: localChanged ? (runtime ? 'update' : 'add') : null,
          runtime_present: Boolean(runtime),
          desired_present: true,
        }
        mockProtectedRoutes = [...mockProtectedRoutes.filter((item) => item.name !== route.name), staged]
        const view = mockProtectedRouteStateRows().find((item) => item.name === route.name)
        if (!view) throw new Error(`Protected route ${route.name} disappeared from mock desired/runtime state`)
        const restartRequired = mockProtectedRoutesHaveRestartDebt()
        if (!restartRequired) mockRuntimeProtectedRoutes = cloneMockProtectedRoutes(mockProtectedRoutes)
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        return {
          route: view,
          restart_required: restartRequired,
          pending_operation: view.pending_operation ?? null,
          restart_note: restartRequired
            ? 'Saved for the next Labby restart.'
            : 'Desired route state matches the running route set; no restart is required.',
        }
      }
      const result = await gatewayApi.stageProtectedRouteAdd(route, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const stageProtectedRouteUpdate = useCallback(
    async (name: string, route: ProtectedMcpRouteInput, signal?: AbortSignal): Promise<ProtectedMcpRouteStageResult> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        const runtime = mockRuntimeProtectedRoutes.find((item) => item.name === route.name)
        const localChanged = !sameMockProtectedRoute(runtime, route)
        const staged: ProtectedMcpRoute = {
          ...route,
          restart_required: localChanged,
          pending_operation: localChanged ? (runtime ? 'update' : 'add') : null,
          runtime_present: Boolean(runtime),
          desired_present: true,
        }
        mockProtectedRoutes = mockProtectedRoutes.map((item) => item.name === name ? staged : item)
        const view = mockProtectedRouteStateRows().find((item) => item.name === route.name)
        if (!view) throw new Error(`Protected route ${route.name} disappeared from mock desired/runtime state`)
        const restartRequired = mockProtectedRoutesHaveRestartDebt()
        if (!restartRequired) mockRuntimeProtectedRoutes = cloneMockProtectedRoutes(mockProtectedRoutes)
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        return {
          route: view,
          restart_required: restartRequired,
          pending_operation: view.pending_operation ?? null,
          restart_note: restartRequired
            ? 'Saved for the next Labby restart.'
            : 'Desired route state matches the running route set; no restart is required.',
        }
      }
      const result = await gatewayApi.stageProtectedRouteUpdate(name, route, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const stageProtectedRouteRemove = useCallback(
    async (name: string, signal?: AbortSignal): Promise<ProtectedMcpRouteStageResult> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        const existing = mockProtectedRoutes.find((item) => item.name === name)
        if (!existing) throw new Error('Protected route ' + name + ' not found')
        const runtime = mockRuntimeProtectedRoutes.find((item) => item.name === name)
        if (!runtime) {
          mockProtectedRoutes = mockProtectedRoutes.filter((item) => item.name !== name)
          const restartRequired = mockProtectedRoutesHaveRestartDebt()
        if (!restartRequired) mockRuntimeProtectedRoutes = cloneMockProtectedRoutes(mockProtectedRoutes)
          await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
          return {
            route: existing,
            restart_required: restartRequired,
            pending_operation: null,
            restart_note: restartRequired
              ? 'This route change was cancelled, but other protected route changes still require a restart.'
              : 'Desired route state matches the running route set; no restart is required.',
          }
        }
        mockProtectedRoutes = mockProtectedRoutes.filter((item) => item.name !== name)
        const view = mockProtectedRouteStateRows().find((item) => item.name === name)
        const restartRequired = mockProtectedRoutesHaveRestartDebt()
        if (!restartRequired) mockRuntimeProtectedRoutes = cloneMockProtectedRoutes(mockProtectedRoutes)
        if (!view) {
          await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
          return {
            route: existing,
            restart_required: restartRequired,
            pending_operation: null,
            restart_note: restartRequired
              ? 'This route change was cancelled, but other protected route changes still require a restart.'
              : 'Desired route state matches the running route set; no restart is required.',
          }
        }
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        return {
          route: view,
          restart_required: restartRequired,
          pending_operation: view.pending_operation ?? null,
          restart_note: restartRequired
            ? 'Saved for the next Labby restart.'
            : 'Desired route state matches the running route set; no restart is required.',
        }
      }
      const result = await gatewayApi.stageProtectedRouteRemove(name, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const addProtectedRoute = useCallback(
    async (route: ProtectedMcpRouteInput, signal?: AbortSignal): Promise<ProtectedMcpRoute> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        if (mockProtectedRoutes.some((item) => item.name === route.name)) {
          throw new Error(`Protected route ${route.name} already exists`)
        }
        if (mockProtectedRoutesHaveRestartDebt()) {
          throw new Error('Protected route changes are already staged; continue through staged actions or restart Labby first')
        }
        const runtimeExisting = mockRuntimeProtectedRoutes.find((item) => item.name === route.name)
        if (mockRouteIsSubset(route) || mockRouteIsSubset(runtimeExisting)) {
          throw new Error(`Protected route ${route.name} involves a gateway subset; stage the add for restart`)
        }
        mockProtectedRoutes = [...mockProtectedRoutes, route]
        mockRuntimeProtectedRoutes = [...mockRuntimeProtectedRoutes, route]
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        return route
      }
      const result = await gatewayApi.addProtectedRoute(route, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const updateProtectedRoute = useCallback(
    async (
      name: string,
      route: ProtectedMcpRouteInput,
      signal?: AbortSignal,
    ): Promise<ProtectedMcpRoute> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        const desiredExisting = mockProtectedRoutes.find((item) => item.name === name)
        const runtimeExisting = mockRuntimeProtectedRoutes.find((item) => item.name === name)
        if (mockProtectedRoutesHaveRestartDebt()) {
          throw new Error('Protected route changes are already staged; continue through staged actions or restart Labby first')
        }
        if (mockRouteIsSubset(desiredExisting) || mockRouteIsSubset(runtimeExisting) || mockRouteIsSubset(route)) {
          throw new Error(`Protected route ${name} involves a gateway subset; stage the update for restart`)
        }
        mockProtectedRoutes = mockProtectedRoutes.map((item) => (item.name === name ? route : item))
        mockRuntimeProtectedRoutes = mockRuntimeProtectedRoutes.map((item) => (item.name === name ? route : item))
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        return route
      }
      const result = await gatewayApi.updateProtectedRoute(name, route, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const removeProtectedRoute = useCallback(
    async (name: string, signal?: AbortSignal): Promise<ProtectedMcpRoute> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(300, signal)
        const removed = mockProtectedRoutes.find((item) => item.name === name)
        if (!removed) {
          throw new Error(`Protected route ${name} not found`)
        }
        const runtimeExisting = mockRuntimeProtectedRoutes.find((item) => item.name === name)
        if (mockProtectedRoutesHaveRestartDebt()) {
          throw new Error('Protected route changes are already staged; continue through staged actions or restart Labby first')
        }
        if (mockRouteIsSubset(removed) || mockRouteIsSubset(runtimeExisting)) {
          throw new Error(`Protected route ${name} involves a gateway subset; stage the removal for restart`)
        }
        mockProtectedRoutes = mockProtectedRoutes.filter((item) => item.name !== name)
        mockRuntimeProtectedRoutes = mockRuntimeProtectedRoutes.filter((item) => item.name !== name)
        await mutate(PROTECTED_MCP_ROUTES_KEY, mockProtectedRouteStateRows(), false)
        return removed
      }
      const result = await gatewayApi.removeProtectedRoute(name, signal)
      await mutate(PROTECTED_MCP_ROUTES_KEY)
      return result
    },
    [],
  )

  const testProtectedRoute = useCallback(
    async (
      route: ProtectedMcpRouteInput,
      signal?: AbortSignal,
    ): Promise<ProtectedMcpRouteTestResult> => {
      if (USE_MOCK_DATA) {
        await abortableMockDelay(250, signal)
        return {
          ok: true,
          route,
          resource: `https://${route.public_host}${route.public_path}`,
          metadata_url: `https://${route.public_host}/.well-known/oauth-protected-resource${route.public_path}`,
        }
      }
      return gatewayApi.testProtectedRoute(route, signal)
    },
    [],
  )

  const enableVirtualServer = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: true }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.enableVirtualServer(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const disableVirtualServer = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: false }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.disableVirtualServer(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const setVirtualServerSurface = useCallback(
    async (id: string, surface: 'cli' | 'api' | 'mcp' | 'webui', enabled: boolean): Promise<Gateway> => {
      if (USE_MOCK_DATA) {
        await mockDelay()
        const gateway = mockGateways.find((item) => item.id === id)
        if (!gateway) throw new Error('Gateway not found')
        const result = {
          ...gateway,
          surfaces: gateway.surfaces
            ? {
                ...gateway.surfaces,
                [surface]: { ...gateway.surfaces[surface], enabled },
              }
            : gateway.surfaces,
        }
        await mutate(gatewayKey(id), result, false)
        await mutate(GATEWAYS_KEY)
        return result
      }
      const result = await gatewayApi.setVirtualServerSurface(id, surface, enabled)
      await refreshGatewayCache(id)
      return result
    },
    [],
  )

  const enableGateway = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: true }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.enableGateway(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const disableGateway = useCallback(async (id: string): Promise<Gateway> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      const gateway = mockGateways.find((item) => item.id === id)
      if (!gateway) throw new Error('Gateway not found')
      const result = { ...gateway, enabled: false }
      await mutate(gatewayKey(id), result, false)
      await mutate(GATEWAYS_KEY)
      return result
    }
    const result = await gatewayApi.disableGateway(id)
    await refreshGatewayCache(id)
    return result
  }, [])

  const cleanupGateway = useCallback(async (
    id: string,
    aggressive: boolean = false,
    dryRun: boolean = false,
  ): Promise<GatewayCleanupResult> => {
    if (USE_MOCK_DATA) {
      await mockDelay()
      return {
        upstream: id,
        aggressive,
        dry_run: dryRun,
        gateway_matched: 0,
        local_matched: 0,
        aggressive_matched: 0,
        gateway_killed: 0,
        local_killed: 0,
        aggressive_killed: 0,
        gateway_matches: [],
        local_matches: [],
        aggressive_matches: [],
      }
    }
    const result = await gatewayApi.cleanupGateway(id, aggressive, dryRun)
    await refreshGatewayCache(id)
    return result
  }, [])

  return {
    createGateway,
    discoverExternalConfigs,
    importExternalConfigs,
    clearImportTombstone,
    restoreImportTombstone,
    updateGateway,
    removeGateway,
    removeVirtualServer,
    testGateway,
    reloadGateway,
    setExposurePolicy,
    previewExposurePolicy,
    saveServiceConfig,
    setCodeModeConfig,
    addLoadout,
    patchLoadout,
    removeLoadout,
    stageLoadoutUpdate,
    stageLoadoutRemove,
    addProtectedRoute,
    updateProtectedRoute,
    removeProtectedRoute,
    stageProtectedRouteAdd,
    stageProtectedRouteUpdate,
    stageProtectedRouteRemove,
    testProtectedRoute,
    enableVirtualServer,
    disableVirtualServer,
    enableGateway,
    disableGateway,
    cleanupGateway,
    setVirtualServerSurface,
  }
}
