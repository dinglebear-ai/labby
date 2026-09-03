'use client'

import { useEffect, useState, type ReactNode } from 'react'
import Link from 'next/link'
import { useRouter } from 'next/navigation'
import dynamic from 'next/dynamic'
import { ArrowDown, ArrowRight, ArrowUp, Cable, Clock3, GripVertical } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { StatusBadge } from '@/components/gateway/status-badge'
import { TransportBadge } from '@/components/gateway/transport-badge'
import { OverviewHero } from '@/components/dashboard/overview-hero'
import {
  FanOutPanel,
  LeastUsedPanel,
  MostActivePanel,
} from '@/components/dashboard/activity-insight-panels'
import {
  CallOutcomesPanel,
  UpstreamsPanel,
} from '@/components/dashboard/analysis-panels'
import { DashboardPanel } from '@/components/dashboard/panel'
import { ErrorNotice } from '@/components/dashboard/error-notice'
import { WarningsBanner } from '@/components/dashboard/warnings-banner'
import { DASH_METRIC_SM, DASH_SURFACE } from '@/components/dashboard/ui'
import type { DrillTarget } from '@/components/dashboard/drill'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import { useGateways } from '@/lib/hooks/use-gateways'
import { useDashboardMetrics } from '@/lib/hooks/use-dashboard-metrics'
import {
  WINDOW_LABELS,
  buildLiveFleetStats,
  warningsSignature,
} from '@/lib/dashboard/dashboard-metrics'
import type { MetricsWindow } from '@/lib/types/metrics'
import { metricsLoadState } from '@/lib/dashboard/dashboard-load-state'
import { formatUiDate } from '@/lib/format-ui-time'
import { cn } from '@/lib/utils'
import {
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'

const ToolVolumeChart = dynamic(() =>
  import('@/components/dashboard/tool-volume-chart').then((module) => module.ToolVolumeChart),
)
const TopToolsChart = dynamic(() =>
  import('@/components/dashboard/top-tools-chart').then((module) => module.TopToolsChart),
)
const AnalysisSection = dynamic(() =>
  import('@/components/dashboard/analysis-section').then((module) => module.AnalysisSection),
)
const ToolDetailDrawer = dynamic(() =>
  import('@/components/dashboard/tool-detail-drawer').then((module) => module.ToolDetailDrawer),
)
const AgentDetailDrawer = dynamic(() =>
  import('@/components/dashboard/agent-detail-drawer').then((module) => module.AgentDetailDrawer),
)

function MetricsUnavailable({ message }: { message: string }) {
  return (
    <div className="flex h-[200px] items-center justify-center rounded-aurora-2 border border-dashed border-aurora-border-strong px-6 text-center text-sm text-aurora-text-muted">
      {message}
    </div>
  )
}

const OVERVIEW_ORDER_KEY = 'labby:overview-card-order:v1'

function ReorderableOverview({ cards }: { cards: Array<{ id: string; content: ReactNode; wide?: boolean }> }) {
  const ids = cards.map((card) => card.id)
  const [order, setOrder] = useState(ids)
  const [dragging, setDragging] = useState<string | null>(null)

  useEffect(() => {
    try {
      const saved = JSON.parse(window.localStorage.getItem(OVERVIEW_ORDER_KEY) ?? '[]') as string[]
      if (saved.length) setOrder([...saved.filter((id) => ids.includes(id)), ...ids.filter((id) => !saved.includes(id))])
    } catch { /* keep the default layout */ }
  // The card identities are stable for the lifetime of this dashboard.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const commit = (next: string[]) => {
    setOrder(next)
    window.localStorage.setItem(OVERVIEW_ORDER_KEY, JSON.stringify(next))
  }
  const move = (id: string, delta: number) => {
    const from = order.indexOf(id)
    const to = Math.max(0, Math.min(order.length - 1, from + delta))
    if (from === to) return
    const next = [...order]
    next.splice(to, 0, next.splice(from, 1)[0])
    commit(next)
  }
  const dropOn = (id: string) => {
    if (!dragging || dragging === id) return
    const next = order.filter((item) => item !== dragging)
    next.splice(next.indexOf(id), 0, dragging)
    commit(next)
    setDragging(null)
  }

  return <section aria-label="Customizable overview cards">
    <p className="mb-2 flex items-center gap-1.5 text-[10px] text-aurora-text-muted"><GripVertical className="size-3"/>Drag cards to arrange your overview. The order is saved on this device.</p>
    <div className="grid items-start gap-3 xl:grid-cols-2">
      {order.map((id) => {
        const card = cards.find((item) => item.id === id)
        if (!card) return null
        return <div key={id} draggable onDragStart={() => setDragging(id)} onDragEnd={() => setDragging(null)} onDragOver={(event) => event.preventDefault()} onDrop={() => dropOn(id)} className={cn('group relative cursor-grab rounded-aurora-2 outline-none active:cursor-grabbing', card.wide && 'xl:col-span-2', dragging === id && 'opacity-50')}>
          <div className="absolute right-3 top-2 z-10 flex items-center gap-0.5 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-strong/95 p-0.5 opacity-0 shadow-lg transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
            <GripVertical className="mx-1 size-3.5 text-aurora-text-muted" aria-hidden="true"/>
            <button type="button" onClick={() => move(id, -1)} className="rounded p-1 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" aria-label={`Move ${id} earlier`}><ArrowUp className="size-3"/></button>
            <button type="button" onClick={() => move(id, 1)} className="rounded p-1 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" aria-label={`Move ${id} later`}><ArrowDown className="size-3"/></button>
          </div>
          {card.content}
        </div>
      })}
    </div>
  </section>
}

export default function OverviewPage() {
  const router = useRouter()
  const { data: gateways, isLoading: gatewaysLoading } = useGateways()
  const [activeWindow, setActiveWindow] = useState<MetricsWindow>('24h')
  const [drill, setDrill] = useState<DrillTarget | null>(null)
  const {
    data: metrics,
    error: metricsError,
    isLoading: isMetricsLoading,
    mutate: reloadMetrics,
  } = useDashboardMetrics(activeWindow)

  const live = buildLiveFleetStats(gateways ?? [])
  const warningsSig = warningsSignature(gateways ?? [])
  const metricsState = metricsLoadState(metrics, metricsError, isMetricsLoading)
  const metricsLoading = metricsState === 'loading'
  const recentGateways = gateways?.slice(0, 5) ?? []

  // Stamp each successful metrics load so the hero can count "updated Ns ago".
  const [metricsLoadedAt, setMetricsLoadedAt] = useState(() => Date.now())
  useEffect(() => {
    if (metrics) setMetricsLoadedAt(Date.now())
  }, [metrics])

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Overview' }]} />

      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
        {/* Hero — eyebrow + pulse, title + heartbeat, trouble chips, window
            controls, and the welded stat strip / fleet-health squares. */}
        <OverviewHero
          gateways={gateways ?? []}
          live={live}
          metrics={metrics}
          activeWindow={activeWindow}
          onWindowChange={setActiveWindow}
          onRefresh={() => reloadMetrics()}
          loadedAt={metricsLoadedAt}
        />

        {/* Warnings banner (dismissable) */}
        {!gatewaysLoading && (
          <WarningsBanner count={live.warnings} signature={warningsSig} />
        )}

        {metricsState === 'unavailable' ? (
          <ErrorNotice message="Usage metrics aren't available on this Labby server yet." />
        ) : metricsState === 'error' ? (
          <ErrorNotice
            message="Couldn't load usage metrics for this window."
            onRetry={() => reloadMetrics()}
          />
        ) : null}

        {/* Primary telemetry uses the full canvas. The three compact breakdowns
            form a rail below it instead of leaving an empty right-hand column
            whenever the charts and fan-out insights are taller. */}
        <ReorderableOverview cards={[
          { id: 'Call volume', wide: true, content: <DashboardPanel title="Upstream call volume" meta={WINDOW_LABELS[activeWindow]}>
              {metrics ? (
                <ToolVolumeChart
                  data={metrics.timeseries}
                  window={activeWindow}
                  onSelectBucket={(from, to) => router.push(`/usage/?window=${activeWindow}&from=${Math.round(from)}&to=${Math.round(to)}`)}
                />
              ) : metricsLoading ? (
                <Skeleton className="h-[200px] w-full" />
              ) : (
                <MetricsUnavailable message="Upstream-call history is unavailable." />
              )}
            </DashboardPanel> },
          { id: 'Top targets', wide: true, content: <DashboardPanel title="Top targets">
              {metrics ? (
                <TopToolsChart
                  tools={metrics.tools.top}
                  onSelect={(name) => setDrill({ type: 'tool', name })}
                />
              ) : metricsLoading ? (
                <Skeleton className="h-[200px] w-full" />
              ) : (
                <MetricsUnavailable message="Target rankings are unavailable." />
              )}
            </DashboardPanel> },
          ...(metrics ? [
              { id: 'Code Mode fan-out', content: <FanOutPanel fanOut={metrics.fan_out} collected={metrics.collected.fan_out} /> },
              { id: 'Least used', content: <LeastUsedPanel
                  tools={metrics.tools.least}
                  distinct={metrics.tools.distinct}
                  onSelect={(name) => setDrill({ type: 'tool', name })}
                /> },
              { id: 'Most active', content: <MostActivePanel actors={metrics.actors} window={activeWindow} actorKindsCollected={metrics.collected.actor_kinds} onSelectActor={(entry) => setDrill({ type: 'agent', id: entry.id })}/> },
              { id: 'Upstreams', content: <UpstreamsPanel upstreams={metrics.upstreams} onSelect={(name) => router.push(`/usage/?window=${activeWindow}&upstream=${encodeURIComponent(name)}`)}/> },
              { id: 'Call outcomes', content: <CallOutcomesPanel toolCalls={metrics.tool_calls} errors={metrics.errors} window={activeWindow} onSelectOutcome={(outcome) => router.push(`/usage/?window=${activeWindow}&outcome=${outcome}`)} onSelectError={(kind) => router.push(`/usage/?window=${activeWindow}&outcome=failed&error=${encodeURIComponent(kind)}`)}/> },
            ] : metricsLoading ? (
              ['Code Mode fan-out','Least used','Most active','Upstreams','Call outcomes'].map((id) => ({ id, content: <Skeleton className="h-[176px] w-full rounded-aurora-2" /> }))
            ) : (
              ['Code Mode fan-out','Least used','Most active','Upstreams','Call outcomes'].map((id) => ({ id, content: <MetricsUnavailable message="Usage insights are unavailable." /> }))
            )),
        ]} />

        {/* ── Performance, cost & rhythm ─────────────────────────────── */}
        {metrics ? (
          <AnalysisSection
            metrics={metrics}
            onSelectTool={(name) => setDrill({ type: 'tool', name })}
            onOpenUsage={(query) => router.push(`/usage/?window=${activeWindow}&${query}`)}
          />
        ) : null}

        {/* ── Recent servers ─────────────────────────────────────────── */}
        <div>
          <div className="mb-4 flex items-center justify-between">
            <h2 className="text-lg font-semibold text-aurora-text-primary">Recent servers</h2>
            <Button variant="ghost" size="sm" asChild>
              <Link href="/gateways">
                View all
                <ArrowRight className="ml-1 size-4" />
              </Link>
            </Button>
          </div>

          {gatewaysLoading ? (
            <div className="space-y-2">
              {[1, 2, 3].map((i) => (
                <div
                  key={i}
                  className="flex items-center gap-4 rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium p-4"
                >
                  <Skeleton className="size-10 rounded-lg" />
                  <div className="flex-1">
                    <Skeleton className="mb-1 h-5 w-32" />
                    <Skeleton className="h-4 w-24" />
                  </div>
                  <Skeleton className="h-5 w-16" />
                </div>
              ))}
            </div>
          ) : recentGateways.length === 0 ? (
            <div className={cn(AURORA_STRONG_PANEL, DASH_SURFACE,'p-10 text-center')}>
              <div className="mx-auto mb-4 flex size-14 items-center justify-center rounded-full border border-aurora-border-strong bg-aurora-control-surface shadow-[0_8px_16px_rgba(0,0,0,0.16)]">
                <Cable className="size-7 text-aurora-accent-strong" />
              </div>
              <p className="text-lg font-semibold text-aurora-text-primary">No servers configured</p>
              <p className="mt-1 text-sm text-aurora-text-muted">
                Add your first MCP server to get started
              </p>
              <Button className="mt-5" asChild>
                <Link href="/gateways">Add server</Link>
              </Button>
            </div>
          ) : (
            <div className="space-y-2">
              {recentGateways.map((gateway) => (
                <Link
                  key={gateway.id}
                  href={gatewayDetailHref(gateway.id)}
                  className={cn(
                    'group flex flex-col gap-4 rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium p-4 transition-colors',
                    'hover:border-aurora-accent-primary/30 hover:bg-aurora-panel-strong',
                    'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-offset-2 focus-visible:ring-offset-aurora-page-bg',
                    'sm:flex-row sm:items-start',
                  )}
                >
                  <div
                    className={cn(
                      'flex size-10 shrink-0 items-center justify-center rounded-aurora-1 transition-colors',
                      gateway.status.healthy && gateway.status.connected
                        ? 'bg-aurora-accent-strong/15 text-aurora-accent-strong'
                        : 'bg-aurora-error/15 text-aurora-error',
                    )}
                  >
                    <Cable className="size-5" />
                  </div>
                  <div className="min-w-0 flex-1 space-y-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="truncate font-semibold text-aurora-text-primary transition-colors group-hover:text-aurora-accent-strong">
                        {gateway.name}
                      </p>
                      <StatusBadge healthy={gateway.status.healthy} connected={gateway.status.connected} />
                      <TransportBadge transport={gateway.transport} />
                    </div>
                    <p className="text-sm text-aurora-text-muted">
                      {gateway.status.discovered_tool_count} discovered tools,{' '}
                      {gateway.status.exposed_tool_count} exposed downstream
                    </p>
                    <div className="flex flex-wrap items-center gap-3 text-xs text-aurora-text-muted">
                      <span className="inline-flex items-center gap-1">
                        <Clock3 className="size-3.5" />
                        {formatUiDate(gateway.updated_at)}
                      </span>
                      {gateway.warnings.length > 0 && (
                        <span className="text-aurora-warn">
                          {gateway.warnings.length} warning{gateway.warnings.length === 1 ? '' : 's'}
                        </span>
                      )}
                    </div>
                  </div>
                  <div className="text-sm text-aurora-text-muted sm:text-right">
                    <span className={cn(DASH_METRIC_SM, 'block text-aurora-text-primary')}>
                      {gateway.status.exposed_tool_count}
                    </span>
                    exposed
                  </div>
                </Link>
              ))}
            </div>
          )}
        </div>
      </div>

      <ToolDetailDrawer
        tool={drill?.type === 'tool' ? drill.name : null}
        window={activeWindow}
        onClose={() => setDrill(null)}
        onDrill={setDrill}
      />
      <AgentDetailDrawer
        agentId={drill?.type === 'agent' ? drill.id : null}
        window={activeWindow}
        onClose={() => setDrill(null)}
        onDrill={setDrill}
      />
    </>
  )
}
