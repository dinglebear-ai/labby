import type {
  Gateway,
  CreateGatewayInput,
  UpdateGatewayInput,
  TestGatewayResult,
  ReloadGatewayResult,
  GatewayCleanupResult,
  ExposurePolicy,
  ExposurePolicyPreview,
  ServiceConfig,
  ServiceAction,
  SupportedService,
  CodeModeConfig,
  CodeModeConfigInput,
  GatewayLoadout,
  GatewayLoadoutInput,
  GatewayLoadoutPatch,
  GatewayLoadoutStageResult,
  ProtectedMcpRoute,
  ProtectedMcpRouteInput,
  ProtectedMcpRouteStageResult,
  ProtectedMcpRouteTestResult,
  DiscoveredMcpServer,
  GatewayImportResult,
  GatewayImportTombstone,
} from '../types/gateway.ts'
import {
  type BackendGatewayMcpRuntimeView,
  type BackendGatewayRuntimeView,
  type BackendGatewayToolRow,
  type BackendServerView,
  type BackendGatewayView,
  type GatewayDiscoverySnapshot,
  buildGatewayCreatePayload,
  buildGatewayUpdatePayload,
  exposurePolicyFromConfig,
  humanizeProbeError,
  normalizeGateway,
  normalizeServerView,
  previewExposurePolicy,
  probeStatusFromRuntime,
} from '../server/gateway-adapter.ts'
import { testResultFromProbe } from '../server/gateway-test-result.ts'
import { gatewayActionUrl } from './gateway-config'
import { confirmGatewayParams } from './gateway-request'
import { EXPOSE_NONE_PATTERN, stripExposeNonePattern } from './tool-exposure-draft'
import { synthesizeLabGateway } from './gateway-list-model'
import { performServiceAction, safeFanout, type ServiceActionError } from './service-action-client'
import { gatewayDegradedWarningCounts, hasGatewayDegradedWarnings } from './gateway-degradation'

export class GatewayApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  constructor(
    message: string,
    status: number,
    code?: string
  ) {
    super(message)
    this.name = 'GatewayApiError'
    this.status = status
    this.code = code
  }
}

export async function gatewayAction<T>(
  action: string,
  params: object,
  signal?: AbortSignal,
): Promise<T> {
  return performServiceAction<T, GatewayApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Gateway',
    url: gatewayActionUrl(),
    createError: (message, status, code) => new GatewayApiError(message, status, code),
  })
}

async function fetchDiscovery(name: string, signal?: AbortSignal): Promise<GatewayDiscoverySnapshot> {
  const [tools, resources, prompts] = await Promise.all([
    gatewayAction<BackendGatewayToolRow[]>('gateway.discovered_tools', { name }, signal),
    gatewayAction<string[]>('gateway.discovered_resources', { name }, signal),
    gatewayAction<string[]>('gateway.discovered_prompts', { name }, signal),
  ])

  return {
    tools,
    resources: resources.map((resource) =>
      resource.includes('://') ? resource : `lab://upstream/${name}/${resource}`,
    ),
    prompts,
  }
}

async function probeGateway(name: string, signal?: AbortSignal) {
  try {
    const runtime = await gatewayAction<BackendGatewayRuntimeView>(
      'gateway.test',
      confirmGatewayParams({ name }),
      signal,
    )
    return probeStatusFromRuntime(runtime)
  } catch (error) {
    if (error instanceof GatewayApiError) {
      return {
        connected: false,
        healthy: false,
        last_error: error.message,
      }
    }
    throw error
  }
}

async function normalizeGatewayView(
  view: BackendGatewayView,
  includeDiscovery: boolean,
  runtime: BackendGatewayMcpRuntimeView | undefined,
  signal?: AbortSignal,
): Promise<Gateway> {
  const [probe, discovery] = await Promise.all([
    probeGateway(view.config.name, signal),
    includeDiscovery
      ? fetchDiscovery(view.config.name, signal)
      : Promise.resolve({
          tools: [],
          resources: [],
          prompts: [],
        }),
  ])

  return normalizeGateway(view, probe, discovery, runtime)
}

