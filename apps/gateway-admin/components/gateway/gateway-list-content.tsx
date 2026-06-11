'use client'

import { useDeferredValue, useMemo, useState, type ReactNode } from 'react'
import {
  Activity,
  ArrowLeft,
  Cable,
  Download,
  LayoutList,
  Plus,
  Rows3,
  Search,
  SlidersHorizontal,
  TriangleAlert,
  Wrench,
} from 'lucide-react'
import { toast } from 'sonner'
import { AppHeader } from '@/components/app-header'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { useGateways, useGatewayMutations } from '@/lib/hooks/use-gateways'
import type { Gateway, CreateGatewayInput, UpdateGatewayInput, DiscoveredMcpServer } from '@/lib/types/gateway'
import { cn, getErrorMessage } from '@/lib/utils'
import {
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import {
  aggregateToolsFromGateways,
  filterGateways,
  filterTools,
  sortToolRows,
  type GatewayFilterState,
  type GatewayPrimaryLens,
  type GatewaySourceFacet,
  type GatewayStatusFacet,
  type GatewayTransportFacet,
  type ToolFilterState,
  type ToolsExposureFilter,
} from './gateway-list-state'
import { EmptyState } from './empty-state'
import { GatewayFilters } from './gateway-filters'
import { GatewayFormDialog } from './gateway-form-dialog'
import { GatewayTable } from './gateway-table'
import { GatewayTableSkeleton } from './table-skeleton'
import { GatewayToolsTable } from './gateway-tools-table'
import { TestResultPanel } from './test-result-panel'
import { CleanupResultPanel } from './cleanup-result-panel'
import { AURORA_GATEWAY_STAT, gatewayActionTone } from './gateway-theme'
import { CodeModeHeaderToggle } from './code-mode-toggle'

const DEFAULT_GATEWAY_LENS: GatewayPrimaryLens = 'enabled'
const DEFAULT_DENSITY: 'comfortable' | 'condensed' = 'comfortable'

const DEFAULT_TOOL_FILTERS: ToolFilterState = {
  search: '',
  gatewayIds: [],
  exposure: 'all',
  source: [],
  transport: [],
}

function buildDefaultGatewayFilters(primaryLens: GatewayPrimaryLens): GatewayFilterState {
  return {
    primaryLens,
    search: '',
    status: [],
    source: [],
    transport: [],
  }
}

function toggleArrayValue<T extends string>(values: T[], value: T): T[] {
  return values.includes(value) ? values.filter((item) => item !== value) : [...values, value]
}

type CleanupHistoryEntry = {
  label: string
  occurredAt: string
}

interface GatewaySummary {
  enabled: number
  healthy: number
  disconnected: number
  tools: number
}

export interface GatewayListViewProps {
  summary: GatewaySummary
  showToolsView: boolean
  gatewayFilters: GatewayFilterState
  toolFilters: ToolFilterState
  gatewayOptions: Array<{ value: string; label: string }>
  activeSearch: string
  mobileSheetOpen: boolean
  density: 'comfortable' | 'condensed'
  isLoading: boolean
  errorMessage?: string
  itemsCount: number
  filteredGateways: Gateway[]
  filteredToolRows: Parameters<typeof GatewayToolsTable>[0]['rows']
  discoveredConfigs?: DiscoveredMcpServer[] | null
  isDiscoveringConfigs?: boolean
  isImportingConfigs?: boolean
  onPrimaryLensChange: (lens: GatewayPrimaryLens | 'tools') => void
  onBackToGateways: () => void
  onMobileSheetOpenChange: (open: boolean) => void
  onDensityChange: (density: 'comfortable' | 'condensed') => void
  onSearchChange: (value: string) => void
  onGatewayFilterToggle: (group: 'status' | 'source' | 'transport', value: string) => void
  onToolFilterToggle: (group: 'gatewayIds' | 'source' | 'transport', value: string) => void
  onExposureChange: (value: ToolsExposureFilter) => void
  onClearFilters: () => void
  onCreate: () => void
  onDiscoverConfigs?: () => void
  onImportConfigs?: (names?: string[]) => void
  onRestoreConfig?: (server: DiscoveredMcpServer) => void
  onEdit: (gateway: Gateway) => void
  onTest: (gateway: Gateway) => void
  onReload: (gateway: Gateway) => void
  cleanupSummaryByGatewayId?: Record<
    string,
    { preview?: { label: string; occurredAt: string }; cleanup?: { label: string; occurredAt: string } }
  >
  onCleanup: (gateway: Gateway, aggressive: boolean, dryRun: boolean) => void
  onClearCleanupHistory: (gateway: Gateway) => void
  onToggleEnabled: (gateway: Gateway) => void
  onDelete: (gateway: Gateway) => void
}

export function GatewayListContent() {
  const { data: gateways, isLoading, error } = useGateways()
  const { testGateway, reloadGateway, cleanupGateway, removeGateway, removeVirtualServer, createGateway, discoverExternalConfigs, importExternalConfigs, restoreImportTombstone, updateGateway, enableGateway, disableGateway } =
    useGatewayMutations()

  const [primaryView, setPrimaryView] = useState<GatewayPrimaryLens | 'tools'>(DEFAULT_GATEWAY_LENS)
  const [lastGatewayFilters, setLastGatewayFilters] = useState<GatewayFilterState>(() =>
    buildDefaultGatewayFilters(DEFAULT_GATEWAY_LENS),
  )
  const [toolFilters, setToolFilters] = useState<ToolFilterState>(DEFAULT_TOOL_FILTERS)
  const [density, setDensity] = useState<'comfortable' | 'condensed'>(DEFAULT_DENSITY)
  const [mobileSheetOpen, setMobileSheetOpen] = useState(false)
  const deferredGatewayFilters = useDeferredValue(lastGatewayFilters)
  const deferredToolFilters = useDeferredValue(toolFilters)

  const [formOpen, setFormOpen] = useState(false)
  const [editingGateway, setEditingGateway] = useState<Gateway | null>(null)
  const [discoveredConfigs, setDiscoveredConfigs] = useState<DiscoveredMcpServer[] | null>(null)
  const [isDiscoveringConfigs, setIsDiscoveringConfigs] = useState(false)
  const [isImportingConfigs, setIsImportingConfigs] = useState(false)
  const [testResult, setTestResult] = useState<{
    gateway: Gateway
    result: Awaited<ReturnType<typeof testGateway>>
  } | null>(null)
  const [cleanupResult, setCleanupResult] = useState<{
    gateway: Gateway
    result: Awaited<ReturnType<typeof cleanupGateway>>
  } | null>(null)
  const [cleanupSummaryByGatewayId, setCleanupSummaryByGatewayId] = useState<
    Record<string, { preview?: CleanupHistoryEntry; cleanup?: CleanupHistoryEntry }>
  >({})

  const items = useMemo(() => gateways ?? [], [gateways])

  const summary = useMemo(() => {
    const enabled = items.filter((gateway) => gateway.enabled ?? true).length
    const healthy = items.filter((gateway) => (gateway.enabled ?? true) && gateway.status.healthy && gateway.status.connected).length
    const disconnected = items.filter((gateway) => (gateway.enabled ?? true) && !gateway.status.connected).length
    const tools = items.reduce((sum, gateway) => sum + gateway.status.discovered_tool_count, 0)

    return { enabled, healthy, disconnected, tools }
  }, [items])

  const toolRows = useMemo(() => aggregateToolsFromGateways(items), [items])

  const filteredGateways = useMemo(() => filterGateways(items, deferredGatewayFilters), [items, deferredGatewayFilters])

  const filteredToolRows = useMemo(
    () => sortToolRows(filterTools(toolRows, deferredToolFilters)),
    [deferredToolFilters, toolRows],
  )

  const gatewayOptions = useMemo(
    () => items.map((gateway) => ({ value: gateway.id, label: gateway.name })),
    [items],
  )

  const showToolsView = primaryView === 'tools'

  const handlePrimaryLens = (lens: GatewayPrimaryLens | 'tools') => {
    setMobileSheetOpen(false)

    if (lens === 'tools') {
      setPrimaryView('tools')
      setToolFilters(DEFAULT_TOOL_FILTERS)
      return
    }

    const nextGatewayFilters = buildDefaultGatewayFilters(lens)
    setLastGatewayFilters(nextGatewayFilters)
    setPrimaryView(lens)
  }

  const handleBackToGateways = () => {
    setPrimaryView(lastGatewayFilters.primaryLens)
    setMobileSheetOpen(false)
  }

  const handleSearchChange = (value: string) => {
    if (showToolsView) {
      setToolFilters((current) => ({ ...current, search: value }))
      return
    }

    setLastGatewayFilters((current) => ({ ...current, search: value }))
  }

  const handleGatewayFilterToggle = (
    group: 'status' | 'source' | 'transport',
    value: string,
  ) => {
    setLastGatewayFilters((current) => {
      if (group === 'status') {
        return {
          ...current,
          status: toggleArrayValue(current.status, value as GatewayStatusFacet),
        }
      }

      if (group === 'source') {
        return {
          ...current,
          source: toggleArrayValue(current.source, value as GatewaySourceFacet),
        }
      }

      return {
        ...current,
        transport: toggleArrayValue(current.transport, value as GatewayTransportFacet),
      }
    })
  }

  const handleToolFilterToggle = (
    group: 'gatewayIds' | 'source' | 'transport',
    value: string,
  ) => {
    setToolFilters((current) => {
      if (group === 'gatewayIds') {
        return {
          ...current,
          gatewayIds: toggleArrayValue(current.gatewayIds, value),
        }
      }

      if (group === 'source') {
        return {
          ...current,
          source: toggleArrayValue(current.source, value as GatewaySourceFacet),
        }
      }

      return {
        ...current,
        transport: toggleArrayValue(current.transport, value as GatewayTransportFacet),
      }
    })
  }

  const handleExposureChange = (value: ToolsExposureFilter) => {
    setToolFilters((current) => ({ ...current, exposure: value }))
  }

  const handleClearFilters = () => {
    if (showToolsView) {
      setToolFilters(DEFAULT_TOOL_FILTERS)
      return
    }

    setLastGatewayFilters((current) => ({
      ...current,
      search: '',
      status: [],
      source: [],
      transport: [],
    }))
  }

  const handleCreate = () => {
    setEditingGateway(null)
    setFormOpen(true)
  }

  const handleDiscoverConfigs = async () => {
    setIsDiscoveringConfigs(true)
    try {
      const discovered = await discoverExternalConfigs()
      setDiscoveredConfigs(discovered)
      const importable = discovered.filter((server) => !server.already_configured && !server.tombstoned).length
      toast.success(`${discovered.length} configs found, ${importable} available to import`)
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to scan MCP configs'))
    } finally {
      setIsDiscoveringConfigs(false)
    }
  }

  const handleImportConfigs = async (names?: string[]) => {
    setIsImportingConfigs(true)
    try {
      const result = await importExternalConfigs(names)
      const importedNames = result.imported.map((item) => item.config.name)
      toast.success(`${importedNames.length} servers imported disabled`)
      const refreshed = await discoverExternalConfigs()
      setDiscoveredConfigs(refreshed)
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to import MCP configs'))
    } finally {
      setIsImportingConfigs(false)
    }
  }

  const handleRestoreConfig = async (server: DiscoveredMcpServer) => {
    setIsImportingConfigs(true)
    try {
      const gateway = await restoreImportTombstone(server)
      toast.success(`${gateway.name} restored disabled`)
      const refreshed = await discoverExternalConfigs()
      setDiscoveredConfigs(refreshed)
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to restore MCP config import'))
    } finally {
      setIsImportingConfigs(false)
    }
  }

  const handleEdit = (gateway: Gateway) => {
    setEditingGateway(gateway)
    setFormOpen(true)
  }

  const handleTest = async (gateway: Gateway) => {
    try {
      const result = await testGateway(gateway.id)
      setTestResult({ gateway, result })
      if (result.severity === 'warning') {
        toast.warning(result.detail || result.message)
      } else if (result.success) {
        toast.success('Connection test passed')
      } else {
        toast.error(result.error || result.message)
      }
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to test server'))
    }
  }

  const handleReload = async (gateway: Gateway) => {
    try {
      const result = await reloadGateway(gateway.id)
      if (result.success) {
        toast.success(`Server reloaded: ${result.new_tool_count} tools discovered`)
      } else {
        toast.error(result.message)
      }
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to reload server'))
    }
  }

  const handleDelete = async (gateway: Gateway) => {
    try {
      if (gateway.source === 'in_process') {
        await removeVirtualServer(gateway.id)
        toast.success('Stale service removed successfully')
      } else {
        await removeGateway(gateway.id)
        toast.success('Server removed successfully')
      }
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to remove server'))
    }
  }

  const handleCleanup = async (gateway: Gateway, aggressive: boolean, dryRun: boolean) => {
    try {
      const result = await cleanupGateway(gateway.id, aggressive, dryRun)
      setCleanupResult({ gateway, result })
      const occurredAt = new Date().toISOString()
      const totalMatched =
        (result.gateway_matched ?? result.gateway_killed) +
        (result.local_matched ?? result.local_killed) +
        (result.aggressive_matched ?? result.aggressive_killed)
      const totalKilled =
        result.gateway_killed + result.local_killed + result.aggressive_killed
      setCleanupSummaryByGatewayId((current) => ({
        ...current,
        [gateway.id]: {
          ...current[gateway.id],
          ...(dryRun
            ? {
                preview: {
                  label: aggressive
                    ? `last preview: ${totalMatched} matched (aggressive)`
                    : `last preview: ${totalMatched} matched`,
                  occurredAt,
                },
              }
            : {
                cleanup: {
                  label: aggressive
                    ? `last cleanup: ${totalKilled} killed (aggressive)`
                    : `last cleanup: ${totalKilled} killed`,
                  occurredAt,
                },
              }),
        },
      }))
      if (dryRun) {
        toast.success(
          aggressive
            ? `Aggressive cleanup preview completed. ${totalMatched} processes matched.`
            : `Runtime cleanup preview completed. ${totalMatched} processes matched.`,
        )
      } else {
        toast.success(
          aggressive
            ? `Aggressive cleanup completed. ${totalKilled} processes terminated.`
            : `Runtime cleanup completed. ${totalKilled} processes terminated.`,
        )
      }
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to cleanup server runtime'))
    }
  }

  const handleClearCleanupHistory = (gateway: Gateway) => {
    setCleanupSummaryByGatewayId((current) => {
      const next = { ...current }
      delete next[gateway.id]
      return next
    })
    toast.success('Cleared row cleanup history')
  }

  const handleToggleEnabled = async (gateway: Gateway) => {
    if (gateway.enabled ?? true) {
      try {
        await disableGateway(gateway.id)
        toast.success('Server disabled. Catalog change sent and runtime cleanup requested.')
      } catch (requestError) {
        toast.error(getErrorMessage(requestError, 'Failed to disable server'))
      }
      return
    }

    try {
      await enableGateway(gateway.id)
      toast.success('Server enabled. Catalog change sent to clients.')
    } catch (requestError) {
      toast.error(getErrorMessage(requestError, 'Failed to update server state'))
    }
  }

  const handleSave = async (input: CreateGatewayInput | UpdateGatewayInput) => {
    if (editingGateway) {
      await updateGateway(editingGateway.id, input as UpdateGatewayInput)
      toast.success('Server updated successfully')
    } else {
      await createGateway(input as CreateGatewayInput)
      toast.success('Server created successfully')
    }
    setFormOpen(false)
    setEditingGateway(null)
  }

  const activeSearch = showToolsView ? toolFilters.search : lastGatewayFilters.search

  return (
    <>
      <GatewayListView
        summary={summary}
        showToolsView={showToolsView}
        gatewayFilters={lastGatewayFilters}
        toolFilters={toolFilters}
        gatewayOptions={gatewayOptions}
        activeSearch={activeSearch}
        mobileSheetOpen={mobileSheetOpen}
        density={density}
        isLoading={isLoading}
        errorMessage={error?.message}
        itemsCount={items.length}
        filteredGateways={filteredGateways}
        filteredToolRows={filteredToolRows}
        discoveredConfigs={discoveredConfigs}
        isDiscoveringConfigs={isDiscoveringConfigs}
        isImportingConfigs={isImportingConfigs}
        cleanupSummaryByGatewayId={cleanupSummaryByGatewayId}
        onPrimaryLensChange={handlePrimaryLens}
        onBackToGateways={handleBackToGateways}
        onMobileSheetOpenChange={setMobileSheetOpen}
        onDensityChange={setDensity}
        onSearchChange={handleSearchChange}
        onGatewayFilterToggle={handleGatewayFilterToggle}
        onToolFilterToggle={handleToolFilterToggle}
        onExposureChange={handleExposureChange}
        onClearFilters={handleClearFilters}
        onCreate={handleCreate}
        onDiscoverConfigs={handleDiscoverConfigs}
        onImportConfigs={handleImportConfigs}
        onRestoreConfig={handleRestoreConfig}
        onEdit={handleEdit}
        onTest={handleTest}
        onReload={handleReload}
        onCleanup={handleCleanup}
        onClearCleanupHistory={handleClearCleanupHistory}
        onToggleEnabled={handleToggleEnabled}
        onDelete={handleDelete}
      />

      <GatewayFormDialog
        open={formOpen}
        onOpenChange={setFormOpen}
        gateway={editingGateway}
        onSave={handleSave}
      />

      <TestResultPanel result={testResult} onClose={() => setTestResult(null)} />
      <CleanupResultPanel result={cleanupResult} onClose={() => setCleanupResult(null)} />
    </>
  )
}

export function GatewayListView({
  summary,
  showToolsView,
  gatewayFilters,
  toolFilters,
  gatewayOptions,
  activeSearch,
  mobileSheetOpen,
  density,
  isLoading,
  errorMessage,
  itemsCount,
  filteredGateways,
  filteredToolRows,
  discoveredConfigs = null,
  isDiscoveringConfigs = false,
  isImportingConfigs = false,
  onPrimaryLensChange,
  onBackToGateways,
  onMobileSheetOpenChange,
  onDensityChange,
  onSearchChange,
  onGatewayFilterToggle,
  onToolFilterToggle,
  onExposureChange,
  onClearFilters,
  cleanupSummaryByGatewayId,
  onCreate,
  onDiscoverConfigs = () => undefined,
  onImportConfigs = () => undefined,
  onRestoreConfig = () => undefined,
  onEdit,
  onTest,
  onReload,
  onCleanup,
  onClearCleanupHistory,
  onToggleEnabled,
  onDelete,
}: GatewayListViewProps) {
  return (
    <>
      <AppHeader
        breadcrumbs={[{ label: 'Gateway' }]}
        actions={
          <div className="flex items-center gap-2">
            {showToolsView ? (
              <Button
                variant="outline"
                size="sm"
                onClick={onBackToGateways}
                className={cn(
                  gatewayActionTone(),
                  'h-10 px-3 text-aurora-text-primary hover:bg-aurora-hover-bg',
                )}
              >
                <ArrowLeft className="mr-1.5 size-4" />
                Back to servers
              </Button>
            ) : null}
            {!showToolsView ? (
              <>
                <CodeModeHeaderToggle />
                <McpConfigHeaderActions
                  discoveredConfigs={discoveredConfigs}
                  isDiscovering={isDiscoveringConfigs}
                  isImporting={isImportingConfigs}
                  onDiscover={onDiscoverConfigs}
                  onImport={onImportConfigs}
                />
                <Button
                  variant="outline"
                  size="icon"
                  onClick={() => onPrimaryLensChange('tools')}
                  className={cn(
                    gatewayActionTone(),
                    'size-9 lg:hidden hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
                  )}
                  aria-label="Switch to tools view"
                >
                  <SlidersHorizontal className="size-3.5" />
                </Button>
                <Button
                  variant="outline"
                  size="icon"
                  onClick={() => onDensityChange('comfortable')}
                  className={cn(
                    gatewayActionTone(),
                    'hidden size-10 hover:bg-aurora-hover-bg hover:text-aurora-text-primary lg:inline-flex',
                    density === 'comfortable' && 'border-aurora-accent-primary/45 text-aurora-accent-strong',
                  )}
                  aria-label="Comfortable density"
                  aria-pressed={density === 'comfortable'}
                  title="Comfortable density"
                >
                  <LayoutList className="size-4" />
                </Button>
                <Button
                  variant="outline"
                  size="icon"
                  onClick={() => onDensityChange('condensed')}
                  className={cn(
                    gatewayActionTone(),
                    'hidden size-10 hover:bg-aurora-hover-bg hover:text-aurora-text-primary lg:inline-flex',
                    density === 'condensed' && 'border-aurora-accent-primary/45 text-aurora-accent-strong',
                  )}
                  aria-label="Condensed density"
                  aria-pressed={density === 'condensed'}
                  title="Condensed density"
                >
                  <Rows3 className="size-4" />
                </Button>
              </>
            ) : null}
            <Button
              onClick={onCreate}
              className={cn(
                gatewayActionTone('accent'),
                'hidden border px-4 text-aurora-text-primary hover:bg-aurora-hover-bg hover:text-aurora-text-primary sm:inline-flex',
              )}
            >
              <Plus className="mr-2 size-4" />
              Add Server
            </Button>
            <Button
              onClick={onCreate}
              size="icon"
              className={cn(
                gatewayActionTone('accent'),
                'size-9 border sm:hidden',
              )}
              aria-label="Add server"
            >
              <Plus className="size-3.5" />
            </Button>
          </div>
        }
      />
      <h1 className="sr-only">Servers</h1>

      <div
        className={cn(
          'relative min-h-[calc(100vh-3.5rem)] w-full overflow-hidden bg-aurora-page-bg text-aurora-text-primary',
          AURORA_PAGE_SHELL,
        )}
      >
        <div
          className="pointer-events-none absolute inset-0 opacity-[0.32] [background-image:linear-gradient(rgba(41,182,246,0.045)_1px,transparent_1px),linear-gradient(90deg,rgba(41,182,246,0.035)_1px,transparent_1px)] [background-size:28px_28px]"
          aria-hidden="true"
        />
        <div className={cn(AURORA_PAGE_FRAME, 'relative z-10 gap-4')}>
          <section className={cn(AURORA_MEDIUM_PANEL, 'p-1.5 lg:hidden')}>
            <div className="grid grid-cols-4 gap-1">
              <MobileSummaryChip
                metric="enabled"
                value={summary.enabled}
                icon={<Cable className="size-3.5" />}
                active={!showToolsView && gatewayFilters.primaryLens === 'enabled'}
                onClick={() => onPrimaryLensChange('enabled')}
              />
              <MobileSummaryChip
                metric="healthy"
                value={summary.healthy}
                icon={<Activity className="size-3.5" />}
                active={!showToolsView && gatewayFilters.primaryLens === 'healthy'}
                onClick={() => onPrimaryLensChange('healthy')}
              />
              <MobileSummaryChip
                metric="disconnected"
                value={summary.disconnected}
                icon={<TriangleAlert className="size-3.5" />}
                active={!showToolsView && gatewayFilters.primaryLens === 'disconnected'}
                onClick={() => onPrimaryLensChange('disconnected')}
              />
              <MobileSummaryChip
                metric="tools"
                value={summary.tools}
                icon={<Wrench className="size-3.5" />}
                active={showToolsView}
                onClick={() => onPrimaryLensChange('tools')}
              />
            </div>
          </section>

          <section className={cn(AURORA_STRONG_PANEL, 'hidden rounded-aurora-1 p-2 lg:block')}>
            <div className="grid h-full gap-2 sm:grid-cols-2 xl:grid-cols-4">
              <SummaryCard
                label="Enabled"
                value={summary.enabled}
                subline="Ready to route"
                icon={<Cable className="size-5 text-aurora-text-muted" />}
                active={!showToolsView && gatewayFilters.primaryLens === 'enabled'}
                onClick={() => onPrimaryLensChange('enabled')}
              />
              <SummaryCard
                label="Healthy"
                value={summary.healthy}
                subline={`${summary.healthy} connected`}
                icon={<Activity className="size-5 text-aurora-accent-strong" />}
                valueClassName="text-aurora-accent-strong"
                active={!showToolsView && gatewayFilters.primaryLens === 'healthy'}
                onClick={() => onPrimaryLensChange('healthy')}
              />
              <SummaryCard
                label="Disconnected"
                value={summary.disconnected}
                subline={`${summary.disconnected} needs attention`}
                icon={<TriangleAlert className="size-5 text-aurora-warn" />}
                valueClassName="text-aurora-warn"
                active={!showToolsView && gatewayFilters.primaryLens === 'disconnected'}
                onClick={() => onPrimaryLensChange('disconnected')}
              />
              <SummaryCard
                label="Discovered tools"
                value={summary.tools}
                subline={`${summary.tools} exposed surfaces`}
                icon={<Wrench className="size-5 text-aurora-accent-primary" />}
                active={showToolsView}
                onClick={() => onPrimaryLensChange('tools')}
              />
            </div>
          </section>

          <div className="grid gap-4">
            <GatewayFilters
              mode={showToolsView ? 'tools' : 'gateways'}
              search={activeSearch}
              gatewayFilters={{
                status: gatewayFilters.status,
                source: gatewayFilters.source,
                transport: gatewayFilters.transport,
              }}
              toolFilters={toolFilters}
              gatewayOptions={gatewayOptions}
              mobileSheetOpen={mobileSheetOpen}
              onMobileSheetOpenChange={onMobileSheetOpenChange}
              onSearchChange={onSearchChange}
              onGatewayFilterToggle={onGatewayFilterToggle}
              onToolFilterToggle={onToolFilterToggle}
              onExposureChange={onExposureChange}
              onClearFilters={onClearFilters}
            />

            <div>
              {!showToolsView && discoveredConfigs ? (
                <McpConfigImportReviewPanel
                  discoveredConfigs={discoveredConfigs}
                  isImporting={isImportingConfigs}
                  onImport={onImportConfigs}
                  onRestore={onRestoreConfig}
                />
              ) : null}
              {isLoading ? (
                <GatewayTableSkeleton />
              ) : errorMessage ? (
                <div className={cn(AURORA_STRONG_PANEL, 'p-8 text-center')}>
                  <p className="text-aurora-error">Failed to load servers</p>
                  <p className="mt-1 text-sm text-aurora-text-muted">{errorMessage}</p>
                </div>
              ) : showToolsView ? (
                filteredToolRows.length === 0 ? (
                  itemsCount === 0 || summary.tools === 0 ? (
                    <EmptyState
                      title="No discovered tools"
                      description="Reload or add a server to build the aggregated tools inventory."
                      action={itemsCount === 0 ? { label: 'Add Server', onClick: onCreate } : undefined}
                    />
                  ) : (
                    <EmptyState
                      title="No matching tools"
                      description="Try adjusting your filters to find the tools you want."
                    />
                  )
                ) : (
                  <GatewayToolsTable rows={filteredToolRows} />
                )
              ) : filteredGateways.length === 0 ? (
                itemsCount === 0 ? (
                  <EmptyState
                    title="No servers configured"
                    description="Get started by adding your first MCP server connection to manage upstream server tools."
                    action={{ label: 'Add Server', onClick: onCreate }}
                  />
                ) : (
                  <EmptyState
                    title="No matching servers"
                    description="Try adjusting your filters to find what you're looking for."
                  />
                )
              ) : (
                <GatewayTable
                  gateways={filteredGateways}
                  density={density}
                  cleanupSummaryByGatewayId={cleanupSummaryByGatewayId}
                  onEdit={onEdit}
                  onTest={onTest}
                  onReload={onReload}
                  onCleanup={onCleanup}
                  onClearCleanupHistory={onClearCleanupHistory}
                  onToggleEnabled={onToggleEnabled}
                  onDelete={onDelete}
                />
              )}
            </div>
          </div>
        </div>
      </div>
    </>
  )
}

function McpConfigHeaderActions({
  discoveredConfigs,
  isDiscovering,
  isImporting,
  onDiscover,
  onImport,
}: {
  discoveredConfigs: DiscoveredMcpServer[] | null
  isDiscovering: boolean
  isImporting: boolean
  onDiscover: () => void
  onImport: (names?: string[]) => void
}) {
  const importable = discoveredConfigs?.filter((server) => !server.already_configured && !server.tombstoned) ?? []
  const disabled = isDiscovering || isImporting

  return (
    <>
      <Button
        variant="outline"
        size="icon"
        onClick={onDiscover}
        disabled={disabled}
        className={cn(gatewayActionTone(), 'hidden size-10 hover:bg-aurora-hover-bg hover:text-aurora-text-primary sm:inline-flex')}
        aria-label="Scan MCP configs"
        title="Scan MCP configs"
      >
        <Search className={cn('size-4', isDiscovering && 'animate-pulse')} />
      </Button>
      <Button
        variant="outline"
        size="icon"
        onClick={() => onImport()}
        disabled={disabled || importable.length === 0}
        className={cn(gatewayActionTone('accent'), 'hidden size-10 hover:bg-aurora-hover-bg hover:text-aurora-text-primary sm:inline-flex')}
        aria-label="Import all MCP configs"
        title="Import all MCP configs"
      >
        <Download className={cn('size-4', isImporting && 'animate-pulse')} />
      </Button>
    </>
  )
}

function McpConfigImportReviewPanel({
  discoveredConfigs,
  isImporting,
  onImport,
  onRestore,
}: {
  discoveredConfigs: DiscoveredMcpServer[]
  isImporting: boolean
  onImport: (names?: string[]) => void
  onRestore: (server: DiscoveredMcpServer) => void
}) {
  const importable = discoveredConfigs.filter((server) => !server.already_configured && !server.tombstoned)
  const configured = discoveredConfigs.filter((server) => server.already_configured).length
  const tombstoned = discoveredConfigs.filter((server) => server.tombstoned).length

  return (
    <section className={cn(AURORA_STRONG_PANEL, 'mb-4 p-3')}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="outline">{discoveredConfigs.length} found</Badge>
        <Badge variant="outline" status={importable.length > 0 ? 'warn' : 'success'}>
          {importable.length} new
        </Badge>
        {configured > 0 ? <Badge variant="outline">{configured} configured</Badge> : null}
        {tombstoned > 0 ? <Badge variant="outline" status="warn">{tombstoned} removed</Badge> : null}
        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="icon"
            onClick={() => onImport()}
            disabled={isImporting || importable.length === 0}
            className={cn(gatewayActionTone('accent'), 'size-8 hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
            aria-label="Import all MCP configs"
            title="Import all MCP configs"
          >
            <Download className={cn('size-3.5', isImporting && 'animate-pulse')} />
          </Button>
        </div>
      </div>

      {discoveredConfigs.length > 0 ? (
        <details className="group mt-3 rounded-aurora-1 border border-aurora-border-strong bg-aurora-panel/60">
          <summary className="cursor-pointer px-3 py-2 text-xs font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary">
            Review scanned configs
          </summary>
          <div className="grid gap-2 border-t border-aurora-border-subtle p-2">
            {discoveredConfigs.slice(0, 6).map((server) => (
              <div
                key={`${server.source_client}:${server.name}:${server.source_path}`}
                className="min-w-0 rounded-aurora-1 border border-aurora-border-strong bg-aurora-panel/70 p-2.5"
              >
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-aurora-text-primary" title={server.name}>
                      {server.name}
                    </p>
                    <p className="mt-1 truncate text-xs text-aurora-text-muted" title={server.source_path}>
                      {server.source_client} / {server.transport === 'http' ? server.url_preview : server.command_preview}
                    </p>
                  </div>
                  <Badge variant="outline" status={server.already_configured ? 'success' : 'warn'}>
                    {server.already_configured ? 'configured' : server.tombstoned ? 'removed' : 'new'}
                  </Badge>
                </div>
                {server.tombstoned ? (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => onRestore(server)}
                    disabled={isImporting}
                    className={cn(gatewayActionTone(), 'mt-3 h-8 w-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                  >
                    <Download className="mr-2 size-3.5" />
                    Restore
                  </Button>
                ) : !server.already_configured ? (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => onImport([server.name])}
                    disabled={isImporting}
                    className={cn(gatewayActionTone(), 'mt-3 h-8 w-full hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
                  >
                    <Download className="mr-2 size-3.5" />
                    Import
                  </Button>
                ) : null}
              </div>
            ))}
          </div>
        </details>
      ) : (
        <p className="mt-3 text-xs text-aurora-text-muted">No external MCP configs found.</p>
      )}
    </section>
  )
}

function SummaryCard({
  label,
  value,
  subline,
  icon,
  active,
  onClick,
  valueClassName,
}: {
  label: string
  value: number
  subline: string
  icon: ReactNode
  active: boolean
  onClick: () => void
  valueClassName?: string
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        AURORA_GATEWAY_STAT,
        'cursor-pointer rounded-aurora-1 px-3 py-2.5 text-left transition-[background-color,border-color,box-shadow,transform] duration-150 ease-out focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34',
        !active &&
          'bg-aurora-panel/72 hover:border-aurora-accent-primary/28 hover:bg-aurora-hover-bg hover:shadow-[0_0_0_1px_rgba(87,190,255,0.08)]',
        active && 'border-aurora-accent-primary/40 bg-aurora-accent-primary/8 shadow-[inset_0_0_0_1px_rgba(87,190,255,0.12)]',
      )}
      aria-pressed={active}
    >
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <p className={AURORA_MUTED_LABEL}>{label}</p>
          <p className={cn(AURORA_DISPLAY_NUMBER, 'mt-1 text-[22px] text-aurora-text-primary', valueClassName)}>
            {value}
          </p>
          <p className="mt-1 truncate text-[11px] font-medium text-aurora-text-muted">{subline}</p>
        </div>
        <span className="ml-auto flex size-9 shrink-0 items-center justify-center rounded-aurora-1 border border-aurora-border-strong/80 bg-aurora-control-surface/70 text-aurora-text-muted shadow-[var(--aurora-highlight-medium)]">
          {icon}
        </span>
      </div>
    </button>
  )
}

function MobileSummaryChip({
  metric,
  value,
  icon,
  active,
  onClick,
}: {
  metric: 'enabled' | 'configured' | 'healthy' | 'disconnected' | 'tools'
  value: number
  icon: ReactNode
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      data-mobile-summary={metric}
      onClick={onClick}
      className={cn(
        'flex h-10 items-center justify-center gap-1.5 rounded-aurora-1 border px-2 text-sm font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34',
        'h-9 px-1.5 text-[13px]',
        active
          ? 'border-aurora-accent-primary/36 bg-aurora-accent-primary/12 text-aurora-text-primary'
          : 'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
      )}
      aria-pressed={active}
    >
      {icon}
      <span className={cn(AURORA_DISPLAY_NUMBER, 'text-[13px] leading-none text-current')}>{value}</span>
    </button>
  )
}
