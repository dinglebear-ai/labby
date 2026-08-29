'use client'

import dynamic from 'next/dynamic'
import {
  useEffect,
  useMemo,
  useState,
  type ButtonHTMLAttributes,
  type ReactNode,
} from 'react'
import { useRouter } from 'next/navigation'
import {
  Play,
  RefreshCw,
  Pencil,
  Trash2,
  Copy,
  Check,
  AlertTriangle,
  Clock,
  FileText,
  MessageSquare,
  BookOpen,
  Loader2,
  Search,
  Wrench,
  Settings,
  Power,
  SlidersHorizontal,
  Activity,
  Braces,
  Cpu,
  Globe,
  HardDrive,
  Lock,
  MemoryStick,
  Network,
  Terminal,
} from 'lucide-react'
import { toast } from 'sonner'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import {
  REMOVE_GATEWAY_CONFIRM_LABEL,
  REMOVE_GATEWAY_TITLE,
  removeGatewayDescription,
} from './gateway-confirmations'
import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent } from '@/components/ui/tabs'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { ToolExposureTable } from './tool-exposure-table'
import { PrimitiveExposureTable } from './primitive-exposure-table'
import type { GatewaySaveRollback } from './gateway-form-dialog'
import { TestResultPanel } from './test-result-panel'
import { CleanupResultPanel } from './cleanup-result-panel'
import { useGateway, useGatewayMutations } from '@/lib/hooks/use-gateways'
import type { Gateway, CreateGatewayInput, UpdateGatewayInput } from '@/lib/types/gateway'
import {
  applyBulkExposureToDraft,
  buildExposurePolicyFromDraft,
  getDraftExposureSummary,
} from '@/lib/api/tool-exposure-draft'
import { cn, getErrorMessage } from '@/lib/utils'
import { buildGatewayClientConfig } from '@/lib/api/gateway-client-config'
import { useStableToolExposure } from './use-stable-tool-exposure'
import { GatewayEnabledSetting } from './gateway-enabled-setting'
import {
  DETAIL_NO_DATA,
  DETAIL_PANEL_GRID_STYLE,
  DETAIL_STAT_GRID_STYLE,
  DetailCard,
  DetailIconButton,
  DetailInset,
  DetailMicroLabel,
  DetailMiniList,
  DetailStatCard,
  DetailTopbarButton,
  DetailWarnPill,
} from './gateway-detail-chrome'
import {
  DETAIL_KV_GRID_STYLE,
  DetailCapabilityCluster,
  DetailExposureCell,
  DetailKeyValueCard,
  DetailStatStrip,
  DetailStripCard,
  DetailTabBar,
  DetailTabTrigger,
  DetailTabsList,
  GATEWAY_DETAIL_TAB_LABELS,
  type GatewayDetailTab,
} from './gateway-detail-tabs'

const GatewayFormDialog = dynamic(
  () => import('./gateway-form-dialog').then((module) => module.GatewayFormDialog),
  { ssr: false },
)

function SettingRow({
  title,
  description,
  checked,
  onCheckedChange,
  ariaLabel,
}: {
  title: string
  description: string
  checked: boolean
  onCheckedChange: (v: boolean) => void
  ariaLabel?: string
}) {
  return (
    <div className="flex items-start justify-between gap-4 rounded-lg border bg-aurora-control-surface/10 p-4">
      <div className="min-w-0">
        <p className="text-sm font-semibold text-aurora-text-primary">{title}</p>
        <p className="mt-1 text-sm text-aurora-text-muted">{description}</p>
      </div>
      <Switch aria-label={ariaLabel ?? title} checked={checked} onCheckedChange={onCheckedChange} />
    </div>
  )
}

interface GatewayDetailContentProps {
  gatewayId: string | null
}

/** 3px separator between items in the header card's meta lane (mock literal). */
function HeaderMetaDot() {
  return (
    <span
      aria-hidden="true"
      style={{
        width: 3,
        height: 3,
        borderRadius: 999,
        background: 'color-mix(in srgb, var(--aurora-text-muted) 45%, transparent)',
      }}
    />
  )
}

/**
 * 18px borderless button in the header card's meta lane — the mock's transport
 * copy affordance and its metadata links share this chrome.
 */