async function normalizeGatewaySnapshotView(
  view: BackendGatewayView,
  includeDiscovery: boolean,
  signal?: AbortSignal,
  runtime?: BackendGatewayMcpRuntimeView,
): Promise<Gateway> {
  const discovery = includeDiscovery
    ? await fetchDiscovery(view.config.name, signal)
    : { tools: [], resources: [], prompts: [] }
  return normalizeGateway(view, probeStatusFromRuntime(view.runtime), discovery, runtime)
}

async function findServerView(id: string, signal?: AbortSignal): Promise<BackendServerView> {
  return gatewayAction<BackendServerView>('gateway.server.get', { id }, signal)
}

function compiledTools(actions: ServiceAction[]): ServiceAction[] {
  return [...actions].sort((left, right) => left.name.localeCompare(right.name))
}

async function fetchSortedServiceActions(
  service: string,
  signal?: AbortSignal,
): Promise<ServiceAction[]> {
  return compiledTools(
    await gatewayAction<ServiceAction[]>(
      'gateway.service_actions',
      { service },
      signal,
    ),
  )
}

async function fetchVirtualServerAllowedActions(
  id: string,
  signal?: AbortSignal,
): Promise<string[] | undefined> {
  try {
    const policy = await gatewayAction<{ allowed_actions: string[] }>(
      'gateway.virtual_server.get_mcp_policy',
      { id },
      signal,
    )
    return policy.allowed_actions
  } catch (error) {
    if (signal?.aborted) {
      throw error
    }
    if (error instanceof GatewayApiError && (error.status === 404 || error.code === 'method_not_found')) {
      return undefined
    }
    throw error
  }
}

function logGatewayDegradation(gateways: Gateway[]) {
  const counts = gatewayDegradedWarningCounts(gateways)
  if (hasGatewayDegradedWarnings(counts)) {
    console.warn('[gateway] degraded gateway rows', counts)
  }
}

function applyRuntimeRow(gateway: Gateway, runtime: BackendGatewayMcpRuntimeView | undefined): Gateway {
  if (!runtime) {
    return gateway
  }

  const enabled = runtime.enabled ?? gateway.enabled ?? true
  const connected = Boolean(runtime.connected)
  const discoveredToolCount = runtime.discovered_tool_count ?? gateway.status.discovered_tool_count
  const exposedToolCount = runtime.exposed_tool_count ?? gateway.status.exposed_tool_count
  const discoveredResourceCount = runtime.discovered_resource_count ?? gateway.status.discovered_resource_count
  const exposedResourceCount = runtime.exposed_resource_count ?? gateway.status.exposed_resource_count
  const discoveredPromptCount = runtime.discovered_prompt_count ?? gateway.status.discovered_prompt_count
  const exposedPromptCount = runtime.exposed_prompt_count ?? gateway.status.exposed_prompt_count
  const discoveredSkillCount = runtime.discovered_skill_count ?? gateway.status.discovered_skill_count ?? 0
  const exposedSkillCount = runtime.exposed_skill_count ?? gateway.status.exposed_skill_count ?? 0

  return {
    ...gateway,
    enabled,
    transport: runtime.transport === 'http' || runtime.transport === 'stdio'
      ? runtime.transport
      : gateway.transport,
    surfaces: {
      cli: gateway.surfaces?.cli ?? { enabled: false, connected: false },
      api: gateway.surfaces?.api ?? { enabled: false, connected: false },
      mcp: {
        ...(gateway.surfaces?.mcp ?? { enabled: false, connected: false }),
        enabled,
        connected,
      },
      webui: gateway.surfaces?.webui ?? { enabled: false, connected: false },
    },
    config: {
      ...gateway.config,
      ...((runtime.transport === 'http' && runtime.target && !gateway.config.url)
        ? { url: runtime.target }
        : {}),
      ...((runtime.transport === 'stdio' && runtime.target && !gateway.config.command)
        ? { command: runtime.target }
        : {}),
    },
    status: {
      ...gateway.status,
      connected,
      healthy: Boolean(enabled && connected && !gateway.status.last_error),
      discovered_tool_count: discoveredToolCount,
      exposed_tool_count: exposedToolCount,
      discovered_resource_count: discoveredResourceCount,
      exposed_resource_count: exposedResourceCount,
      discovered_prompt_count: discoveredPromptCount,
      exposed_prompt_count: exposedPromptCount,
      discovered_skill_count: discoveredSkillCount,
      exposed_skill_count: exposedSkillCount,
      supports_skills: runtime.supports_skills ?? gateway.status.supports_skills,
      likely_stale_count: runtime.likely_stale_count,
      pid: runtime.pid ?? undefined,
      pgid: runtime.pgid ?? undefined,
      age_seconds: runtime.age_seconds ?? undefined,
      origin: runtime.origin ?? undefined,
      owner: runtime.owner
        ? {
            surface: runtime.owner.surface,
            subject: runtime.owner.subject ?? undefined,
            request_id: runtime.owner.request_id ?? undefined,
            session_id: runtime.owner.session_id ?? undefined,
            client_name: runtime.owner.client_name ?? undefined,
            raw: runtime.owner.raw ?? undefined,
          }
        : undefined,
      runtime_state_path: runtime.runtime_state_path ?? undefined,
      reconciled_at: runtime.reconciled_at ?? undefined,
    },
  }
}

