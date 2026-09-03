'use client'

import { Fragment, type CSSProperties, type ReactNode, useEffect, useMemo, useState } from 'react'
import Link from 'next/link'
import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  MoreHorizontal,
  Eye,
  Pencil,
  Play,
  Power,
  RefreshCw,
  Search,
  TriangleAlert,
  Trash2,
  Users,
  X,
  FileText,
  MessageSquare,
  BookOpen,
  Wrench,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import {
  REMOVE_GATEWAY_CONFIRM_LABEL,
  REMOVE_GATEWAY_TITLE,
  removeGatewayDescription,
} from './gateway-confirmations'
import { WarningsPill } from './warnings-pill'
import type { Gateway } from '@/lib/types/gateway'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import { buildGatewayEndpointPreview } from '@/lib/api/gateway-mobile'
import {
  AURORA_GATEWAY_DISABLED_ROW,
  gatewayActionTone,
  gatewayStatusTone,
} from './gateway-theme'

type SortKey = 'name' | 'endpoint' | 'exposed' | 'uptime'
type SortDirection = 'asc' | 'desc'
type StatusGroupId = 'attention' | 'healthy'

const GATEWAY_TABLE_BADGE =
  'inline-flex h-6 items-center rounded-full px-2 text-[10px] font-semibold uppercase tracking-[0.12em]'

/**
 * Gateway Console mock — measured off the repository-root
 * `Labby Gateway Console.html` reference.
 * Card, grid track list, header, group headers, and row chrome all mirror the
 * mock's computed styles.
 */
const GW_CARD =
  'overflow-hidden rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.05)]'

const GW_GRID =
  'grid grid-cols-[minmax(0,1fr)_80px_minmax(140px,300px)_210px_130px_18px] items-center'

/**
 * The `--gw*` scrim ramp carries underscores in its token names, which Tailwind
 * rewrites to spaces inside arbitrary values. Aliasing the ramp onto
 * underscore-free custom properties on the card keeps the utilities literal and
 * the values theme-reactive.
 */
const GW_SCRIM_ALIASES = {
  '--gw-head': 'var(--gw0-0_48)',
  '--gw-row': 'var(--gw1-0_62)',
  '--gw-row-hover': 'var(--gw3-0_75)',
  '--gw-group': 'var(--gw4-0_55)',
  '--gw-group-hover': 'var(--gw4-0_75)',
  '--gw-footer': 'var(--gw0-0_38)',
} as CSSProperties

const GW_HEAD_LABEL =
  'text-[10.5px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted'

const GW_ROW_ACTION =
  'grid size-5 shrink-0 cursor-pointer place-items-center rounded-md border-0 bg-transparent p-0 text-aurora-text-muted transition-colors hover:bg-aurora-hover-bg hover:text-aurora-text-primary disabled:cursor-default disabled:opacity-45'

/** `—` cells and zero counts: the mock's 45%-alpha muted tone. */
const GW_EMPTY_TONE = 'text-[color-mix(in_srgb,var(--aurora-text-muted)_45%,transparent)]'

const GW_EMPTY_RAIL = 'bg-[color-mix(in_srgb,var(--aurora-text-muted)_45%,transparent)]'

const GW_COUNT = 'inline-flex items-center gap-1 text-[12px] [font-weight:650] tabular-nums'

const EM_DASH = '—'

/**
 * Exposure tone, as measured on every mock row:
 * nothing discovered → muted dash, partial exposure → pink, full → primary.
 */
function exposureTone(exposed: number, discovered: number): string {
  if (discovered === 0) return GW_EMPTY_TONE
  if (exposed < discovered) return 'text-aurora-accent-pink'
  return 'text-aurora-text-primary'
}

/** A server is "needs attention" when it is enabled but not cleanly connected. */
function needsAttention(gateway: Gateway): boolean {
  if (!(gateway.enabled ?? true)) return false
  return !gateway.status.connected || !gateway.status.healthy || gateway.warnings.length > 0
}

function isStaleVirtualServer(gateway: Gateway): boolean {
  return gateway.source === 'in_process' && gateway.warnings.some((warning) => warning.code === 'unknown_service')
}

function canRemoveGateway(gateway: Gateway): boolean {
  return gateway.source !== 'in_process' || isStaleVirtualServer(gateway)
}

interface GatewayTableProps {
  gateways: Gateway[]
  density: 'comfortable' | 'condensed'
  presentation?: 'table' | 'cards' | 'list'
  cleanupSummaryByGatewayId?: Record<
    string,
    { preview?: { label: string; occurredAt: string }; cleanup?: { label: string; occurredAt: string } }
  >
  onEdit: (gateway: Gateway) => void
  onTest: (gateway: Gateway) => void
  onReload: (gateway: Gateway) => void
  onCleanup: (gateway: Gateway, aggressive: boolean, dryRun: boolean) => void
  onClearCleanupHistory: (gateway: Gateway) => void
  onToggleEnabled: (gateway: Gateway) => void
  onDelete: (gateway: Gateway) => void
}

function GatewayFact({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="min-w-0">
      <p className="text-[9px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted">{label}</p>
      <p className={cn('mt-1 truncate text-aurora-text-primary', mono && 'font-mono')} title={value}>
        {value}
      </p>
    </div>
  )
}

