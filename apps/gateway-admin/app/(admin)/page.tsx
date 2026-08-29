'use client'

import { useEffect, useState } from 'react'
import Link from 'next/link'
import { useRouter } from 'next/navigation'
import dynamic from 'next/dynamic'
import { ArrowRight, Cable, Clock3, Cpu, HardDrive, Network, Server, Users } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'
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

function MockOverviewReferencePanels() {
  return (
    <>
      <div data-mock-region="overview-connected-clients" aria-label="Connected Clients mock data">
        <DashboardPanel
          title="Connected Clients"
          icon={<Users className="size-4" />}
          meta="live sessions"
          action={<MockSurfaceBadge />}
        >
          {[
            ['Claude Code', 'v2.0.14 · http', '2h 14m'],
            ['Claude Desktop', 'v0.11.6 · http', '46m'],
            ['Codex', 'v0.48.0 · stdio', '3h 02m'],
            ['Gemini CLI', 'v0.9.2 · http', '11m'],
          ].map(([name, detail, age]) => (
            <div key={name} className="flex items-center gap-3 border-t border-aurora-border-default/40 py-1.5 first:border-t-0">
              <span className="grid size-7 place-items-center rounded-full bg-aurora-control-surface text-[8px] font-bold text-aurora-accent-strong">{name.split(' ').map((part) => part[0]).join('')}</span>
              <div className="min-w-0 flex-1"><div className="text-[11px] font-semibold text-aurora-text-primary">{name}</div><div className="mt-0.5 text-[9.5px] text-aurora-text-muted">{detail}</div></div>
              <span className="text-[9.5px] text-aurora-text-muted">{age}</span>
            </div>
          ))}
        </DashboardPanel>
      </div>
      <div data-mock-region="overview-gateway-host" aria-label="Gateway Host mock data">
        <DashboardPanel
          title="Gateway Host"
          icon={<Server className="size-4" />}
          meta="tootie · linux/amd64"
          action={<MockSurfaceBadge />}
        >
          <div className="grid grid-cols-2 gap-2">
            {[
              [Cpu, 'CPU', '15% · 8 cores'],
              [HardDrive, 'Mem', '1.2 / 3.0 GB'],
              [HardDrive, 'Disk', '14.6 / 24 GB'],
              [Network, 'Network', '↓ 1.4 MB/s · ↑ 320 KB/s'],
            ].map(([Icon, label, value]) => (
              <div key={label as string} className="rounded-[8px] border border-aurora-border-default/45 bg-[var(--gw0-0_42)] p-2.5">
                <div className="flex items-center gap-1.5 text-[8.5px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted"><Icon className="size-3" />{label as string}</div>
                <div className="mt-1.5 font-display text-[11px] font-bold text-aurora-text-primary">{value as string}</div>
              </div>
            ))}
          </div>
          <div className="text-[9.5px] leading-5 text-aurora-text-muted">9 live connections · 5 stdio processes · 412 MB child RSS</div>
        </DashboardPanel>
      </div>
    </>
  )
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

        {/* Telemetry split — the mock's 2fr content column beside a rail.
            Left column auto-fits at 250px so the wide panels span it. */}
        <div
          data-ovouter="1"
          data-mobile-stack="1"
          style={{
            display: 'grid',
            gridTemplateColumns: 'minmax(0, 2fr) minmax(260px, 1fr)',
            gap: 12,
            alignItems: 'start',
          }}
        >
          <div
            data-ovinner="1"
            data-mobile-stack="1"
            style={{
              display: 'grid',
              gridTemplateColumns: 'minmax(0, 1fr)',
              gap: 12,
              minWidth: 0,
              alignItems: 'start',
            }}
          >
            <DashboardPanel
              className="[grid-column:1/-1]"
              title="Calls by Server"
              meta={WINDOW_LABELS[activeWindow]}
            >
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
            </DashboardPanel>

            <DashboardPanel className="[grid-column:1/-1]" title="Top Tools">
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
            </DashboardPanel>

            {metrics ? (
              <>
                <FanOutPanel fanOut={metrics.fan_out} collected={metrics.collected.fan_out} />
                <LeastUsedPanel
                  tools={metrics.tools.least}
                  distinct={metrics.tools.distinct}
                  onSelect={(name) => setDrill({ type: 'tool', name })}
                />
              </>
            ) : metricsLoading ? (
              [1, 2].map((i) => (
                <Skeleton key={i} className="h-[176px] w-full rounded-aurora-2" />
              ))
            ) : (
              <MetricsUnavailable message="Usage insights are unavailable." />
            )}
          </div>

          <div style={{ display: 'grid', gap: 12, minWidth: 0, alignItems: 'start' }}>
            {metrics ? (
              <>
                <MostActivePanel
                  actors={metrics.actors}
                  window={activeWindow}
                  actorKindsCollected={metrics.collected.actor_kinds}
                  onSelectActor={(entry) => setDrill({ type: 'agent', id: entry.id })}
                />
                <UpstreamsPanel
                  upstreams={metrics.upstreams}
                  onSelect={(name) => router.push(`/usage/?window=${activeWindow}&upstream=${encodeURIComponent(name)}`)}
                />
                <CallOutcomesPanel
                  toolCalls={metrics.tool_calls}
                  errors={metrics.errors}
                  window={activeWindow}
                  onSelectOutcome={(outcome) => router.push(`/usage/?window=${activeWindow}&outcome=${outcome}`)}
                  onSelectError={(kind) => router.push(`/usage/?window=${activeWindow}&outcome=failed&error=${encodeURIComponent(kind)}`)}
                />
                <MockOverviewReferencePanels />
              </>
            ) : metricsLoading ? (
              [1, 2, 3].map((i) => (
                <Skeleton key={i} className="h-[176px] w-full rounded-aurora-2" />
              ))
            ) : (
              <MetricsUnavailable message="Usage breakdowns are unavailable." />
            )}
          </div>
        </div>

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