async function normalizeLabServiceServer(
  serverView: BackendServerView,
  signal?: AbortSignal,
): Promise<Gateway> {
  const [serviceConfig, actions, allowedActions] = await Promise.all([
    gatewayAction<ServiceConfig>(
      'gateway.service_config.get',
      { service: serverView.name },
      signal,
    ),
    fetchSortedServiceActions(serverView.name, signal),
    fetchVirtualServerAllowedActions(serverView.id, signal),
  ])
  const serviceView = normalizeServerView(serverView, {
    tools: actions,
    allowed_actions: allowedActions,
  })

  return {
    ...serviceView,
    config: {
      ...serviceView.config,
      url: fieldPreview(serviceConfig, '_URL'),
    },
  }
}

async function fallbackSupportedServiceGateway(
  id: string,
  signal?: AbortSignal,
): Promise<Gateway | null> {
  const supportedServices = await gatewayAction<SupportedService[]>('gateway.supported_services', {}, signal)
  const supported = supportedServices.find((service) => service.key === id)

  if (!supported) {
    return null
  }

  const [serviceConfig, actions] = await Promise.all([
    gatewayAction<ServiceConfig>('gateway.service_config.get', { service: supported.key }, signal),
    fetchSortedServiceActions(supported.key, signal),
  ])

  return synthesizeLabGateway(supported, serviceConfig, actions)
}

async function mutateVirtualServer(
  action: 'gateway.virtual_server.enable' | 'gateway.virtual_server.disable',
  id: string,
  signal?: AbortSignal,
): Promise<Gateway> {
  const view = await gatewayAction<BackendServerView>(action, confirmGatewayParams({ id }), signal)
  return normalizeLabServiceServer(view, signal)
}

async function mutateGatewayEnabled(
  action: 'gateway.mcp.enable' | 'gateway.mcp.disable',
  id: string,
  signal?: AbortSignal,
): Promise<Gateway> {
  if (action === 'gateway.mcp.enable') {
    const view = await gatewayAction<BackendGatewayView>(
      action,
      confirmGatewayParams({ name: id }),
      signal,
    )
    return normalizeGatewayView(view, true, undefined, signal)
  }

  const result = await gatewayAction<{ gateway: BackendGatewayView }>(
    action,
    confirmGatewayParams({ name: id, cleanup: true, aggressive: false }),
    signal,
  )
  return normalizeGatewayView(result.gateway, true, undefined, signal)
}

function fieldPreview(config: ServiceConfig, suffix: string): string | undefined {
  return config.fields.find((field) => field.name.endsWith(suffix))?.value_preview ?? undefined
}