export function GatewayTable({
  gateways,
  density,
  presentation = 'table',
  cleanupSummaryByGatewayId = {},
  onEdit,
  onTest,
  onReload,
  onCleanup,
  onClearCleanupHistory,
  onToggleEnabled,
  onDelete,
}: GatewayTableProps) {
  const [loadingAction, setLoadingAction] = useState<{ id: string; action: string } | null>(null)
  const [sortKey, setSortKey] = useState<SortKey>('name')
  const [sortDirection, setSortDirection] = useState<SortDirection>('asc')
  const [copiedGatewayId, setCopiedGatewayId] = useState<string | null>(null)
  const [expandedMobileGatewayId, setExpandedMobileGatewayId] = useState<string | null>(null)
  const [expandedDesktopGatewayId, setExpandedDesktopGatewayId] = useState<string | null>(null)
  const [disableConfirmationGatewayId, setDisableConfirmationGatewayId] = useState<string | null>(null)
  const [removeConfirmationGatewayId, setRemoveConfirmationGatewayId] = useState<string | null>(null)
  const [collapsedGroups, setCollapsedGroups] = useState<StatusGroupId[]>([])
  const [attentionBannerDismissed, setAttentionBannerDismissed] = useState(false)
  const [selectedGatewayIds, setSelectedGatewayIds] = useState<string[]>([])
  const disableConfirmationGateway = disableConfirmationGatewayId
    ? gateways.find((gateway) => gateway.id === disableConfirmationGatewayId) ?? null
    : null
  const removeConfirmationGateway = removeConfirmationGatewayId
    ? gateways.find((gateway) => gateway.id === removeConfirmationGatewayId) ?? null
    : null

  // The dialog is gated on the *resolved* gateway, so a target that leaves the
  // list mid-confirmation closes it rather than asking the operator to confirm
  // a removal against a server we can no longer name. Closing is the safe half;
  // this reconciles the other half, since `onOpenChange` never fires in that
  // case and the stale id would otherwise sit set forever, blocking the next
  // confirmation for a different row.
  useEffect(() => {
    if (removeConfirmationGatewayId && !removeConfirmationGateway) {
      setRemoveConfirmationGatewayId(null)
    }
  }, [removeConfirmationGatewayId, removeConfirmationGateway])

  const requestToggleEnabled = (gateway: Gateway) => {
    if (gateway.enabled ?? true) {
      setDisableConfirmationGatewayId(gateway.id)
      return
    }
    onToggleEnabled(gateway)
  }

  const confirmDisableGateway = () => {
    const gateway = disableConfirmationGateway
    setDisableConfirmationGatewayId(null)
    if (!gateway || !(gateway.enabled ?? true)) return
    onToggleEnabled(gateway)
  }

  // gateway.remove is destructive (permanent, hard to recover); confirm
  // before it fires. removeVirtualServer (in_process "stale service" rows)
  // clears derived runtime bookkeeping, not persisted config, and is
  // classified non-destructive in the shared action catalog — so it skips
  // the extra step, matching the metadata instead of inventing a UI-only rule.
  const requestRemoveGateway = (gateway: Gateway) => {
    if (gateway.source === 'in_process') {
      onDelete(gateway)
      return
    }
    setRemoveConfirmationGatewayId(gateway.id)
  }

  const confirmRemoveGateway = () => {
    const gateway = removeConfirmationGateway
    setRemoveConfirmationGatewayId(null)
    if (!gateway) return
    onDelete(gateway)
  }

  const handleAction = async (
    gateway: Gateway,
    action: 'test' | 'reload',
    handler: (gateway: Gateway) => void | Promise<void>,
  ) => {
    setLoadingAction({ id: gateway.id, action })
    try {
      await handler(gateway)
    } finally {
      setLoadingAction(null)
    }
  }

  const isLoading = (id: string, action: string) => loadingAction?.id === id && loadingAction?.action === action

  const copyCommand = async (gateway: Gateway, value: string) => {
    try {
      await navigator.clipboard.writeText(value)
      setCopiedGatewayId(gateway.id)
      window.setTimeout(() => setCopiedGatewayId((current) => (current === gateway.id ? null : current)), 1200)
    } catch {
      // Clipboard failures should not block table use.
    }
  }

  const sortedGateways = useMemo(() => {
    const sorted = [...gateways].sort((left, right) => {
      let result = 0

      switch (sortKey) {
        case 'name':
          result = left.name.localeCompare(right.name, undefined, { sensitivity: 'base' })
          break
        case 'endpoint':
          result = buildGatewayEndpointPreview(left).localeCompare(
            buildGatewayEndpointPreview(right),
            undefined,
            { sensitivity: 'base' },
          )
          break
        case 'exposed':
          result =
            left.status.exposed_tool_count - right.status.exposed_tool_count ||
            left.status.exposed_resource_count - right.status.exposed_resource_count ||
            left.status.exposed_prompt_count - right.status.exposed_prompt_count ||
            (left.status.exposed_skill_count ?? 0) - (right.status.exposed_skill_count ?? 0)
          break
        case 'uptime':
          result = (left.status.age_seconds ?? -1) - (right.status.age_seconds ?? -1)
          break
      }

      if (result === 0) {
        result = left.name.localeCompare(right.name)
      }

      return sortDirection === 'asc' ? result : -result
    })

    return sorted
  }, [gateways, sortDirection, sortKey])

  const statusGroups = useMemo(() => {
    const attention = sortedGateways.filter(needsAttention)
    const healthy = sortedGateways.filter((gateway) => !needsAttention(gateway))

    return [
      { id: 'attention' as const, label: 'Needs attention', tone: 'text-aurora-error', rows: attention },
      { id: 'healthy' as const, label: 'Healthy', tone: 'text-aurora-success', rows: healthy },
    ].filter((group) => group.rows.length > 0)
  }, [sortedGateways])

  const attentionCount = useMemo(() => sortedGateways.filter(needsAttention).length, [sortedGateways])

  const exposureTotals = useMemo(
    () =>
      gateways.reduce(
        (totals, gateway) => ({
          exposed: totals.exposed + gateway.status.exposed_tool_count,
          discovered: totals.discovered + gateway.status.discovered_tool_count,
        }),
        { exposed: 0, discovered: 0 },
      ),
    [gateways],
  )

  const isGroupCollapsed = (id: StatusGroupId) => collapsedGroups.includes(id)

  const toggleGroup = (id: StatusGroupId) => {
    setCollapsedGroups((current) =>
      current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id],
    )
  }

  const toggleSelected = (id: string) => {
    setSelectedGatewayIds((current) =>
      current.includes(id) ? current.filter((entry) => entry !== id) : [...current, id],
    )
  }

  const handleSort = (nextKey: SortKey) => {
    if (sortKey === nextKey) {
      setSortDirection((current) => (current === 'asc' ? 'desc' : 'asc'))
      return
    }

    setSortKey(nextKey)
    setSortDirection(nextKey === 'exposed' || nextKey === 'uptime' ? 'desc' : 'asc')
  }

  const SortHeader = ({
    label,
    sort,
    align = 'center',
  }: {
    label: string
    sort: SortKey
    align?: 'start' | 'center'
  }) => (
    <button
      type="button"
      data-sorthead="1"
      onClick={() => handleSort(sort)}
      className={cn(
        GW_HEAD_LABEL,
        'inline-flex cursor-pointer items-center gap-[5px] whitespace-nowrap transition-colors hover:text-aurora-text-primary',
        align === 'start' ? 'justify-self-start p-0' : 'justify-self-center rounded-md px-1 py-0.5',
      )}
      aria-label={`Sort by ${label.toLowerCase()}`}
      aria-sort={sortKey === sort ? (sortDirection === 'asc' ? 'ascending' : 'descending') : 'none'}
    >
      <span>{label}</span>
      {sortKey === sort ? (
        <span className="text-[10px]" aria-hidden="true">{sortDirection === 'asc' ? '↑' : '↓'}</span>
      ) : (
        <span data-ghost="1" className="text-[10px]" aria-hidden="true">↓</span>
      )}
    </button>
  )

  /** Clients have no field on `Gateway`, so that header remains static. */
  const StaticHeader = ({ label, title }: { label: string; title: string }) => (
    <span className={cn(GW_HEAD_LABEL, 'justify-self-center whitespace-nowrap')} title={title}>
      {label}
    </span>
  )

  const formatRuntimeAge = (ageSeconds?: number) => {
    if (!ageSeconds || ageSeconds < 0) return null
    if (ageSeconds < 60) return `${ageSeconds}s old`
    if (ageSeconds < 3600) return `${Math.floor(ageSeconds / 60)}m old`
    if (ageSeconds < 86400) return `${Math.floor(ageSeconds / 3600)}h old`
    return `${Math.floor(ageSeconds / 86400)}d old`
  }

  const runtimeAgeLabel = (gateway: Gateway) => formatRuntimeAge(gateway.status.age_seconds)

  const formatHistoryTime = (occurredAt: string) => {
    const date = new Date(occurredAt)
    if (Number.isNaN(date.getTime())) return null
    return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  }

  const cleanupBadgeLabel = (
    entry: { label: string; occurredAt: string } | undefined,
    prefix: string,
  ) => {
    if (!entry) return null
    const time = formatHistoryTime(entry.occurredAt)
    return time ? `${prefix} ${time}` : prefix
  }

  const runtimeDetailsTitle = (gateway: Gateway) => {
    const owner = gateway.status.owner
    const lines = [
      owner ? `Owner surface: ${owner.surface}` : null,
      owner?.client_name ? `Owner client: ${owner.client_name}` : null,
      owner?.subject ? `Owner subject: ${owner.subject}` : null,
      owner?.request_id ? `Owner request: ${owner.request_id}` : null,
      owner?.session_id ? `Owner session: ${owner.session_id}` : null,
      gateway.status.origin ? `Origin: ${gateway.status.origin}` : null,
      gateway.status.runtime_state_path ? `Runtime snapshot: ${gateway.status.runtime_state_path}` : null,
      gateway.status.reconciled_at ? `Reconciled: ${gateway.status.reconciled_at}` : null,
    ].filter(Boolean)

    return lines.length > 0 ? lines.join('\n') : undefined
  }

  const runtimeBadges = (gateway: Gateway) => {
    const badges: ReactNode[] = []
    const detailsTitle = runtimeDetailsTitle(gateway)

    if ((gateway.status.likely_stale_count ?? 0) > 0) {
      badges.push(
        <Badge
          key="stale"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_12%,transparent)] text-aurora-warn')}
        >
          {gateway.status.likely_stale_count} stale
        </Badge>,
      )
    }

    if (gateway.status.pid) {
      badges.push(
        <Badge
          key="pid"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-border-strong bg-[rgba(7,17,26,0.48)] font-mono text-aurora-text-muted')}
        >
          pid {gateway.status.pid}
        </Badge>,
      )
    }

    if (gateway.status.pgid && gateway.status.pgid !== gateway.status.pid) {
      badges.push(
        <Badge
          key="pgid"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-border-strong bg-[rgba(7,17,26,0.48)] font-mono text-aurora-text-muted')}
        >
          pgid {gateway.status.pgid}
        </Badge>,
      )
    }

    const age = runtimeAgeLabel(gateway)
    if (age) {
      badges.push(
        <Badge
          key="age"
          title={detailsTitle}
          className={cn(GATEWAY_TABLE_BADGE, 'border border-aurora-border-strong bg-[rgba(7,17,26,0.48)] text-aurora-text-muted')}
        >
          {age}
        </Badge>,
      )
    }

    return badges
  }

  const statusRailClass = (gateway: Gateway) => {
    if (!(gateway.enabled ?? true)) return GW_EMPTY_RAIL
    if (gateway.status.healthy && gateway.status.connected && gateway.warnings.length === 0) return 'bg-aurora-accent-strong'
    if (!gateway.status.connected) return 'bg-aurora-error'
    return 'bg-aurora-warn'
  }

  /** One desktop row, laid out on the mock's six-track grid. */
  const renderDesktopRow = (gateway: Gateway, stripeIndex = 0) => {
    const supportsProbeControls = gateway.source !== 'in_process'
    const canRemoveGatewayRow = canRemoveGateway(gateway)
    const endpointPreview = buildGatewayEndpointPreview(gateway)
    const showsCommandLine = gateway.transport === 'stdio'
    const isDisabled = !(gateway.enabled ?? true)
    const statusTone = gatewayStatusTone(gateway.status.healthy, gateway.status.connected)
    const runtimeChips = runtimeBadges(gateway)
    const cleanupSummary = cleanupSummaryByGatewayId[gateway.id]
    const cleanupBadge = cleanupBadgeLabel(cleanupSummary?.cleanup, 'cleaned')
    const previewBadge = cleanupBadgeLabel(cleanupSummary?.preview, 'preview')
    const isSelected = selectedGatewayIds.includes(gateway.id)
    const status = gateway.status
    const discoveredSkills = status.discovered_skill_count ?? 0
    const exposedSkills = status.exposed_skill_count ?? 0
    const toggleLabel = gateway.enabled ?? true ? 'Disable server' : 'Enable server'
    const isExpanded = expandedDesktopGatewayId === gateway.id

    return (
      <Fragment key={gateway.id}>
        <div
          data-gwrow="1"
          data-hoverrow="1"
          className={cn(
            GW_GRID,
            'group relative border-t border-[color-mix(in_srgb,var(--aurora-border-default)_55%,var(--aurora-page-bg))] transition-[background-color,box-shadow] duration-150 hover:bg-[color-mix(in_srgb,var(--aurora-accent-primary)_7%,var(--gw-row-hover))]',
            stripeIndex % 2 === 0 ? 'bg-[var(--gw-row)]' : 'bg-[color-mix(in_srgb,var(--aurora-panel-medium)_48%,var(--gw-row))]',
            density === 'condensed' ? 'py-[7px]' : 'py-[11px]',
            isDisabled && 'text-aurora-text-muted',
          )}
        >
        <span
          className={cn('absolute inset-y-0 left-0 w-[3px]', statusRailClass(gateway))}
          aria-hidden="true"
        />

        <div className="min-w-0 pl-5">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <button
              type="button"
              className={cn(GW_ROW_ACTION, 'size-[18px]')}
              onClick={() => setExpandedDesktopGatewayId((current) => current === gateway.id ? null : gateway.id)}
              aria-expanded={isExpanded}
              aria-label={`${isExpanded ? 'Collapse' : 'Expand'} ${gateway.name} details`}
            >
              <ChevronRight className={cn('size-3 transition-transform', isExpanded && 'rotate-90')} aria-hidden="true" />
            </button>
            <button
              type="button"
              role="checkbox"
              aria-checked={isSelected}
              aria-label={`Select ${gateway.name}`}
              onClick={() => toggleSelected(gateway.id)}
              className={cn(
                'grid size-[14px] shrink-0 cursor-pointer place-items-center rounded-[4px] border p-0 transition-[border-color,background-color]',
                isSelected
                  ? 'border-aurora-accent-primary/60 bg-aurora-accent-primary/20'
                  : 'border-[color-mix(in_srgb,var(--aurora-border-strong)_85%,transparent)] bg-[var(--gw-head)] hover:border-aurora-accent-primary/45',
              )}
            >
              {isSelected ? (
                <Check className="size-2.5 text-aurora-accent-strong" aria-hidden="true" />
              ) : null}
            </button>
            <Link
              href={gatewayDetailHref(gateway.id)}
              title={statusTone.label}
              className="min-w-0 max-w-full break-words font-display text-[13.5px] leading-[1.16] [font-weight:760] text-aurora-text-primary underline-offset-4 hover:text-aurora-accent-strong hover:underline"
            >
              {gateway.name}
            </Link>
            {isDisabled ? (
              <Badge
                className={cn(
                  GATEWAY_TABLE_BADGE,
                  'border border-aurora-border-strong bg-[var(--gw-head)] text-aurora-text-muted',
                )}
              >
                Disabled
              </Badge>
            ) : null}
            <WarningsPill warnings={gateway.warnings} />
            {runtimeChips}
            {cleanupSummary?.cleanup && cleanupBadge ? (
              <Badge
                className={cn(
                  GATEWAY_TABLE_BADGE,
                  'border border-aurora-success/30 bg-[color-mix(in_srgb,var(--aurora-success)_12%,transparent)] text-aurora-success',
                )}
                title={`${cleanupSummary.cleanup.label}\n${cleanupSummary.cleanup.occurredAt}`}
              >
                {cleanupBadge}
              </Badge>
            ) : null}
            {cleanupSummary?.preview && previewBadge ? (
              <Badge
                className={cn(
                  GATEWAY_TABLE_BADGE,
                  'border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 text-aurora-accent-strong',
                )}
                title={`${cleanupSummary.preview.label}\n${cleanupSummary.preview.occurredAt}`}
              >
                {previewBadge}
              </Badge>
            ) : null}

            <span data-hoverreveal="1" className="inline-flex items-center gap-0.5">
              {density === 'comfortable' ? (
                <button
                  type="button"
                  className={GW_ROW_ACTION}
                  onClick={() => requestToggleEnabled(gateway)}
                  title={toggleLabel}
                >
                  <Power className="size-[11px]" aria-hidden="true" />
                  <span className="sr-only">{toggleLabel}</span>
                </button>
              ) : null}
              {supportsProbeControls && density === 'comfortable' ? (
                <button
                  type="button"
                  className={GW_ROW_ACTION}
                  onClick={() => handleAction(gateway, 'test', onTest)}
                  disabled={isLoading(gateway.id, 'test')}
                  title="Test connection"
                >
                  <Play
                    className={cn('size-[11px]', isLoading(gateway.id, 'test') && 'animate-pulse')}
                    aria-hidden="true"
                  />
                  <span className="sr-only">Test connection</span>
                </button>
              ) : null}
              {supportsProbeControls && density === 'comfortable' ? (
                <button
                  type="button"
                  className={GW_ROW_ACTION}
                  onClick={() => handleAction(gateway, 'reload', onReload)}
                  disabled={isLoading(gateway.id, 'reload')}
                  title="Reload server"
                >
                  <RefreshCw
                    className={cn('size-[11px]', isLoading(gateway.id, 'reload') && 'animate-spin')}
                    aria-hidden="true"
                  />
                  <span className="sr-only">Reload server</span>
                </button>
              ) : null}
              {isStaleVirtualServer(gateway) && density === 'comfortable' ? (
                <button
                  type="button"
                  className={cn(GW_ROW_ACTION, 'text-aurora-error hover:text-aurora-error')}
                  onClick={() => requestRemoveGateway(gateway)}
                  title="Remove stale service"
                >
                  <Trash2 className="size-[11px]" aria-hidden="true" />
                  <span className="sr-only">Remove stale service</span>
                </button>
              ) : null}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button type="button" className={GW_ROW_ACTION}>
                    <MoreHorizontal className="size-[11px]" aria-hidden="true" />
                    <span className="sr-only">More actions</span>
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem asChild>
                    <Link href={gatewayDetailHref(gateway.id)}>
                      <Eye className="mr-2 size-4" />
                      View details
                    </Link>
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={() => onEdit(gateway)}>
                    <Pencil className="mr-2 size-4" />
                    Edit server
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={() => requestToggleEnabled(gateway)}>
                    {gateway.enabled ?? true ? (
                      <>
                        <Trash2 className="mr-2 size-4" />
                        Disable server
                      </>
                    ) : (
                      <>
                        <Play className="mr-2 size-4" />
                        Enable server
                      </>
                    )}
                  </DropdownMenuItem>
                  {supportsProbeControls ? (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem onClick={() => onTest(gateway)}>
                        <Play className="mr-2 size-4" />
                        Test connection
                      </DropdownMenuItem>
                      <DropdownMenuItem onClick={() => onReload(gateway)}>
                        <RefreshCw className="mr-2 size-4" />
                        Reload server
                      </DropdownMenuItem>
                      <DropdownMenuItem onClick={() => onCleanup(gateway, false, true)}>
                        <Search className="mr-2 size-4" />
                        Preview cleanup
                      </DropdownMenuItem>
                      <DropdownMenuItem onClick={() => onCleanup(gateway, false, false)}>
                        <Wrench className="mr-2 size-4" />
                        Cleanup runtime
                      </DropdownMenuItem>
                      <DropdownMenuItem onClick={() => onCleanup(gateway, true, true)}>
                        <Search className="mr-2 size-4" />
                        Preview aggressive cleanup
                      </DropdownMenuItem>
                      <DropdownMenuItem onClick={() => onCleanup(gateway, true, false)}>
                        <TriangleAlert className="mr-2 size-4" />
                        Aggressive cleanup
                      </DropdownMenuItem>
                      {cleanupSummary ? (
                        <>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem onClick={() => onClearCleanupHistory(gateway)}>
                            <Trash2 className="mr-2 size-4" />
                            Clear cleanup history
                          </DropdownMenuItem>
                        </>
                      ) : null}
                    </>
                  ) : null}
                  {canRemoveGatewayRow ? (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem
                        onClick={() => requestRemoveGateway(gateway)}
                        className="text-destructive focus:text-destructive"
                      >
                        <Trash2 className="mr-2 size-4" />
                        {gateway.source === 'in_process' ? 'Remove stale service' : 'Remove server'}
                      </DropdownMenuItem>
                    </>
                  ) : null}
                </DropdownMenuContent>
              </DropdownMenu>
            </span>
          </div>
        </div>

        {/* Clients — the Gateway API reports no client attribution, so the mock's
            own "no data" treatment (dimmed em dash) applies to every row. */}
        <div className="min-w-0 justify-self-center">
          <span
            className={cn(GW_COUNT, 'gap-[5px]', GW_EMPTY_TONE)}
            title="Connected clients are not reported by the gateway API"
          >
            <Users className="size-[11px] shrink-0 opacity-65" aria-hidden="true" />
            <span className="sr-only">Clients:</span>
            {EM_DASH}
          </span>
        </div>

        <div className="min-w-0 max-w-full justify-self-center px-2.5">
          <button
            type="button"
            onClick={() => copyCommand(gateway, endpointPreview)}
            title={endpointPreview}
            aria-label={`Copy ${gateway.name} ${showsCommandLine ? 'command' : 'endpoint'}`}
            className={cn(
              'block max-w-full cursor-pointer truncate rounded-md px-1.5 py-0.5 text-[10.5px] transition-colors hover:bg-aurora-hover-bg hover:text-aurora-accent-strong',
              copiedGatewayId === gateway.id
                ? 'text-aurora-accent-strong'
                : 'text-[color-mix(in_srgb,var(--aurora-text-muted)_85%,transparent)]',
            )}
          >
            {endpointPreview}
          </button>
        </div>

        <div className="min-w-0 justify-self-center">
          <span
            className="grid grid-cols-[40px_40px_40px_40px] items-center gap-x-1.5"
            title={`Exposed — tools ${status.exposed_tool_count}/${status.discovered_tool_count} · resources ${status.exposed_resource_count}/${status.discovered_resource_count} · prompts ${status.exposed_prompt_count}/${status.discovered_prompt_count} · skills ${exposedSkills}/${discoveredSkills}`}
          >
            <span
              className={cn(
                GW_COUNT,
                exposureTone(status.exposed_tool_count, status.discovered_tool_count),
              )}
            >
              <Wrench className="size-[11px] shrink-0 opacity-65" aria-hidden="true" />
              <span className="sr-only">Tools:</span>
              {status.discovered_tool_count === 0 ? EM_DASH : status.exposed_tool_count}
            </span>
            <span
              className={cn(
                GW_COUNT,
                exposureTone(status.exposed_resource_count, status.discovered_resource_count),
              )}
            >
              <FileText className="size-[11px] shrink-0 opacity-65" aria-hidden="true" />
              <span className="sr-only">Resources:</span>
              {status.discovered_resource_count === 0 ? EM_DASH : status.exposed_resource_count}
            </span>
            <span
              className={cn(
                GW_COUNT,
                exposureTone(status.exposed_prompt_count, status.discovered_prompt_count),
              )}
            >
              <MessageSquare className="size-[11px] shrink-0 opacity-65" aria-hidden="true" />
              <span className="sr-only">Prompts:</span>
              {status.discovered_prompt_count === 0 ? EM_DASH : status.exposed_prompt_count}
            </span>
            <span
              className={cn(
                GW_COUNT,
                exposureTone(exposedSkills, discoveredSkills),
              )}
            >
              <BookOpen className="size-[11px] shrink-0 opacity-65" aria-hidden="true" />
              <span className="sr-only">Skills:</span>
              {discoveredSkills === 0 ? EM_DASH : exposedSkills}
            </span>
          </span>
        </div>

        {/* Runtime age is the uptime value currently reported by the gateway API. */}
        <div className="min-w-0 justify-self-center">
          <span
            className={cn('text-[10.5px] [font-weight:650] tabular-nums', GW_EMPTY_TONE)}
            title={runtimeAgeLabel(gateway) ?? 'Runtime uptime is not reported by this server'}
          >
            <span className="sr-only">Uptime:</span>
            {runtimeAgeLabel(gateway)?.replace(' old', '') ?? EM_DASH}
          </span>
        </div>
        </div>
        {isExpanded ? (
          <div className="border-t border-aurora-border-strong bg-[color-mix(in_srgb,var(--aurora-accent-primary)_4%,var(--gw-head))] px-10 py-4">
            <div className="grid gap-4 text-[11px] sm:grid-cols-2 xl:grid-cols-4">
              <GatewayFact label="Transport" value={gateway.transport} />
              <GatewayFact label="Endpoint" value={endpointPreview} mono />
              <GatewayFact label="Runtime" value={runtimeAgeLabel(gateway) ?? 'Not reported'} />
              <GatewayFact label="Process" value={status.pid ? `PID ${status.pid}${status.pgid ? ` · PGID ${status.pgid}` : ''}` : 'Not reported'} />
              <GatewayFact label="Origin" value={status.origin ?? gateway.source ?? 'Not reported'} />
              <GatewayFact label="Owner" value={status.owner?.client_name ?? status.owner?.subject ?? status.owner?.surface ?? 'Not reported'} />
              <GatewayFact label="Resources" value={`${status.exposed_resource_count}/${status.discovered_resource_count} exposed`} />
              <GatewayFact label="Prompts" value={`${status.exposed_prompt_count}/${status.discovered_prompt_count} exposed`} />
            </div>
            <div className="mt-4 flex items-center justify-between gap-3 border-t border-aurora-border-subtle pt-3">
              <p className="text-[10px] text-aurora-text-muted">
                CPU, memory, client history, and uptime are not reported by the gateway API.
              </p>
              <Button asChild variant="outline" size="sm" className="h-8 shrink-0">
                <Link href={gatewayDetailHref(gateway.id)}>Open server page</Link>
              </Button>
            </div>
          </div>
        ) : null}
      </Fragment>
    )
  }
  return (
    <>
      <section
        aria-label="Server inventory cards"
        className={cn(
          'space-y-2 min-[1101px]:hidden',
          presentation === 'cards' && 'min-[1101px]:grid min-[1101px]:grid-cols-2 min-[1101px]:gap-3 min-[1101px]:space-y-0 min-[1450px]:grid-cols-3',
        )}
      >
        {sortedGateways.map((gateway) => {
          const supportsProbeControls = gateway.source !== 'in_process'
          const canRemoveGatewayRow = canRemoveGateway(gateway)
          const isDisabled = !(gateway.enabled ?? true)
          const statusTone = gatewayStatusTone(gateway.status.healthy, gateway.status.connected)
          const endpointPreview = buildGatewayEndpointPreview(gateway)
          const showsCommandLine = gateway.transport === 'stdio'
          const isExpanded = expandedMobileGatewayId === gateway.id
          const envCount = Object.keys(gateway.config.env ?? {}).length
          const runtimeLabel = runtimeAgeLabel(gateway) ?? 'live'
          const cleanupSummary = cleanupSummaryByGatewayId[gateway.id]
          const cleanupSummaryLabel =
            cleanupBadgeLabel(cleanupSummary?.cleanup, 'cleaned') ??
            cleanupBadgeLabel(cleanupSummary?.preview, 'preview')
          const counts = [
            { label: 'Tools', icon: Wrench, exposed: gateway.status.exposed_tool_count, discovered: gateway.status.discovered_tool_count },
            { label: 'Resources', icon: FileText, exposed: gateway.status.exposed_resource_count, discovered: gateway.status.discovered_resource_count },
            { label: 'Prompts', icon: MessageSquare, exposed: gateway.status.exposed_prompt_count, discovered: gateway.status.discovered_prompt_count },
            { label: 'Skills', icon: BookOpen, exposed: gateway.status.exposed_skill_count ?? 0, discovered: gateway.status.discovered_skill_count ?? 0 },
          ]

          return (
            <article
              key={gateway.id}
              className={cn(
                'relative overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium),inset_0_1px_0_rgba(255,255,255,0.035)]',
                isDisabled && AURORA_GATEWAY_DISABLED_ROW,
              )}
            >
              <span className={cn('absolute inset-y-0 left-0 w-[3px]', statusRailClass(gateway))} aria-hidden="true" />
              <div className="space-y-3 p-3 pl-4">
                <div className="flex min-w-0 items-start justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex min-w-0 flex-wrap items-center gap-2">
                      <Link
                        href={gatewayDetailHref(gateway.id)}
                        className="min-w-0 truncate font-display text-[15px] font-bold leading-tight text-aurora-text-primary underline-offset-4 hover:text-aurora-accent-strong hover:underline"
                      >
                        {gateway.name}
                      </Link>
                      <span className={cn('inline-flex min-h-6 items-center gap-1.5 rounded-full border px-2 text-[10px] font-semibold', statusTone.text)}>
                        <span className={cn('size-1.5 rounded-full', statusTone.dot)} aria-hidden="true" />
                        {statusTone.label}
                      </span>
                      {isDisabled ? <span className="inline-flex min-h-6 items-center rounded-full border border-aurora-border-strong px-2 text-[10px] font-semibold text-aurora-text-muted">Disabled</span> : null}
                    </div>
                    <button
                      type="button"
                      className={cn(
                        'mt-1.5 flex min-h-8 w-full min-w-0 items-center gap-1.5 rounded-md text-left text-[11px] text-aurora-text-muted',
                        showsCommandLine && 'hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
                      )}
                      onClick={() => showsCommandLine && setExpandedMobileGatewayId((current) => current === gateway.id ? null : gateway.id)}
                      aria-expanded={showsCommandLine ? isExpanded : undefined}
                      title={endpointPreview}
                    >
                      <span className="min-w-0 flex-1 truncate font-mono">{endpointPreview}</span>
                      {showsCommandLine ? <ChevronDown className={cn('size-3.5 shrink-0 transition-transform', isExpanded && 'rotate-180')} /> : null}
                    </button>
                  </div>

                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="outline" size="icon" className={cn(gatewayActionTone(), 'size-10 shrink-0 rounded-full')} aria-label={`More actions for ${gateway.name}`}>
                        <MoreHorizontal className="size-4" />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="min-w-56">
                      <DropdownMenuItem asChild><Link href={gatewayDetailHref(gateway.id)}><Eye className="mr-2 size-4" />View details</Link></DropdownMenuItem>
                      <DropdownMenuItem onClick={() => onEdit(gateway)}><Pencil className="mr-2 size-4" />Edit server</DropdownMenuItem>
                      <DropdownMenuItem onClick={() => requestToggleEnabled(gateway)}>
                        {gateway.enabled ?? true ? <><Power className="mr-2 size-4" />Disable server</> : <><Play className="mr-2 size-4" />Enable server</>}
                      </DropdownMenuItem>
                      {supportsProbeControls ? (
                        <>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem onClick={() => onTest(gateway)}><Play className="mr-2 size-4" />Test connection</DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onReload(gateway)}><RefreshCw className="mr-2 size-4" />Reload server</DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, false, true)}><Search className="mr-2 size-4" />Preview cleanup</DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, false, false)}><Wrench className="mr-2 size-4" />Cleanup runtime</DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, true, true)}><Search className="mr-2 size-4" />Preview aggressive cleanup</DropdownMenuItem>
                          <DropdownMenuItem onClick={() => onCleanup(gateway, true, false)}><TriangleAlert className="mr-2 size-4" />Aggressive cleanup</DropdownMenuItem>
                          {cleanupSummary ? <><DropdownMenuSeparator /><DropdownMenuItem onClick={() => onClearCleanupHistory(gateway)}><Trash2 className="mr-2 size-4" />Clear cleanup history</DropdownMenuItem></> : null}
                        </>
                      ) : null}
                      {canRemoveGatewayRow ? <><DropdownMenuSeparator /><DropdownMenuItem onClick={() => requestRemoveGateway(gateway)} className="text-aurora-error focus:text-aurora-error"><Trash2 className="mr-2 size-4" />{gateway.source === 'in_process' ? 'Remove stale service' : 'Remove server'}</DropdownMenuItem></> : null}
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>

                {showsCommandLine && isExpanded ? (
                  <div className="rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface/55 p-2.5">
                    <div className="flex items-start gap-2">
                      <code className="min-w-0 flex-1 break-all font-mono text-[11px] leading-5 text-aurora-text-primary">{endpointPreview}</code>
                      <Button variant="outline" size="icon" className="size-9 shrink-0" onClick={() => copyCommand(gateway, endpointPreview)} aria-label={`Copy ${gateway.name} command`}>
                        {copiedGatewayId === gateway.id ? <Check className="size-4" /> : <Copy className="size-4" />}
                      </Button>
                    </div>
                    <p className="mt-1 text-[10px] uppercase tracking-[0.1em] text-aurora-text-muted">{envCount > 0 ? `${envCount} env vars` : 'No env vars'}</p>
                  </div>
                ) : null}

                <div className="grid grid-cols-2 gap-2">
                  {counts.map(({ label, icon: Icon, exposed, discovered }) => (
                    <div key={label} data-mobile-metric={label.toLowerCase()} className="rounded-lg border border-aurora-border-subtle bg-aurora-control-surface/15 px-2.5 py-2">
                      <div className="flex items-center gap-1.5 text-[10px] uppercase tracking-[0.09em] text-aurora-text-muted"><Icon className="size-3.5" />{label}</div>
                      <div className="mt-1 font-display text-[16px] font-bold tabular-nums text-aurora-text-primary">
                        {exposed}<span className="ml-1 text-[10px] font-medium text-aurora-text-muted">/ {discovered}</span>
                      </div>
                    </div>
                  ))}
                </div>

                <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-aurora-border-subtle pt-2 text-[10px] text-aurora-text-muted">
                  <span data-mobile-metric="runtime">Runtime <strong className="font-semibold text-aurora-text-primary">{runtimeLabel}</strong></span>
                  {cleanupSummaryLabel ? <span>· {cleanupSummaryLabel}</span> : null}
                  {gateway.warnings.length > 0 ? <span className="text-aurora-warn">· {gateway.warnings.length} warning{gateway.warnings.length === 1 ? '' : 's'}</span> : null}
                </div>

                <div className="grid grid-cols-3 gap-2">
                  <Button asChild variant="outline" size="sm" className="min-h-10"><Link href={gatewayDetailHref(gateway.id)}><Eye className="mr-1.5 size-4" />Open</Link></Button>
                  {supportsProbeControls ? <Button variant="outline" size="sm" className="min-h-10" onClick={() => onTest(gateway)}><Play className="mr-1.5 size-4" />Test</Button> : <Button variant="outline" size="sm" className="min-h-10" onClick={() => onEdit(gateway)}><Pencil className="mr-1.5 size-4" />Edit</Button>}
                  {supportsProbeControls ? <Button variant="outline" size="sm" className="min-h-10" onClick={() => onReload(gateway)}><RefreshCw className="mr-1.5 size-4" />Reload</Button> : <Button variant="outline" size="sm" className="min-h-10" onClick={() => requestToggleEnabled(gateway)}>{gateway.enabled ?? true ? 'Disable' : 'Enable'}</Button>}
                </div>
              </div>
            </article>
          )
        })}
      </section>

      {/* Desktop — the Gateway Console mock's grid table. Every value below was
          measured off the mock's computed styles; see
          docs/gateway-console-mock-alignment.md. */}
      <section
        aria-label="Server inventory"
        data-hovercard="1"
        className={cn(GW_CARD, 'hidden', presentation !== 'cards' && 'min-[1101px]:block')}
        style={GW_SCRIM_ALIASES}
      >
        <div
          data-gwtablewrap="1"
          className="aurora-scrollbar overflow-x-auto min-[1101px]:overflow-x-visible"
        >
          <div data-gwtable="1" className="min-w-[1010px]">
            <div
              data-gwhead="1"
              className={cn(
                GW_GRID,
                'sticky top-0 z-[18] h-10 border-b border-aurora-border-strong bg-[var(--gw-head)] pl-5',
              )}
            >
              <SortHeader label="Server" sort="name" align="start" />
              <StaticHeader
                label="Clients"
                title="Connected clients are not reported by the gateway API"
              />
              <SortHeader label="Endpoint" sort="endpoint" />
              <SortHeader label="Exposed" sort="exposed" />
              <SortHeader label="Uptime" sort="uptime" />
            </div>

            {attentionCount > 0 && !attentionBannerDismissed ? (
              <div className="flex items-center gap-2 border-b border-[color-mix(in_srgb,var(--aurora-error)_22%,var(--aurora-border-default))] bg-[color-mix(in_srgb,var(--aurora-error)_6%,var(--gw-head))] px-5 py-1.5 transition-colors hover:bg-[color-mix(in_srgb,var(--aurora-error)_10%,var(--gw-head))]">
                <button
                  type="button"
                  onClick={() => toggleGroup('attention')}
                  aria-expanded={!isGroupCollapsed('attention')}
                  className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 text-left"
                >
                  <TriangleAlert className="size-3 shrink-0 text-aurora-error" aria-hidden="true" />
                  <span className="text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-error">
                    Needs attention
                  </span>
                  <span className="text-[10.5px] tabular-nums text-aurora-text-muted">
                    {attentionCount} {attentionCount === 1 ? 'server' : 'servers'}
                  </span>
                  <ChevronRight
                    className={cn(
                      'size-[11px] shrink-0 text-aurora-text-muted transition-transform',
                      !isGroupCollapsed('attention') && 'rotate-90',
                    )}
                    aria-hidden="true"
                  />
                </button>
                <button
                  type="button"
                  onClick={() => setAttentionBannerDismissed(true)}
                  className={cn(GW_ROW_ACTION, 'size-[22px] rounded-[7px]')}
                  aria-label="Dismiss"
                  title="Dismiss until something new needs attention"
                >
                  <X className="size-3" aria-hidden="true" />
                </button>
              </div>
            ) : null}

            {statusGroups.map((group) => {
              const expanded = !isGroupCollapsed(group.id)

              return (
                <Fragment key={group.id}>
                  <button
                    type="button"
                    onClick={() => toggleGroup(group.id)}
                    aria-expanded={expanded}
                    className="flex w-full cursor-pointer items-center gap-2 border-b border-[color-mix(in_srgb,var(--aurora-border-default)_55%,var(--aurora-page-bg))] bg-[var(--gw-group)] px-5 pt-[5px] pb-1 text-left transition-colors hover:bg-[var(--gw-group-hover)]"
                  >
                    <ChevronRight
                      className={cn(
                        'size-2.5 shrink-0 text-aurora-text-muted transition-transform',
                        expanded && 'rotate-90',
                      )}
                      aria-hidden="true"
                    />
                    <span className={cn('text-[9px] font-bold uppercase tracking-[0.16em]', group.tone)}>
                      {group.label}
                    </span>
                    <span className="text-[9.5px] tabular-nums text-[color-mix(in_srgb,var(--aurora-text-muted)_65%,transparent)]">
                      {group.rows.length}
                    </span>
                    <span
                      aria-hidden="true"
                      className="h-px flex-1 bg-[color-mix(in_srgb,var(--aurora-border-default)_40%,var(--aurora-page-bg))]"
                    />
                  </button>
                  {expanded ? group.rows.map((gateway, index) => renderDesktopRow(gateway, index)) : null}
                </Fragment>
              )
            })}
          </div>
        </div>
        <div className="flex items-center justify-between gap-3 border-t border-[color-mix(in_srgb,var(--aurora-border-default)_70%,var(--aurora-page-bg))] bg-[var(--gw-footer)] px-5 py-[9px]">
          <span className="text-[11.5px] tabular-nums text-aurora-text-muted">
            {gateways.length} {gateways.length === 1 ? 'server' : 'servers'} ·{' '}
            {exposureTotals.exposed}/{exposureTotals.discovered} tools
            {selectedGatewayIds.length > 0 ? ` · ${selectedGatewayIds.length} selected` : ''}
          </span>
        </div>
      </section>
      <ActionConfirmationDialog
        open={disableConfirmationGatewayId !== null}
        title="Disable server?"
        description="Connected clients should no longer have access to this server. Existing sessions may fail until the gateway is enabled again."
        confirmLabel="Disable server"
        onOpenChange={(open) => {
          if (!open) setDisableConfirmationGatewayId(null)
        }}
        onConfirm={confirmDisableGateway}
      />
      {/* Mounted only with a resolved target, so the description always has a
          real name to use. `?? ''` here would have contradicted
          `removeGatewayDescription`'s own contract, which deliberately has no
          "this server" fallback — an unnamed delete confirmation is exactly
          what that helper exists to prevent. */}
      {removeConfirmationGateway !== null && (
        <ActionConfirmationDialog
          open
          title={REMOVE_GATEWAY_TITLE}
          description={removeGatewayDescription(removeConfirmationGateway.name)}
          confirmLabel={REMOVE_GATEWAY_CONFIRM_LABEL}
          onOpenChange={(open) => {
            if (!open) setRemoveConfirmationGatewayId(null)
          }}
          onConfirm={confirmRemoveGateway}
        />
      )}
    </>
  )
}