function HeaderMetaButton({
  className,
  style,
  type = 'button',
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type={type}
      className={[
        'grid place-items-center shrink-0 cursor-pointer border-0 bg-transparent',
        'hover:bg-[var(--aurora-hover-bg)] hover:text-aurora-accent-strong',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
      style={{ width: 18, height: 18, borderRadius: 5, ...style }}
      {...rest}
    />
  )
}

/** The MCP glyph the mock puts beside the negotiated protocol version. */
function McpGlyph() {
  return (
    <svg
      width="11"
      height="11"
      viewBox="0 0 195 195"
      fill="none"
      stroke="currentColor"
      strokeWidth="14"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M25 97.85 92.88 29.97c9.37-9.37 24.57-9.37 33.94 0 9.37 9.38 9.37 24.57 0 33.94l-51.26 51.27" />
      <path d="m76.27 114.47 50.55-50.56c9.38-9.37 24.57-9.37 33.94 0l.36.36c9.37 9.37 9.37 24.57 0 33.94l-61.4 61.39c-3.12 3.13-3.12 8.19 0 11.31l12.61 12.61" />
      <path d="M109.85 46.94 59.65 97.15c-9.37 9.37-9.37 24.57 0 33.94 9.37 9.37 24.57 9.37 33.94 0l50.2-50.2" />
    </svg>
  )
}

function formatGatewayTimestamp(value: string | null | undefined): string {
  if (!value) {
    return 'Unknown'
  }

  const parsed = new Date(value)
  if (Number.isNaN(parsed.getTime())) {
    return value
  }

  return new Intl.DateTimeFormat('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
    hour12: true,
    timeZone: 'UTC',
  }).format(parsed)
}

export function GatewayDetailContent({ gatewayId }: GatewayDetailContentProps) {
  const router = useRouter()
  const { data: gateway, isLoading, error } = useGateway(gatewayId)
  const {
    testGateway,
    reloadGateway,
    updateGateway,
    removeGateway,
    setExposurePolicy,
    disableVirtualServer,
    disableGateway,
    enableVirtualServer,
    enableGateway,
    cleanupGateway,
    setVirtualServerSurface,
  } = useGatewayMutations()

  const [isTesting, setIsTesting] = useState(false)
  const [isReloading, setIsReloading] = useState(false)
  const [isCleaningRuntime, setIsCleaningRuntime] = useState(false)
  const [isAggressiveCleanup, setIsAggressiveCleanup] = useState(false)
  const [configCopied, setConfigCopied] = useState(false)
  const [editOpen, setEditOpen] = useState(false)
  const [removeConfirmationOpen, setRemoveConfirmationOpen] = useState(false)
  const [manageToolsMode, setManageToolsMode] = useState(false)
  const [draftSelectedToolNames, setDraftSelectedToolNames] = useState<string[]>([])
  const [selectedRowToolNames, setSelectedRowToolNames] = useState<string[]>([])
  const [isSavingExposure, setIsSavingExposure] = useState(false)
  const [exposureSaveError, setExposureSaveError] = useState<string | null>(null)
  const [inventorySearch, setInventorySearch] = useState('')
  const [inventoryFilter, setInventoryFilter] = useState<'all' | 'tools' | 'resources' | 'prompts'>('all')
  const [activeTab, setActiveTab] = useState<GatewayDetailTab>('overview')
  const [testResult, setTestResult] = useState<{ gateway: Gateway; result: Awaited<ReturnType<typeof testGateway>> } | null>(null)
  const [cleanupResult, setCleanupResult] = useState<{ gateway: Gateway; result: Awaited<ReturnType<typeof cleanupGateway>> } | null>(null)
  const [hasMounted, setHasMounted] = useState(false)
  const {
    signature: toolExposureSignature,
    allToolNames,
    currentExposedToolNames,
  } = useStableToolExposure(gateway?.discovery.tools ?? [])
  const isLabGateway = gateway?.source === 'in_process'
  const surfaceEntries = gateway?.surfaces
    ? ([
        ['cli', gateway.surfaces.cli],
        ['api', gateway.surfaces.api],
        ['mcp', gateway.surfaces.mcp],
      ] as const)
    : []
  const exposeAllTools =
    allToolNames.length > 0 && draftSelectedToolNames.length === allToolNames.length
  const displayedTools = useMemo(
    () => {
      const draftSet = new Set(draftSelectedToolNames)
      return (gateway?.discovery.tools ?? []).map((tool) => ({
        ...tool,
        exposed: draftSet.has(tool.name),
        matched_by: draftSet.has(tool.name) ? (exposeAllTools ? '*' : tool.name) : null,
      }))
    },
    [gateway?.discovery.tools, draftSelectedToolNames, exposeAllTools],
  )
  const clientConfigJson = useMemo(
    () => (gateway ? JSON.stringify(buildGatewayClientConfig(gateway), null, 2) : ''),
    [gateway],
  )

  useEffect(() => {
    setHasMounted(true)
  }, [])

  useEffect(() => {
    setDraftSelectedToolNames(currentExposedToolNames)
    setSelectedRowToolNames([])
    setManageToolsMode(false)
  }, [currentExposedToolNames, gateway?.id, toolExposureSignature])

  if (!gatewayId) {
    return (
      <>
        <AppHeader
          breadcrumbs={[
            { label: 'Gateway', href: '/gateways' },
            { label: 'Missing Server' }
          ]}
        />
        <div className="flex-1 p-6">
          <div className="rounded-lg border bg-aurora-panel-medium p-8 text-center">
            <AlertTriangle className="size-8 mx-auto text-destructive mb-3" />
            <p className="font-medium">No server selected</p>
            <p className="text-sm text-aurora-text-muted mt-1">
              Open this page from the server list or provide a server id in the URL query string.
            </p>
            <Button variant="outline" className="mt-4" onClick={() => router.push('/gateways')}>
              Back to Servers
            </Button>
          </div>
        </div>
      </>
    )
  }

  const handleTest = async () => {
    if (!gateway || !(gateway.enabled ?? true)) return
    setIsTesting(true)
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
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to test server'))
    } finally {
      setIsTesting(false)
    }
  }

  const handleReload = async () => {
    if (!gateway || gateway.source === 'in_process' || !(gateway.enabled ?? true)) return
    setIsReloading(true)
    try {
      const result = await reloadGateway(gateway.id)
      if (result.success) {
        toast.success(`Server reloaded: ${result.new_tool_count} tools discovered`)
      } else {
        toast.error(result.message)
      }
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to reload server'))
    } finally {
      setIsReloading(false)
    }
  }

  const handleCopyConfig = async () => {
    try {
      await navigator.clipboard.writeText(clientConfigJson)
      setConfigCopied(true)
      toast.success('Configuration copied to clipboard')
      setTimeout(() => setConfigCopied(false), 2000)
    } catch {
      toast.error('Failed to copy configuration to clipboard')
    }
  }

  const handleSave = async (
    input: CreateGatewayInput | UpdateGatewayInput,
  ): Promise<GatewaySaveRollback | void> => {
    if (!gateway) return
    const previous = gateway
    await updateGateway(gateway.id, input as UpdateGatewayInput)
    return async () => {
      await updateGateway(previous.id, {
        name: previous.name,
        transport: previous.transport,
        config: previous.config,
      })
    }
  }

  const handleDelete = async () => {
    if (!gateway) return
    try {
      await removeGateway(gateway.id)
      toast.success('Server removed successfully')
      router.push('/gateways')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to remove server'))
    }
  }

  const confirmDelete = () => {
    setRemoveConfirmationOpen(false)
    void handleDelete()
  }

  const handleEnableGateway = async () => {
    if (!gateway) return
    try {
      if (gateway.source === 'in_process') {
        await enableVirtualServer(gateway.id)
      } else {
        await enableGateway(gateway.id)
      }
      toast.success('Server enabled. Catalog change sent to clients.')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update server state'))
    }
  }

  const handleDisableGateway = async () => {
    if (!gateway || !(gateway.enabled ?? true)) return

    try {
      if (gateway.source === 'in_process') {
        await disableVirtualServer(gateway.id)
      } else {
        await disableGateway(gateway.id)
      }
      toast.success('Server disabled. Catalog change sent and runtime cleanup requested.')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update server state'))
    }
  }

  const handleSurfaceToggle = async (surface: 'cli' | 'api' | 'mcp' | 'webui', enabled: boolean) => {
    if (!gateway || gateway.source !== 'in_process') return
    try {
      await setVirtualServerSurface(gateway.id, surface, enabled)
      toast.success(`Updated ${surface.toUpperCase()} surface`)
    } catch (error) {
      toast.error(getErrorMessage(error, `Failed to update ${surface} surface`))
    }
  }

  if (!hasMounted || isLoading) {
    return (
      <>
        <AppHeader
          breadcrumbs={[
            { label: 'Gateway', href: '/gateways' },
            { label: 'Loading...' }
          ]}
        />
        <div className="flex-1 p-6">
          <div className="space-y-6">
            <div className="flex items-start justify-between">
              <div className="space-y-2">
                <Skeleton className="h-8 w-48" />
                <Skeleton className="h-5 w-32" />
              </div>
              <div className="flex gap-2">
                <Skeleton className="h-9 w-20" />
                <Skeleton className="h-9 w-20" />
              </div>
            </div>
            <Skeleton className="h-[400px] w-full rounded-lg" />
          </div>
        </div>
      </>
    )
  }

  if (error || !gateway) {
    return (
      <>
        <AppHeader
          breadcrumbs={[
            { label: 'Gateway', href: '/gateways' },
            { label: 'Error' }
          ]}
        />
        <div className="flex-1 p-6">
          <div className="rounded-lg border bg-aurora-panel-medium p-8 text-center">
            <AlertTriangle className="size-8 mx-auto text-destructive mb-3" />
            <p className="font-medium">Failed to load server</p>
            <p className="text-sm text-aurora-text-muted mt-1">
              {error?.message || 'Server not found'}
            </p>
            <Button variant="outline" className="mt-4" onClick={() => router.push('/gateways')}>
              Back to Servers
            </Button>
          </div>
        </div>
      </>
    )
  }

  const hasDraftChanges =
    draftSelectedToolNames.length !== currentExposedToolNames.length ||
    draftSelectedToolNames.some((toolName) => !currentExposedToolNames.includes(toolName))
  const exposureSummary = getDraftExposureSummary(allToolNames, draftSelectedToolNames)
  const resourceExposureEnabled = gateway.config.proxy_resources ?? true
  const promptExposureEnabled = gateway.config.proxy_prompts ?? true
  const skillExposureEnabled = gateway.config.proxy_skills ?? false
  const skillSupportLabel =
    gateway.status.supports_skills === true
      ? 'Supported'
      : gateway.status.supports_skills === false
        ? 'Not advertised'
        : 'Unknown'
  const toolsTabLabel = isLabGateway ? 'Actions' : 'Tools'
  const runtimeAgeLabel = gateway.status.age_seconds
    ? gateway.status.age_seconds < 60
      ? `${gateway.status.age_seconds}s old`
      : gateway.status.age_seconds < 3600
        ? `${Math.floor(gateway.status.age_seconds / 60)}m old`
        : gateway.status.age_seconds < 86400
          ? `${Math.floor(gateway.status.age_seconds / 3600)}h old`
          : `${Math.floor(gateway.status.age_seconds / 86400)}d old`
    : null

  const handleCleanupRuntime = async (aggressive: boolean, dryRun: boolean) => {
    if (!gateway || gateway.source === 'in_process') return
    const previousAggressive = isAggressiveCleanup
    setIsCleaningRuntime(true)
    setIsAggressiveCleanup(aggressive)
    try {
      const result = await cleanupGateway(gateway.id, aggressive, dryRun)
      setCleanupResult({ gateway, result })
      const totalMatched =
        (result.gateway_matched ?? result.gateway_killed) +
        (result.local_matched ?? result.local_killed) +
        (result.aggressive_matched ?? result.aggressive_killed)
      const totalKilled =
        result.gateway_killed + result.local_killed + result.aggressive_killed
      if (dryRun) {
        toast.success(
          aggressive
            ? `Aggressive runtime cleanup preview completed. ${totalMatched} processes matched.`
            : `Runtime cleanup preview completed. ${totalMatched} processes matched.`,
        )
      } else {
        toast.success(
          aggressive
            ? `Aggressive runtime cleanup completed. ${totalKilled} processes terminated.`
            : `Runtime cleanup completed. ${totalKilled} processes terminated.`,
        )
      }
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to clean up runtime'))
    } finally {
      setIsCleaningRuntime(false)
      setIsAggressiveCleanup(previousAggressive)
    }
  }

  const handleExposeAllChange = (checked: boolean) => {
    if (!manageToolsMode) {
      return
    }
    setDraftSelectedToolNames(checked ? [...allToolNames].sort((left, right) => left.localeCompare(right)) : [])
    setSelectedRowToolNames([])
    setExposureSaveError(null)
  }

  const handleBulkEnableSelected = (toolNames: string[]) => {
    setDraftSelectedToolNames((current) => applyBulkExposureToDraft(current, toolNames, true))
    setSelectedRowToolNames([])
    setExposureSaveError(null)
  }

  const handleBulkDisableSelected = (toolNames: string[]) => {
    setDraftSelectedToolNames((current) => applyBulkExposureToDraft(current, toolNames, false))
    setSelectedRowToolNames([])
    setExposureSaveError(null)
  }

  const handleCancelExposureDraft = () => {
    setDraftSelectedToolNames(currentExposedToolNames)
    setSelectedRowToolNames([])
    setManageToolsMode(false)
    setExposureSaveError(null)
  }

  const handleSaveExposureDraft = async () => {
    setIsSavingExposure(true)
    setExposureSaveError(null)
    try {
      const policy = buildExposurePolicyFromDraft(allToolNames, draftSelectedToolNames)
      await setExposurePolicy(gateway.id, policy)
      toast.success('Tool exposure updated successfully')
      setManageToolsMode(false)
      setSelectedRowToolNames([])
    } catch (error) {
      const message = getErrorMessage(error, 'Failed to update tool exposure')
      setExposureSaveError(`Could not save these exposure changes. Your draft is still local. ${message}`)
      toast.error(message)
    } finally {
      setIsSavingExposure(false)
    }
  }

  const handleProxyResourcesToggle = async (enabled: boolean) => {
    try {
      await updateGateway(gateway.id, {
        config: {
          proxy_resources: enabled,
        },
      })
      toast.success(enabled ? 'Resource exposure enabled' : 'Resource exposure disabled')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update resource exposure'))
    }
  }

  const handleProxyPromptsToggle = async (enabled: boolean) => {
    try {
      await updateGateway(gateway.id, {
        config: {
          proxy_prompts: enabled,
        },
      })
      toast.success(enabled ? 'Prompt exposure enabled' : 'Prompt exposure disabled')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update prompt exposure'))
    }
  }

  const handleProxySkillsToggle = async (enabled: boolean) => {
    try {
      await updateGateway(gateway.id, {
        config: {
          proxy_skills: enabled,
        },
      })
      toast.success(enabled ? 'Agent Skills trusted and enabled' : 'Agent Skills disabled')
    } catch (error) {
      toast.error(getErrorMessage(error, 'Failed to update Agent Skills trust'))
    }
  }

  /*
    AppHeader actions — the mock's `isDetailPage` topbar cluster, measured on
    the detail page (not the row expansion): 32px squares, radius-1, a
    70%-blended border on --aurora-control-surface, 5px apart. The mock's own
    order is Test · View in Logs · Reload · Generate skill · Edit · More, where
    More is a chevron menu holding Copy .mcp.json / Enable-Disable / Remove.

    Two of those have nothing behind them here — there is no per-server log
    route and no skill generator — so they are omitted rather than rendered
    dead. Remove stays a visible button instead of moving into a More menu we
    have no other occupants for; its confirm flow is unchanged.
  */
  const headerActions = (
    <div className="flex items-center" style={{ gap: 5 }}>
      {!isLabGateway && (
        <DetailTopbarButton
          onClick={handleTest}
          disabled={isTesting || !(gateway.enabled ?? true)}
          aria-label="Test server"
          title="Test connection"
        >
          {isTesting ? (
            <Loader2 size={13} className="animate-spin" />
          ) : (
            <Play size={13} />
          )}
        </DetailTopbarButton>
      )}
      {!isLabGateway && (
        <DetailTopbarButton
          onClick={handleReload}
          disabled={isReloading || !(gateway.enabled ?? true)}
          aria-label="Reload server"
          title="Reload server"
        >
          <RefreshCw size={13} className={isReloading ? 'animate-spin' : undefined} />
        </DetailTopbarButton>
      )}
      <DetailTopbarButton
        onClick={() => setEditOpen(true)}
        aria-label="Edit server"
        title="Edit server"
      >
        <Pencil size={13} />
      </DetailTopbarButton>
      <DetailTopbarButton
        onClick={() => setRemoveConfirmationOpen(true)}
        aria-label="Remove server"
        title="Remove server"
      >
        <Trash2 size={13} />
      </DetailTopbarButton>
    </div>
  )

  const updatedAtLabel = formatGatewayTimestamp(gateway.updated_at)
  const isEnabled = gateway.enabled ?? true
  const isHealthy = gateway.status.healthy && gateway.status.connected && isEnabled
  // Mock: dStatusLabel / dDotColor / dDotHalo.
  const statusLabel = !isEnabled
    ? 'Disabled'
    : isHealthy
      ? 'Healthy'
      : gateway.status.connected
        ? 'Needs attention'
        : 'Disconnected'
  const statusDotColor = isHealthy
    ? 'var(--aurora-accent-strong)'
    : !gateway.status.connected || !isEnabled
      ? 'var(--aurora-error)'
      : 'var(--aurora-warn)'
  const statusDotHalo = isHealthy ? 'rgba(103,203,250,0.10)' : 'rgba(199,132,144,0.10)'
  const transportLabel =
    gateway.transport === 'http' ? 'HTTP' : gateway.transport === 'stdio' ? 'STDIO' : 'IN-PROCESS'
  // Mock: dChips — PID, runtime age, and a Disabled marker.
  const headerChips = [
    ...(gateway.status.pid ? [{ label: `PID ${gateway.status.pid}`, title: 'Runtime process id' }] : []),
    ...(runtimeAgeLabel ? [{ label: runtimeAgeLabel, title: 'Runtime age' }] : []),
    ...(isEnabled ? [] : [{ label: 'Disabled', title: 'Excluded from the active catalog' }]),
  ]
  // Mock: dExposureStats — the strip's leading cell, one row per primitive kind.
  const exposureStats = [
    {
      label: 'Tools',
      icon: <Wrench size={13} />,
      exposed: gateway.status.exposed_tool_count,
      discovered: gateway.status.discovered_tool_count,
    },
    {
      label: 'Resources',
      icon: <FileText size={13} />,
      exposed: gateway.status.exposed_resource_count,
      discovered: gateway.status.discovered_resource_count,
    },
    {
      label: 'Prompts',
      icon: <MessageSquare size={13} />,
      exposed: gateway.status.exposed_prompt_count,
      discovered: gateway.status.discovered_prompt_count,
    },
    {
      label: 'Skills',
      icon: <BookOpen size={13} />,
      exposed: gateway.status.exposed_skill_count ?? 0,
      discovered: gateway.status.discovered_skill_count ?? 0,
    },
  ]
  const totalExposedPrimitives = exposureStats.reduce((total, stat) => total + stat.exposed, 0)
  const totalDiscoveredPrimitives = exposureStats.reduce((total, stat) => total + stat.discovered, 0)
  /*
    Mock: dHealthCards. Calls / Errors lead, then the transport-specific pair —
    Process + Stale on stdio, Clients on HTTP. Everything the gateway API does
    not report dashes; nothing here is synthesised.
  */
  const stripCards: Array<{
    label: string
    value: ReactNode
    sub: ReactNode
    title?: string
  }> = [
    {
      label: 'Calls',
      value: DETAIL_NO_DATA,
      sub: 'last 24h',
      title: 'Call volume · last 24h — not reported by the gateway API',
    },
    {
      label: 'Errors',
      value: DETAIL_NO_DATA,
      sub: 'last 24h',
      title: 'Errors · last 24h — not reported by the gateway API',
    },
    ...(gateway.transport === 'stdio'
      ? [
          {
            label: 'Process',
            value: gateway.status.pid ? `pid ${gateway.status.pid}` : DETAIL_NO_DATA,
            sub: gateway.status.pid
              ? [
                  `pgid ${gateway.status.pgid ?? DETAIL_NO_DATA}`,
                  ...(runtimeAgeLabel ? [runtimeAgeLabel] : []),
                ].join(' · ')
              : 'not running',
          },
          {
            label: 'Stale',
            value: gateway.status.likely_stale_count ?? 0,
            sub: 'orphaned runtimes after reconciliation',
          },
        ]
      : [
          {
            label: 'Clients',
            value: DETAIL_NO_DATA,
            sub: 'live /mcp sessions',
            title: 'Connected MCP sessions — not reported by the gateway API',
          },
        ]),
  ]
  /*
    Mock: dProcRows / dMetaRows on the detail Overview tab. Same label
    vocabulary, our data. Fields the gateway API does not return — the
    upstream's serverInfo version and the negotiated protocolVersion — dash.
  */
  const httpOrigin = (() => {
    if (!gateway.config.url) return DETAIL_NO_DATA
    try {
      return new URL(gateway.config.url).origin
    } catch {
      return DETAIL_NO_DATA
    }
  })()
  const runtimeFactRows =
    gateway.transport === 'stdio'
      ? [
          { label: 'Command', value: gateway.config.command || DETAIL_NO_DATA },
          { label: 'Args', value: gateway.config.args?.join(' ') || DETAIL_NO_DATA },
          {
            label: 'PID / PGID',
            value: `${gateway.status.pid ?? DETAIL_NO_DATA} / ${gateway.status.pgid ?? DETAIL_NO_DATA}`,
          },
          { label: 'Uptime', value: runtimeAgeLabel?.replace(' old', '') ?? DETAIL_NO_DATA },
          {
            label: 'Runtime snapshot',
            value: gateway.status.runtime_state_path ?? DETAIL_NO_DATA,
          },
        ]
      : [
          { label: 'Endpoint', value: gateway.config.url || DETAIL_NO_DATA },
          { label: 'Origin', value: httpOrigin },
          {
            label: 'Auth',
            value: gateway.config.oauth_enabled
              ? 'OAuth'
              : gateway.config.bearer_token_env
                ? `Bearer · ${gateway.config.bearer_token_env}`
                : 'None',
          },
          {
            label: 'Runtime snapshot',
            value: gateway.status.runtime_state_path ?? DETAIL_NO_DATA,
          },
        ]
  const serverMetadataRows = [
    { label: 'serverInfo.name / version', value: `${gateway.name} · ${DETAIL_NO_DATA}` },
    { label: 'protocolVersion', value: DETAIL_NO_DATA },
    { label: 'origin', value: gateway.status.origin ?? 'server-managed' },
    {
      label: 'imported_from',
      value: gateway.config.imported_from
        ? `${gateway.config.imported_from.client} · ${gateway.config.imported_from.path}`
        : `${DETAIL_NO_DATA}  (manual entry)`,
    },
    {
      label: 'oauth_enabled / bearer_token_env',
      value: `${gateway.config.oauth_enabled ? 'true' : 'false'} · ${gateway.config.bearer_token_env ?? DETAIL_NO_DATA}`,
    },
    {
      label: 'proxy_resources / proxy_prompts / proxy_skills',
      value: `${resourceExposureEnabled ? 'true' : 'false'} · ${promptExposureEnabled ? 'true' : 'false'} · ${skillExposureEnabled ? 'true' : 'false'}`,
    },
    {
      label: 'skills extension / trust',
      value: `${skillSupportLabel} · ${skillExposureEnabled ? 'trusted' : 'not trusted'}`,
    },
    {
      label: 'reconciled_at',
      value: gateway.status.reconciled_at
        ? formatGatewayTimestamp(gateway.status.reconciled_at)
        : DETAIL_NO_DATA,
    },
    { label: 'updated_at', value: gateway.updated_at ? updatedAtLabel : DETAIL_NO_DATA },
  ]
  const endpointDisplay =
    gateway.transport === 'http'
      ? (gateway.config.url ?? '')
      : isLabGateway
        ? gateway.config.url ?? 'Lab-managed server configuration'
        : [gateway.config.command, ...(gateway.config.args ?? [])].join(' ')

  return (
    <>
      <AppHeader
        breadcrumbs={[
          { label: 'Gateway', href: '/gateways' },
          { label: gateway.name }
        ]}
        actions={headerActions}
      />

      <div className="flex-1 p-6 min-w-0 overflow-x-hidden">
        {!(gateway.enabled ?? true) ? (
          <div className="mb-4 flex items-start gap-3 rounded-lg border border-aurora-warn/30 bg-aurora-warn/10 px-4 py-3">
            <AlertTriangle className="mt-0.5 size-4 shrink-0 text-aurora-warn" />
            <div className="min-w-0">
              <p className="text-sm font-semibold text-aurora-text-primary">Server disabled</p>
              <p className="mt-1 text-sm text-aurora-text-muted">
                This server is excluded from the active catalog. Clients should no longer see its tools, resources, or prompts until you re-enable it.
              </p>
            </div>
          </div>
        ) : null}
        <Tabs value={activeTab} onValueChange={(value) => setActiveTab(value as GatewayDetailTab)} className="space-y-4">
          {/*
            Header card — the mock's gateway detail *page* header, re-measured
            2026-08-14. Reaching it means clicking the server *name* (an <a> in
            the row's first grid cell); clicking the row body opens a different
            surface, an inline expansion, whose chrome is not this one.

            Three bands, all ported: the title row (8px status dot with a 3px
            halo, 25px display name, neutral chips) with a right-aligned meta
            lane; the full-bleed stat strip, `2fr repeat(n, minmax(120px,1fr))`
            over a --gw0-0_30 wash; and the full-bleed tab bar. The visible tab
            contract now matches the mock exactly: Overview · Variables ·
            Catalog · Activity · Routes · Logs. Activity is the only fixture
            surface and carries explicit mock labeling; the other tabs map
            existing gateway state and controls into the reference structure.

            Deviation: the mock's header card is sticky (`top: -186px`), ours
            is not. The endpoint is not printed here because the mock does not
            print it here — it stays one click away in the meta lane's copy
            button (whose title is the endpoint), in the Runtime tab's
            Connection & Network card, and in the Config tab's client JSON.
          */}
          <DetailCard
            padding="16px 20px 0"
            style={{ borderRadius: 'var(--radius-3)', overflow: 'hidden' }}
          >
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0 flex-1">
                <div className="flex min-w-0 flex-wrap items-center gap-2.5">
                  <span
                    title={statusLabel}
                    aria-label={statusLabel}
                    style={{
                      width: 8,
                      height: 8,
                      flexShrink: 0,
                      borderRadius: 999,
                      background: statusDotColor,
                      boxShadow: `0 0 0 3px ${statusDotHalo}`,
                    }}
                  />
                  <h1
                    className="font-display break-words"
                    style={{
                      margin: 0,
                      fontSize: 25,
                      lineHeight: 1.1,
                      fontWeight: 800,
                      letterSpacing: '-0.01em',
                      color: 'var(--aurora-text-primary)',
                    }}
                  >
                    {gateway.name}
                  </h1>
                  {headerChips.map((chip) => (
                    <span
                      key={chip.label}
                      title={chip.title}
                      style={{
                        display: 'inline-flex',
                        alignItems: 'center',
                        gap: 4,
                        height: 20,
                        padding: '0 8px',
                        borderRadius: 999,
                        border:
                          '1px solid color-mix(in srgb, var(--aurora-border-strong) 80%, transparent)',
                        background: 'var(--gw0-0_48)',
                        color: 'var(--aurora-text-muted)',
                        fontSize: 9.5,
                        fontWeight: 650,
                        letterSpacing: '0.1em',
                        textTransform: 'uppercase',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {chip.label}
                    </span>
                  ))}
                </div>
              </div>

              {/* Meta lane — mock: transport, version, protocol, auth, status reason. */}
              <div className="min-w-0 shrink-0 pt-1">
                <div className="flex flex-wrap items-center justify-end gap-2.5 text-[11px] leading-none text-aurora-text-muted">
                  <HeaderMetaButton
                    onClick={async () => {
                      try {
                        await navigator.clipboard.writeText(endpointDisplay)
                        toast.success('Copied to clipboard')
                      } catch {
                        toast.error('Failed to copy to clipboard')
                      }
                    }}
                    aria-label="Copy command"
                    title={endpointDisplay}
                    style={{
                      color:
                        gateway.transport === 'http'
                          ? 'var(--aurora-accent-strong)'
                          : 'var(--aurora-text-muted)',
                    }}
                  >
                    {gateway.transport === 'http' ? <Globe size={15} /> : <Terminal size={15} />}
                  </HeaderMetaButton>
                  <HeaderMetaButton
                    onClick={handleCopyConfig}
                    aria-label="Copy client configuration"
                    title="Copy .mcp.json entry"
                  >
                    {configCopied ? <Check size={14} /> : <Braces size={14} />}
                  </HeaderMetaButton>
                  <HeaderMetaDot />
                  <span style={{ fontWeight: 650 }}>{transportLabel}</span>
                  <HeaderMetaDot />
                  {/*
                    The mock prints the upstream's serverInfo version and the
                    negotiated protocolVersion. The gateway API returns neither,
                    so both dash rather than being invented.
                  */}
                  <span
                    style={{ fontVariantNumeric: 'tabular-nums' }}
                    title="Upstream server version — not reported by the gateway API"
                  >
                    v {DETAIL_NO_DATA}
                  </span>
                  <HeaderMetaDot />
                  <span
                    className="inline-flex items-center gap-1.5"
                    style={{ fontVariantNumeric: 'tabular-nums' }}
                    title="Negotiated Model Context Protocol version — not reported by the gateway API"
                  >
                    <McpGlyph />
                    {DETAIL_NO_DATA}
                  </span>
                  {gateway.config.oauth_enabled ? (
                    <>
                      <HeaderMetaDot />
                      <DetailWarnPill
                        onClick={() => setActiveTab('routes')}
                        aria-label="OAuth"
                        title="This server authenticates with OAuth"
                        style={{ height: 22, padding: '0 8px', borderRadius: 7 }}
                      >
                        <Lock size={11} />
                        OAuth
                      </DetailWarnPill>
                    </>
                  ) : null}
                  {gateway.warnings.length > 0 ? (
                    <>
                      <HeaderMetaDot />
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <DetailWarnPill
                            onClick={() => setActiveTab('logs')}
                            aria-label={`Open warnings (${gateway.warnings.length})`}
                            style={{ height: 22, padding: '0 8px', borderRadius: 7 }}
                          >
                            <AlertTriangle size={11} />
                            {gateway.warnings.length}
                          </DetailWarnPill>
                        </TooltipTrigger>
                        <TooltipContent side="bottom" className="max-w-xs">
                          {gateway.warnings[0].message}
                          {gateway.warnings.length > 1 && (
                            <span className="block mt-1 text-xs opacity-70">
                              +{gateway.warnings.length - 1} more — see Warnings tab
                            </span>
                          )}
                        </TooltipContent>
                      </Tooltip>
                    </>
                  ) : null}
                  {gateway.status.last_error ? (
                    <>
                      <HeaderMetaDot />
                      <span
                        title={gateway.status.last_error}
                        aria-label={`Status reason: ${gateway.status.last_error}`}
                        className="inline-grid place-items-center text-aurora-warn"
                        style={{ width: 18, height: 18, borderRadius: 5 }}
                      >
                        <AlertTriangle size={13} />
                      </span>
                    </>
                  ) : null}
                  <HeaderMetaDot />
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        className="inline-flex cursor-default items-center gap-1.5"
                        style={{ fontVariantNumeric: 'tabular-nums' }}
                        title={updatedAtLabel}
                        aria-label={`Last updated ${updatedAtLabel}`}
                      >
                        <Clock size={11} />
                        {updatedAtLabel}
                      </button>
                    </TooltipTrigger>
                    <TooltipContent side="left">{updatedAtLabel}</TooltipContent>
                  </Tooltip>
                </div>
              </div>
            </div>

            {/*
              Stat strip — attached to the bottom of the header card, above the
              tab bar: a wide Exposed cell followed by equal health cards, all
              full-bleed on a --gw0-0_30 wash.

              Only the Exposed cell and the stdio process cells have data
              behind them. The mock's Calls / Errors (and its Clients card on
              an HTTP server) have no counterpart in the gateway API, so they
              render an em-dash with the reason in their title rather than a
              fabricated number.
            */}
            <DetailStatStrip cardCount={stripCards.length}>
              <DetailExposureCell
                stats={exposureStats}
                onClick={() => setActiveTab('catalog')}
                showEnable={totalExposedPrimitives === 0 && totalDiscoveredPrimitives > 0}
                ariaLabel="Open catalog"
                title="Open catalog"
              />
              {stripCards.map((card) => (
                <DetailStripCard
                  key={card.label}
                  label={card.label}
                  value={card.value}
                  sub={card.sub}
                  title={card.title}
                />
              ))}
            </DetailStatStrip>

            {/*
              Tab bar. Mock geometry, measured off its live DOM: a 6px-topped
              full-bleed row over a 55%-blended hairline, holding a 2px-gap
              scroller of 34px tabs (13px side padding, 12.5px/650) with a 2px
              bottom indicator. Idle is muted with a transparent indicator;
              active is --aurora-accent-strong over --aurora-accent-primary.
              Counts ride in a 17px chip that re-tones with the tab.

              The mock also parks a 12-icon capability cluster at the right end
              of this row, toned "supported" / "not advertised" from the
              server's `initialize` response. The gateway API reports no
              capability set, so every icon renders in the third,
              explicitly-unknown state — dashed, unfilled, behind a leading `—`
              — rather than borrowing the mock's dimmed "not advertised" tone,
              which would assert something we never asked the server. See
              `DetailCapabilityCluster`.
            */}
            <DetailTabBar>
              <DetailTabsList aria-label="Server detail sections">
                <DetailTabTrigger
                  value="overview"
                  active={activeTab === 'overview'}
                  label={GATEWAY_DETAIL_TAB_LABELS.overview}
                />
                <DetailTabTrigger
                  value="variables"
                  active={activeTab === 'variables'}
                  label={GATEWAY_DETAIL_TAB_LABELS.variables}
                />
                <DetailTabTrigger
                  value="catalog"
                  active={activeTab === 'catalog'}
                  label={GATEWAY_DETAIL_TAB_LABELS.catalog}
                  count={
                    gateway.discovery.tools.length +
                    gateway.discovery.resources.length +
                    gateway.discovery.prompts.length
                  }
                />
                <DetailTabTrigger
                  value="activity"
                  active={activeTab === 'activity'}
                  label={GATEWAY_DETAIL_TAB_LABELS.activity}
                  count={<span className="inline-flex items-center gap-1">1K <span className="text-[7px] tracking-wider">MOCK</span></span>}
                />
                <DetailTabTrigger
                  value="routes"
                  active={activeTab === 'routes'}
                  label={GATEWAY_DETAIL_TAB_LABELS.routes}
                  count={3 + surfaceEntries.length}
                />
                <DetailTabTrigger
                  value="logs"
                  active={activeTab === 'logs'}
                  tone={gateway.warnings.length > 0 ? 'warn' : 'default'}
                  label={GATEWAY_DETAIL_TAB_LABELS.logs}
                  count={gateway.warnings.length > 0 ? gateway.warnings.length : undefined}
                />
              </DetailTabsList>
              <DetailCapabilityCluster />
            </DetailTabBar>
          </DetailCard>

          {/* Tab content */}
          <TabsContent value="catalog">
            <div className="space-y-4">
              <DetailCard padding="14px 20px 16px">
                <div className="space-y-3">
                  <div className="relative">
                    <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
                    <input
                      value={inventorySearch}
                      onChange={(event) => setInventorySearch(event.target.value)}
                      placeholder="Search tools, resources, and prompts..."
                      name="catalog-search"
                      aria-label="Search tools, resources, and prompts"
                      className="flex h-10 w-full rounded-md border bg-aurora-page-bg px-3 py-1 pl-9 text-sm shadow-xs outline-none transition-colors focus-visible:border-[var(--aurora-accent-primary)] focus-visible:ring-[3px] focus-visible:ring-[var(--aurora-accent-primary)]/34"
                    />
                  </div>
                  <div className="flex flex-wrap items-center gap-1.5">
                    {([
                      ['tools', toolsTabLabel, Wrench, gateway.discovery.tools.length],
                      ['resources', 'Resources', FileText, gateway.discovery.resources.length],
                      ['prompts', 'Prompts', MessageSquare, gateway.discovery.prompts.length],
                    ] as const).map(([value, label, Icon, count]) => (
                      <button
                        key={value}
                        type="button"
                        // Toggle: clicking an already-selected chip returns to the 'all' view.
                        onClick={() => setInventoryFilter(inventoryFilter === value ? 'all' : value)}
                        className={cn(
                          'inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1.5 text-[13px] font-medium transition-colors',
                          inventoryFilter === value
                            ? 'border-aurora-accent-primary/28 bg-[linear-gradient(180deg,rgba(16,35,48,0.96),rgba(11,25,35,0.98))] text-aurora-text-primary shadow-[var(--aurora-active-glow)]'
                            : 'border-aurora-border-strong bg-aurora-page-bg text-aurora-text-muted',
                        )}
                        aria-pressed={inventoryFilter === value}
                        aria-label={label}
                        title={label}
                      >
                        <Icon className="size-3.5" />
                        <Badge variant="secondary" className="rounded-full px-2 py-0.5 text-[11px]">{count}</Badge>
                      </button>
                    ))}
                    {inventoryFilter === 'tools' ? (
                      <Button
                        type="button"
                        variant="outline"
                        size="icon"
                        onClick={() => setManageToolsMode((current) => !current)}
                        className={cn(
                          'size-8 rounded-full',
                          manageToolsMode
                            ? 'border-aurora-accent-primary/28 bg-[linear-gradient(180deg,rgba(16,35,48,0.96),rgba(11,25,35,0.98))] text-aurora-text-primary shadow-[var(--aurora-active-glow)]'
                            : 'border-aurora-border-strong bg-aurora-page-bg text-aurora-text-muted',
                        )}
                        aria-pressed={manageToolsMode}
                        aria-label="Manage tools"
                        title="Manage tools"
                      >
                        <SlidersHorizontal className="size-3.5" />
                      </Button>
                    ) : null}
                    {gateway.warnings.length > 0 ? (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            type="button"
                            variant="outline"
                            size="icon"
                            onClick={() => setActiveTab('logs')}
                            className="size-8 rounded-full border-aurora-warn/30 bg-aurora-warn/10 text-aurora-warn hover:bg-aurora-warn/14 hover:text-aurora-warn"
                            aria-label={`Open warnings (${gateway.warnings.length})`}
                            title={`Open warnings (${gateway.warnings.length})`}
                          >
                            <AlertTriangle className="size-3.5" />
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent side="bottom" className="max-w-xs">
                          {gateway.warnings[0].message}
                          {gateway.warnings.length > 1 && (
                            <span className="block mt-1 text-xs opacity-70">+{gateway.warnings.length - 1} more — see Warnings tab</span>
                          )}
                        </TooltipContent>
                      </Tooltip>
                    ) : null}
                  </div>
                </div>
              </DetailCard>

              {(inventoryFilter === 'all' || inventoryFilter === 'tools') ? (
                <DetailCard padding="14px 20px 16px">
                  <div className="mb-4 flex items-center gap-2">
                    <Wrench className="size-4 text-aurora-text-muted" />
                    <h2 className="text-lg font-semibold">{toolsTabLabel}</h2>
                  </div>
                  <ToolExposureTable
                    tools={displayedTools}
                    exposureLabel={exposureSummary.label}
                    exposeAll={exposeAllTools}
                    manageMode={manageToolsMode}
                    hasDraftChanges={hasDraftChanges}
                    isSaving={isSavingExposure}
                    selectedRowToolNames={selectedRowToolNames}
                    currentExposedToolNames={currentExposedToolNames}
                    draftSelectedToolNames={draftSelectedToolNames}
                    saveErrorMessage={exposureSaveError}
                    onExposeAllChange={handleExposeAllChange}
                    onManageModeChange={setManageToolsMode}
                    onRowSelectionChange={setSelectedRowToolNames}
                    onBulkEnableSelected={handleBulkEnableSelected}
                    onBulkDisableSelected={handleBulkDisableSelected}
                    onSaveChanges={handleSaveExposureDraft}
                    onCancelChanges={handleCancelExposureDraft}
                    searchValue={inventorySearch}
                    onSearchValueChange={setInventorySearch}
                    hideSearchAndFilterControls
                    hideManageModeToggle
                  />
                </DetailCard>
              ) : null}

              {(inventoryFilter === 'all' || inventoryFilter === 'resources') ? (
                <PrimitiveExposureTable
                  title="Discovered MCP Resources"
                  description="Search and manage which upstream resources are exposed through this server."
                  searchPlaceholder="Search resources"
                  manageLabel="Manage resources"
                  emptyLabel="No resources discovered"
                  exposureEnabled={resourceExposureEnabled}
                  icon={FileText}
                  items={gateway.discovery.resources.map((resource) => ({
                    name: resource.name,
                    description: resource.description,
                    secondary: resource.uri,
                    exposed: resource.exposed ?? false,
                  }))}
                  searchValue={inventorySearch}
                  onSearchValueChange={setInventorySearch}
                  onSaveSelection={async (selectedNames) => {
                    try {
                      await updateGateway(gateway.id, {
                        config: {
                          expose_resources: selectedNames,
                        },
                      })
                      toast.success('Resource exposure updated.')
                    } catch (error) {
                      toast.error(getErrorMessage(error, 'Failed to update resource exposure'))
                      throw error
                    }
                  }}
                />
              ) : null}

              {(inventoryFilter === 'all' || inventoryFilter === 'prompts') ? (
                <PrimitiveExposureTable
                  title="Discovered MCP Prompts"
                  description="Search and manage which upstream prompts are exposed through this server."
                  searchPlaceholder="Search prompts"
                  manageLabel="Manage prompts"
                  emptyLabel="No prompts discovered"
                  exposureEnabled={promptExposureEnabled}
                  icon={MessageSquare}
                  items={gateway.discovery.prompts.map((prompt) => ({
                    name: prompt.name,
                    description: prompt.description,
                    secondary:
                      prompt.arguments && prompt.arguments.length > 0
                        ? `${prompt.arguments.length} arg${prompt.arguments.length === 1 ? '' : 's'}`
                        : undefined,
                    exposed: prompt.exposed ?? false,
                  }))}
                  searchValue={inventorySearch}
                  onSearchValueChange={setInventorySearch}
                  onSaveSelection={async (selectedNames) => {
                    try {
                      await updateGateway(gateway.id, {
                        config: {
                          expose_prompts: selectedNames,
                        },
                      })
                      toast.success('Prompt exposure updated.')
                    } catch (error) {
                      toast.error(getErrorMessage(error, 'Failed to update prompt exposure'))
                      throw error
                    }
                  }}
                />
              ) : null}
            </div>
          </TabsContent>

          <TabsContent value="activity">
            <DetailCard
              padding="0"
              data-mock-region="gateway-detail-activity"
              aria-label="Gateway activity mock data"
            >
              <div className="flex flex-wrap items-center gap-3 border-b border-aurora-border-default/55 px-5 py-3">
                <Activity className="size-4 text-aurora-accent-pink" />
                <div>
                  <h2 className="text-sm font-semibold text-aurora-text-primary">Recent Calls</h2>
                  <p className="mt-0.5 text-[10.5px] text-aurora-text-muted">Illustrative until per-server call history is available.</p>
                </div>
                <MockSurfaceBadge className="ml-auto" />
              </div>
              <div className="aurora-scrollbar overflow-x-auto">
                <div className="min-w-[650px]">
                  <div className="grid grid-cols-[72px_minmax(150px,1fr)_minmax(150px,1fr)_70px_58px] gap-3 border-b border-aurora-border-default/45 px-5 py-2 text-[9px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
                    <span>When</span><span>Tool</span><span>Client</span><span>Latency</span><span>Result</span>
                  </div>
                  {[
                    ['4m ago', 'list_items_3', 'claude-code · jmagar', '99ms', 'OK'],
                    ['12m ago', 'get_item_3', 'claude-desktop · jmagar', '91ms', 'OK'],
                    ['20m ago', 'search_3', 'axon-runner', '16ms', 'OK'],
                    ['23m ago', 'create_item_3', 'codex · jmagar', '251ms', 'Error'],
                    ['31m ago', 'list_items', 'claude-code · jmagar', '111ms', 'OK'],
                  ].map(([when, tool, client, latency, result]) => (
                    <div key={`${when}-${tool}`} className="grid grid-cols-[72px_minmax(150px,1fr)_minmax(150px,1fr)_70px_58px] gap-3 border-t border-aurora-border-default/40 px-5 py-3 first:border-t-0">
                      <span className="text-[10px] text-aurora-text-muted">{when}</span>
                      <span className="font-mono text-[10.5px] font-semibold text-aurora-text-primary">{tool}</span>
                      <span className="text-[10.5px] text-aurora-text-muted">{client}</span>
                      <span className="font-mono text-[10px] text-aurora-text-secondary">{latency}</span>
                      <span className={result === 'OK' ? 'text-[10px] font-semibold text-aurora-success' : 'text-[10px] font-semibold text-aurora-error'}>{result}</span>
                    </div>
                  ))}
                </div>
              </div>
            </DetailCard>
          </TabsContent>

          <TabsContent value="variables">
            <DetailCard padding="16px 20px 18px">
              <div className="mb-4">
                <h2 className="text-lg font-semibold">Variables &amp; Client Configuration</h2>
                <p className="text-sm text-aurora-text-muted mt-1">
                  Add this JSON block to your MCP client configuration to connect to this server.
                </p>
              </div>
              <DetailInset style={{ padding: 0, overflow: 'hidden' }}>
                <div
                  className="flex items-center justify-between px-4 py-3"
                  style={{
                    borderBottom:
                      '1px solid color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))',
                  }}
                >
                  <DetailMicroLabel>Client JSON</DetailMicroLabel>
                  <DetailIconButton
                    onClick={handleCopyConfig}
                    aria-label="Copy client JSON"
                    title="Copy to clipboard"
                  >
                    {configCopied ? <Check size={14} /> : <Copy size={14} />}
                  </DetailIconButton>
                </div>
                <pre className="aurora-scrollbar overflow-x-auto whitespace-pre-wrap break-all px-4 py-4 text-sm leading-6 text-aurora-text-primary">
                  <code>{clientConfigJson}</code>
                </pre>
              </DetailInset>
            </DetailCard>
          </TabsContent>

          <TabsContent value="routes">
            <DetailCard padding="16px 20px 18px" className="space-y-5">
              <div>
                <h2 className="text-lg font-semibold">Routes &amp; Exposure</h2>
                <p className="mt-1 text-sm text-aurora-text-muted">
                  Control server availability, exposure of MCP resources and prompts, and individual lab surface toggles.
                </p>
              </div>

              <div className="grid gap-5 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
                <DetailInset style={{ padding: 16 }}>
                  <div className="flex items-center gap-2">
                    <Power className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Server state</h3>
                  </div>
                  <div className="mt-4">
                    <GatewayEnabledSetting
                      enabled={gateway.enabled ?? true}
                      onEnable={handleEnableGateway}
                      onDisable={handleDisableGateway}
                    />
                  </div>
                </DetailInset>

                <DetailInset style={{ padding: 16 }}>
                  <div className="flex items-center gap-2">
                    <Settings className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Exposure surfaces</h3>
                  </div>
                  <div className="mt-4 space-y-3">
                    <SettingRow
                      title="Expose resources"
                      description="Allow discovered MCP resources to be exposed through this server."
                      checked={resourceExposureEnabled}
                      onCheckedChange={handleProxyResourcesToggle}
                    />
                    <SettingRow
                      title="Expose prompts"
                      description="Allow discovered MCP prompts to be exposed through this server."
                      checked={promptExposureEnabled}
                      onCheckedChange={handleProxyPromptsToggle}
                    />
                    {!isLabGateway ? (
                      <>
                        <SettingRow
                          title="Trust Agent Skills"
                          description="Opt in to aggregating Skills from this upstream. Skill instructions can direct agent behavior."
                          checked={skillExposureEnabled}
                          onCheckedChange={handleProxySkillsToggle}
                        />
                        <div className="rounded-lg border bg-aurora-control-surface/10 p-4">
                          <div className="flex flex-wrap items-center gap-2">
                            <BookOpen className="size-4 text-aurora-text-muted" />
                            <p className="text-sm font-semibold text-aurora-text-primary">Skill exposure patterns</p>
                            <Badge variant="outline">{skillSupportLabel}</Badge>
                            <Badge variant={skillExposureEnabled ? 'secondary' : 'outline'}>
                              {skillExposureEnabled ? 'trusted' : 'not trusted'}
                            </Badge>
                            <div className="flex-1" />
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              onClick={() => router.push(`/skills?upstream=${encodeURIComponent(gateway.name)}`)}
                            >
                              View Skills
                            </Button>
                          </div>
                          <p className="mt-1 text-sm text-aurora-text-muted">
                            {gateway.config.expose_skills == null
                              ? 'All validated skills are eligible for exposure.'
                              : gateway.config.expose_skills.length === 0
                                ? 'No skills are currently exposed.'
                                : `${gateway.config.expose_skills.length} exposure pattern${gateway.config.expose_skills.length === 1 ? '' : 's'} configured.`}
                            {' '}Use the Skills catalog to manage individual skills; wildcard patterns remain available in Edit Server.
                          </p>
                        </div>
                      </>
                    ) : null}
                  </div>
                </DetailInset>
              </div>

              {surfaceEntries.length > 0 ? (
                <DetailInset style={{ padding: 16 }}>
                  <div className="flex items-center gap-2">
                    <Wrench className="size-4 text-aurora-text-muted" />
                    <h3 className="text-sm font-semibold text-aurora-text-primary">Lab surfaces</h3>
                  </div>
                  <div className="mt-4 grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                    {surfaceEntries.map(([surface, state]) => (
                      <div key={surface} className="flex items-start justify-between gap-4 rounded-lg border bg-aurora-control-surface/10 p-4">
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <span
                              className={`size-2 rounded-full ${state.connected ? 'bg-aurora-success' : 'bg-aurora-error'}`}
                              aria-hidden="true"
                            />
                            <p className="text-sm font-semibold uppercase text-aurora-text-primary">{surface}</p>
                          </div>
                          <p className="mt-1 text-sm text-aurora-text-muted">
                            {state.connected ? 'Connected and reachable.' : 'Configured but not currently connected.'}
                          </p>
                        </div>
                        <Switch
                          aria-label={`${surface.toUpperCase()} surface`}
                          checked={state.enabled}
                          onCheckedChange={(enabled) => handleSurfaceToggle(surface, enabled)}
                        />
                      </div>
                    ))}
                  </div>
                </DetailInset>
              ) : null}
            </DetailCard>
          </TabsContent>

          <TabsContent value="overview">
            <DetailCard padding="16px 20px 18px" className="space-y-5">
              <div>
                <h2 className="text-lg font-semibold">Overview</h2>
                <p className="text-sm text-aurora-text-muted mt-1">
                  Live process metadata comes from the active server pool. If the server restarted, orphaned upstream
                  processes are reconciled from the persisted runtime snapshot and shown here as stale runtime state.
                </p>
              </div>

              <div style={DETAIL_STAT_GRID_STYLE}>
                <DetailStatCard
                  icon={<Network size={11} />}
                  label="Connection"
                  value={gateway.status.connected ? 'Connected' : 'Not connected'}
                  sub={gateway.enabled ?? false ? 'Server enabled' : 'Server disabled'}
                />
                <DetailStatCard
                  icon={<Cpu size={11} />}
                  label="Process"
                  value={gateway.status.pid ? `pid ${gateway.status.pid}` : 'No active pid'}
                  sub={gateway.status.pgid ? `pgid ${gateway.status.pgid}` : 'No process group recorded'}
                />
                <DetailStatCard
                  icon={<Clock size={11} />}
                  label="Process age"
                  value={runtimeAgeLabel ?? 'Unknown'}
                  sub="upstream process start time"
                />
                <DetailStatCard
                  icon={<AlertTriangle size={11} />}
                  label="Stale processes"
                  value={gateway.status.likely_stale_count ?? 0}
                  sub="orphaned runtimes after reconciliation"
                />
              </div>

              {/*
                The mock's detail panel carries a live-telemetry row
                (CPU / RAM / STORAGE / NETWORK) and CLIENTS · 24H /
                TOP TOOLS · 24H / CALLS · 24H panels. The gateway API exposes
                none of those, so every value dashes rather than being faked.
              */}
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Activity className="size-4 text-aurora-text-muted" />
                  <h3 className="text-sm font-semibold text-aurora-text-primary">Live telemetry</h3>
                  <span className="text-xs text-aurora-text-muted">
                    Not reported by the gateway API yet
                  </span>
                </div>
                <div style={DETAIL_STAT_GRID_STYLE}>
                  <DetailStatCard
                    icon={<Cpu size={11} />}
                    label="CPU"
                    value={DETAIL_NO_DATA}
                    sub="proxy share"
                    title="CPU — gateway time spent proxying this server. Not reported by the gateway API."
                  />
                  <DetailStatCard
                    icon={<MemoryStick size={11} />}
                    label="RAM"
                    value={DETAIL_NO_DATA}
                    sub="buffers + session state"
                    title="Memory. Not reported by the gateway API."
                  />
                  <DetailStatCard
                    icon={<HardDrive size={11} />}
                    label="Storage"
                    value={DETAIL_NO_DATA}
                    sub="runtime · logs + cache"
                    title="Disk used by runtime snapshots, logs, and cache for this server. Not reported by the gateway API."
                  />
                  <DetailStatCard
                    icon={<Network size={11} />}
                    label="Network"
                    value={DETAIL_NO_DATA}
                    sub="ingress / egress"
                    title="Network — ingress / egress through the gateway. Not reported by the gateway API."
                  />
                </div>
                <div style={DETAIL_PANEL_GRID_STYLE}>
                  <DetailMiniList
                    label="Clients · 24h"
                    rows={[]}
                    title="Per-client call counts. Not reported by the gateway API."
                  />
                  <DetailMiniList
                    label="Top tools · 24h"
                    rows={[]}
                    title="Per-tool call counts. Not reported by the gateway API."
                  />
                  <DetailMiniList
                    label="Calls · 24h"
                    rows={[]}
                    title="Call volume over the last 24h. Not reported by the gateway API."
                  />
                </div>
              </div>

              {!isLabGateway ? (
                <div className="flex flex-wrap items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(false, false)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && !isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <RefreshCw className="size-4 mr-2" />
                    )}
                    Cleanup runtime
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(false, true)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && !isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <Search className="size-4 mr-2" />
                    )}
                    Preview cleanup
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(true, false)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <AlertTriangle className="size-4 mr-2" />
                    )}
                    Aggressive cleanup
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => handleCleanupRuntime(true, true)}
                    disabled={isCleaningRuntime}
                  >
                    {isCleaningRuntime && isAggressiveCleanup ? (
                      <Loader2 className="size-4 mr-2 animate-spin" />
                    ) : (
                      <Search className="size-4 mr-2" />
                    )}
                    Preview aggressive cleanup
                  </Button>
                </div>
              ) : null}

              {/*
                Mock: the detail Overview tab pairs a transport-conditional
                "Process & Storage" / "Connection & Network" card with a
                "Server Metadata" card, both in the same panel-medium chrome
                with an uppercase header band and baseline-aligned rows. Ported
                here with our fields; catalog exposure is not repeated, it now
                lives in the header strip.
              */}
              <div style={DETAIL_KV_GRID_STYLE}>
                <DetailKeyValueCard
                  label={gateway.transport === 'stdio' ? 'Process & storage' : 'Connection & network'}
                  rows={runtimeFactRows}
                />
                <DetailKeyValueCard label="Server metadata" rows={serverMetadataRows} />
              </div>

              <ul className="space-y-2 text-sm text-aurora-text-muted">
                <li>Active runtime metadata is recorded when the server spawns stdio upstreams.</li>
                <li>Runtime snapshots are written to disk beside the server config and survive server restarts.</li>
                <li>Dead PIDs are pruned during runtime reconciliation; surviving non-current PIDs count as stale runtime state.</li>
              </ul>
            </DetailCard>
          </TabsContent>

          <TabsContent value="logs">
            {gateway.warnings.length > 0 ? (
              <DetailCard padding="16px 20px 18px">
                <h2 className="text-lg font-semibold mb-4">Server Warnings</h2>
                <div className="space-y-2">
                  {gateway.warnings.map((warning, index) => (
                    <div
                      key={index}
                      className="flex items-start gap-3 rounded-lg border border-aurora-warn/20 bg-aurora-warn/5 p-4"
                    >
                      <AlertTriangle className="size-4 text-aurora-warn mt-0.5 shrink-0" />
                      <div className="flex-1">
                        <p className="text-sm font-medium text-aurora-warn">
                          {warning.code}
                        </p>
                        <p className="text-sm text-aurora-text-muted mt-0.5">{warning.message}</p>
                        <p className="text-xs text-aurora-text-muted mt-2">
                          {formatGatewayTimestamp(warning.timestamp)}
                        </p>
                      </div>
                    </div>
                  ))}
                </div>
              </DetailCard>
            ) : (
              <DetailCard padding="16px 20px 18px">
                <div className="flex items-start gap-3">
                  <span className="mt-1 size-2 shrink-0 rounded-full bg-aurora-success" />
                  <div>
                    <h2 className="text-lg font-semibold">No reported warnings</h2>
                    <p className="mt-1 text-sm text-aurora-text-muted">
                      This server has no current warning entries. Use “View in Logs” for the connected server-log stream.
                    </p>
                  </div>
                </div>
              </DetailCard>
            )}
          </TabsContent>
        </Tabs>
      </div>

      {/* Dialogs */}
      {editOpen && (
        <GatewayFormDialog
          open
          onOpenChange={setEditOpen}
          gateway={gateway}
          onSave={handleSave}
        />
      )}

      <TestResultPanel
        result={testResult}
        onClose={() => setTestResult(null)}
      />
      <CleanupResultPanel
        result={cleanupResult}
        onClose={() => setCleanupResult(null)}
      />
      <ActionConfirmationDialog
        open={removeConfirmationOpen}
        title={REMOVE_GATEWAY_TITLE}
        description={removeGatewayDescription(gateway.name)}
        confirmLabel={REMOVE_GATEWAY_CONFIRM_LABEL}
        onOpenChange={setRemoveConfirmationOpen}
        onConfirm={confirmDelete}
      />
    </>
  )
}