function importTombstoneParams(server: DiscoveredMcpServer) {
  return {
    name: server.name,
    source_client: server.source_client,
    source_path: server.source_path,
    transport_fingerprint: server.transport_fingerprint,
  }
}

// Older Labby servers serialized `upstreams`/`services` with
// `skip_serializing_if = "Vec::is_empty"`, so a Loadout that selected only
// upstreams (or only services) arrived with the other key absent even though
// the published type declares both as required arrays — and every consumer
// reads `.length` off them, which took down the whole Loadouts page. Current
// servers always emit both keys, but a freshly deployed UI can face an older
// backend, so normalize at the adapter boundary as deploy-skew defense.
type WireGatewayLoadout = Omit<GatewayLoadout, 'upstreams' | 'services'> & {
  upstreams?: string[]
  services?: string[]
}

type WireGatewayLoadoutStageResult = Omit<GatewayLoadoutStageResult, 'loadout'> & {
  loadout: WireGatewayLoadout
}

function normalizeLoadout(loadout: WireGatewayLoadout): GatewayLoadout {
  return {
    ...loadout,
    upstreams: loadout.upstreams ?? [],
    services: loadout.services ?? [],
  }
}

function normalizeLoadoutStageResult(
  result: WireGatewayLoadoutStageResult,
): GatewayLoadoutStageResult {
  return { ...result, loadout: normalizeLoadout(result.loadout) }
}

export const gatewayApi = {
  async refreshStatus(name?: string, signal?: AbortSignal): Promise<void> {
    await gatewayAction('gateway.status', name ? { name } : {}, signal)
  },

  async discoverExternalConfigs(signal?: AbortSignal): Promise<DiscoveredMcpServer[]> {
    return gatewayAction<DiscoveredMcpServer[]>('gateway.discover', {}, signal)
  },

  async importExternalConfigs(names?: string[], signal?: AbortSignal): Promise<GatewayImportResult> {
    // Empty array is a no-op — caller must pass undefined/null to mean "import all"
    if (names !== undefined && names !== null && names.length === 0) {
      return { imported: [], skipped: [], errors: [] }
    }
    const params = names && names.length > 0
      ? { names }
      : { all: true }
    const raw = await gatewayAction<GatewayImportResult>('gateway.import', confirmGatewayParams(params), signal)
    return {
      imported: raw.imported,
      skipped: raw.skipped ?? [],
      errors: raw.errors ?? [],
    }
  },

  async clearImportTombstone(server: DiscoveredMcpServer, signal?: AbortSignal): Promise<GatewayImportTombstone[]> {
    return gatewayAction<GatewayImportTombstone[]>(
      'gateway.import_tombstones.clear',
      confirmGatewayParams(importTombstoneParams(server)),
      signal,
    )
  },

  async restoreImportTombstone(server: DiscoveredMcpServer, signal?: AbortSignal): Promise<Gateway> {
    const view = await gatewayAction<BackendGatewayView>(
      'gateway.import_tombstones.restore',
      confirmGatewayParams(importTombstoneParams(server)),
      signal,
    )
    return normalizeGatewayView(view, true, undefined, signal)
  },

  async list(signal?: AbortSignal): Promise<Gateway[]> {
    const views = await gatewayAction<BackendServerView[]>('gateway.list', {}, signal)
    const normalizedResults = await safeFanout(
      views,
      async (view) => normalizeServerView(view),
    )
    const gateways = normalizedResults.map((result) => {
      if (result.ok) {
        return result.value
      }
      if (signal?.aborted) {
        throw result.error
      }
      const message = result.error instanceof Error
        ? result.error.message
        : 'Failed to load gateway row details'
      return normalizeServerView({
        ...result.item,
        warnings: [
          ...(result.item.warnings ?? []),
          {
            code: 'service_catalog_unavailable',
            message,
          },
        ],
      }, result.item.source === 'in_process' ? { tools: [] } : undefined)
    })
    logGatewayDegradation(gateways)
    return gateways
  },

  async hydrateRuntime(gateways: Gateway[], signal?: AbortSignal): Promise<Gateway[]> {
    const runtimeRows = await gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal)
    const runtimeByName = new Map(runtimeRows.map((row) => [row.name, row]))
    const hydrated = gateways.map((gateway) => applyRuntimeRow(gateway, runtimeByName.get(gateway.name)))
    logGatewayDegradation(hydrated)
    return hydrated
  },

  async get(id: string, signal?: AbortSignal): Promise<Gateway> {
    let serverView: BackendServerView
    try {
      serverView = await findServerView(id, signal)
    } catch (error) {
      if (error instanceof GatewayApiError) {
        const fallback = await fallbackSupportedServiceGateway(id, signal)
        if (fallback) {
          return fallback
        }
      }
      throw error
    }
    if (serverView.source === 'in_process') {
      return normalizeLabServiceServer(serverView, signal)
    }

    const [view, runtimeRows] = await Promise.all([
      gatewayAction<BackendGatewayView>('gateway.get', { name: id }, signal),
      gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal),
    ])
    return normalizeGatewaySnapshotView(
      view,
      true,
      signal,
      runtimeRows.find((runtime) => runtime.name === id),
    )
  },

  async create(input: CreateGatewayInput, signal?: AbortSignal): Promise<Gateway> {
    const view = await gatewayAction<BackendGatewayView>(
      'gateway.add',
      confirmGatewayParams(buildGatewayCreatePayload(input)),
      signal,
    )
    return normalizeGatewaySnapshotView(view, true, signal)
  },

  async update(id: string, input: UpdateGatewayInput, signal?: AbortSignal): Promise<Gateway> {
    const view = await gatewayAction<BackendGatewayView>(
      'gateway.update',
      confirmGatewayParams(buildGatewayUpdatePayload(id, input)),
      signal,
    )
    return normalizeGatewaySnapshotView(view, true, signal)
  },

  async remove(id: string, signal?: AbortSignal): Promise<void> {
    await gatewayAction<BackendGatewayView>('gateway.remove', confirmGatewayParams({ name: id }), signal)
  },

  async removeVirtualServer(id: string, signal?: AbortSignal): Promise<void> {
    await gatewayAction<BackendServerView>(
      'gateway.virtual_server.remove',
      confirmGatewayParams({ id }),
      signal,
    )
  },

  async test(id: string, signal?: AbortSignal): Promise<TestGatewayResult> {
    // The runtime view returned by `gateway.test` carries no timing field of
    // its own, so this is the only latency figure available to show the
    // operator — measure it around just the probe call, not the concurrent
    // `gateway.get` fetch, so it reflects what "the connection test" actually
    // took.
    const startedAt = performance.now()
    let elapsedMs: number | undefined
    const [runtime, view] = await Promise.all([
      gatewayAction<BackendGatewayRuntimeView>(
        'gateway.test',
        confirmGatewayParams({ name: id }),
        signal,
      ).then((result) => {
        elapsedMs = Math.round(performance.now() - startedAt)
        return result
      }),
      gatewayAction<BackendGatewayView>('gateway.get', { name: id }, signal),
    ])
    const probe = probeStatusFromRuntime(runtime)
    const detail = humanizeProbeError(probe.last_error, view.config)
    return testResultFromProbe(runtime, probe, detail, elapsedMs)
  },

  async reload(id: string, signal?: AbortSignal): Promise<ReloadGatewayResult> {
    const before = await gatewayAction<BackendGatewayView>('gateway.get', { name: id }, signal)
    await gatewayAction('gateway.reload', confirmGatewayParams({}), signal)
    const after = await gatewayAction<BackendGatewayView>('gateway.get', { name: id }, signal)

    return {
      success: true,
      message: 'Gateway reloaded successfully',
      previous_tool_count: before.runtime.tool_count,
      new_tool_count: after.runtime.tool_count,
    }
  },

  async getExposurePolicy(id: string, signal?: AbortSignal): Promise<ExposurePolicy> {
    const serverView = await findServerView(id, signal)
    if (serverView.source === 'in_process') {
      const policy = await gatewayAction<{ allowed_actions: string[] }>(
        'gateway.virtual_server.get_mcp_policy',
        { id },
        signal,
      )
      const patterns = stripExposeNonePattern(policy.allowed_actions)
      return {
        mode: policy.allowed_actions.length === 0 ? 'expose_all' : 'allowlist',
        patterns,
      }
    }

    const view = await gatewayAction<BackendGatewayView>('gateway.get', { name: id }, signal)
    return exposurePolicyFromConfig(view.config)
  },

  async setExposurePolicy(id: string, policy: ExposurePolicy, signal?: AbortSignal): Promise<ExposurePolicy> {
    const serverView = await findServerView(id, signal)
    if (serverView.source === 'in_process') {
      // Virtual servers genuinely still need the sentinel: on this surface
      // `allowed_actions: []` means expose-ALL (see `matchVirtualServerAction`
      // and `getExposurePolicy`), so an empty list cannot express expose-none
      // and a pattern matching nothing is the only encoding available.
      const allowedActions = policy.mode === 'allowlist'
        ? policy.patterns.length === 0 ? [EXPOSE_NONE_PATTERN] : policy.patterns
        : []
      await gatewayAction<{ allowed_actions: string[] }>(
        'gateway.virtual_server.set_mcp_policy',
        confirmGatewayParams({
          id,
          allowed_actions: allowedActions,
        }),
        signal,
      )
      return {
        mode: policy.mode,
        patterns: stripExposeNonePattern(allowedActions),
      }
    }

    // Custom gateways express "expose nothing" as a real empty allowlist.
    // They used to need EXPOSE_NONE_PATTERN here because `update_upstream`
    // collapsed `[]` to null (expose-all); it no longer does, so writing the
    // sentinel would only keep manufacturing configs that carry a magic
    // string chosen to look like a tool name. Reads still strip it for
    // configs already written that way — see `exposurePolicyFromConfig`.
    const exposeTools = policy.mode === 'allowlist' ? policy.patterns : null
    await gatewayAction<BackendGatewayView>(
      'gateway.update',
      confirmGatewayParams({
        name: id,
        patch: {
          expose_tools: exposeTools,
        },
      }),
      signal,
    )
    return policy.mode === 'allowlist'
      ? { mode: 'allowlist', patterns: stripExposeNonePattern(exposeTools ?? []) }
      : { mode: 'expose_all', patterns: [] }
  },

  async previewExposurePolicy(
    id: string,
    patterns: string[],
    signal?: AbortSignal,
  ): Promise<ExposurePolicyPreview> {
    const serverView = await findServerView(id, signal)
    const tools =
      serverView.source === 'in_process'
        ? (await gatewayAction<ServiceAction[]>(
            'gateway.service_actions',
            { service: serverView.name },
            signal,
          )).map(
            (action) => action.name,
          )
        : (await gatewayAction<BackendGatewayToolRow[]>('gateway.discovered_tools', { name: id }, signal)).map(
            (tool) => tool.name,
          )
    return previewExposurePolicy(tools, patterns)
  },

  async supportedServices(signal?: AbortSignal): Promise<SupportedService[]> {
    return gatewayAction<SupportedService[]>('gateway.supported_services', {}, signal)
  },

  async getServiceConfig(service: string, signal?: AbortSignal): Promise<ServiceConfig> {
    return gatewayAction<ServiceConfig>('gateway.service_config.get', { service }, signal)
  },

  async serviceActions(service: string, signal?: AbortSignal): Promise<ServiceAction[]> {
    return gatewayAction<ServiceAction[]>('gateway.service_actions', { service }, signal)
  },

  async getCodeModeConfig(signal?: AbortSignal): Promise<CodeModeConfig> {
    return gatewayAction<CodeModeConfig>('gateway.code_mode.get', {}, signal)
  },

  async setCodeModeConfig(
    input: CodeModeConfigInput,
    signal?: AbortSignal,
  ): Promise<CodeModeConfig> {
    return gatewayAction<CodeModeConfig>(
      'gateway.code_mode.set',
      confirmGatewayParams(input),
      signal,
    )
  },

  async listLoadouts(signal?: AbortSignal): Promise<GatewayLoadout[]> {
    const loadouts = await gatewayAction<WireGatewayLoadout[]>('gateway.loadout.list_state', {}, signal)
    return loadouts.map(normalizeLoadout)
  },

  async getLoadout(name: string, signal?: AbortSignal): Promise<GatewayLoadout> {
    return normalizeLoadout(await gatewayAction<WireGatewayLoadout>('gateway.loadout.get', { name }, signal))
  },

  async addLoadout(loadout: GatewayLoadoutInput, signal?: AbortSignal): Promise<GatewayLoadout> {
    return normalizeLoadout(await gatewayAction<WireGatewayLoadout>(
      'gateway.loadout.add',
      confirmGatewayParams({ loadout }),
      signal,
    ))
  },

  async updateLoadout(
    name: string,
    loadout: GatewayLoadoutInput,
    signal?: AbortSignal,
  ): Promise<GatewayLoadout> {
    return normalizeLoadout(await gatewayAction<WireGatewayLoadout>(
      'gateway.loadout.update',
      confirmGatewayParams({ name, loadout }),
      signal,
    ))
  },

  async patchLoadout(
    name: string,
    patch: GatewayLoadoutPatch,
    signal?: AbortSignal,
  ): Promise<GatewayLoadout> {
    return normalizeLoadout(await gatewayAction<WireGatewayLoadout>(
      'gateway.loadout.patch',
      confirmGatewayParams({ name, patch }),
      signal,
    ))
  },

  async stageLoadoutUpdate(
    name: string,
    loadout: GatewayLoadoutInput,
    signal?: AbortSignal,
  ): Promise<GatewayLoadoutStageResult> {
    return normalizeLoadoutStageResult(await gatewayAction<WireGatewayLoadoutStageResult>(
      'gateway.loadout.stage_update',
      confirmGatewayParams({ name, loadout }),
      signal,
    ))
  },

  async stageLoadoutPatch(
    name: string,
    patch: GatewayLoadoutPatch,
    signal?: AbortSignal,
  ): Promise<GatewayLoadoutStageResult> {
    return normalizeLoadoutStageResult(await gatewayAction<WireGatewayLoadoutStageResult>(
      'gateway.loadout.stage_patch',
      confirmGatewayParams({ name, patch }),
      signal,
    ))
  },

  async stageLoadoutRemove(
    name: string,
    signal?: AbortSignal,
  ): Promise<GatewayLoadoutStageResult> {
    return normalizeLoadoutStageResult(await gatewayAction<WireGatewayLoadoutStageResult>(
      'gateway.loadout.stage_remove',
      confirmGatewayParams({ name }),
      signal,
    ))
  },

  async removeLoadout(name: string, signal?: AbortSignal): Promise<GatewayLoadout> {
    return normalizeLoadout(await gatewayAction<WireGatewayLoadout>(
      'gateway.loadout.remove',
      confirmGatewayParams({ name }),
      signal,
    ))
  },

  async listProtectedRoutes(signal?: AbortSignal): Promise<ProtectedMcpRoute[]> {
    return gatewayAction<ProtectedMcpRoute[]>('gateway.protected_route.list_state', {}, signal)
  },

  async getProtectedRoute(name: string, signal?: AbortSignal): Promise<ProtectedMcpRoute> {
    return gatewayAction<ProtectedMcpRoute>('gateway.protected_route.get', { name }, signal)
  },

  async addProtectedRoute(
    route: ProtectedMcpRouteInput,
    signal?: AbortSignal,
  ): Promise<ProtectedMcpRoute> {
    return gatewayAction<ProtectedMcpRoute>(
      'gateway.protected_route.add',
      confirmGatewayParams({ route }),
      signal,
    )
  },

  async updateProtectedRoute(
    name: string,
    route: ProtectedMcpRouteInput,
    signal?: AbortSignal,
  ): Promise<ProtectedMcpRoute> {
    return gatewayAction<ProtectedMcpRoute>(
      'gateway.protected_route.update',
      confirmGatewayParams({ name, route }),
      signal,
    )
  },

  async removeProtectedRoute(name: string, signal?: AbortSignal): Promise<ProtectedMcpRoute> {
    return gatewayAction<ProtectedMcpRoute>(
      'gateway.protected_route.remove',
      confirmGatewayParams({ name }),
      signal,
    )
  },

  async stageProtectedRouteAdd(
    route: ProtectedMcpRouteInput,
    signal?: AbortSignal,
  ): Promise<ProtectedMcpRouteStageResult> {
    return gatewayAction<ProtectedMcpRouteStageResult>(
      'gateway.protected_route.stage_add',
      confirmGatewayParams({ route }),
      signal,
    )
  },

  async stageProtectedRouteUpdate(
    name: string,
    route: ProtectedMcpRouteInput,
    signal?: AbortSignal,
  ): Promise<ProtectedMcpRouteStageResult> {
    return gatewayAction<ProtectedMcpRouteStageResult>(
      'gateway.protected_route.stage_update',
      confirmGatewayParams({ name, route }),
      signal,
    )
  },

  async stageProtectedRouteRemove(
    name: string,
    signal?: AbortSignal,
  ): Promise<ProtectedMcpRouteStageResult> {
    return gatewayAction<ProtectedMcpRouteStageResult>(
      'gateway.protected_route.stage_remove',
      confirmGatewayParams({ name }),
      signal,
    )
  },

  async testProtectedRoute(
    route: ProtectedMcpRouteInput,
    signal?: AbortSignal,
  ): Promise<ProtectedMcpRouteTestResult> {
    return gatewayAction<ProtectedMcpRouteTestResult>(
      'gateway.protected_route.test',
      { route },
      signal,
    )
  },

  async setServiceConfig(
    service: string,
    values: Record<string, string>,
    signal?: AbortSignal,
  ): Promise<ServiceConfig> {
    return gatewayAction<ServiceConfig>(
      'gateway.service_config.set',
      confirmGatewayParams({ service, values }),
      signal,
    )
  },

  async enableVirtualServer(id: string, signal?: AbortSignal): Promise<Gateway> {
    return mutateVirtualServer('gateway.virtual_server.enable', id, signal)
  },

  async disableVirtualServer(id: string, signal?: AbortSignal): Promise<Gateway> {
    return mutateVirtualServer('gateway.virtual_server.disable', id, signal)
  },

  async enableGateway(id: string, signal?: AbortSignal): Promise<Gateway> {
    const serverView = await findServerView(id, signal)
    if (serverView.source === 'in_process') {
      return mutateVirtualServer('gateway.virtual_server.enable', id, signal)
    }
    return mutateGatewayEnabled('gateway.mcp.enable', id, signal)
  },

  async disableGateway(id: string, signal?: AbortSignal): Promise<Gateway> {
    const serverView = await findServerView(id, signal)
    if (serverView.source === 'in_process') {
      return mutateVirtualServer('gateway.virtual_server.disable', id, signal)
    }
    return mutateGatewayEnabled('gateway.mcp.disable', id, signal)
  },

  async cleanupGateway(
    id: string,
    aggressive: boolean = false,
    dryRun: boolean = false,
    signal?: AbortSignal,
  ): Promise<GatewayCleanupResult> {
    return await gatewayAction<GatewayCleanupResult>(
      'gateway.mcp.cleanup',
      confirmGatewayParams({ name: id, aggressive, dry_run: dryRun }),
      signal,
    )
  },

  async setVirtualServerSurface(
    id: string,
    surface: 'cli' | 'api' | 'mcp' | 'webui',
    enabled: boolean,
    signal?: AbortSignal,
  ): Promise<Gateway> {
    const view = await gatewayAction<BackendServerView>(
      'gateway.virtual_server.set_surface',
      confirmGatewayParams({ id, surface, enabled }),
      signal,
    )
    return normalizeLabServiceServer(view, signal)
  },
}
