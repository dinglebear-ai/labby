'use client'

import { useEffect, useState } from 'react'
import Link from 'next/link'
import { Wrench } from 'lucide-react'
import { OverviewChartMenu } from '@/components/dashboard/chart-menu'
import { ToolVolumeLegend } from '@/components/dashboard/tool-volume-chart'
import { ConnectedClientsPanel, GatewayHostPanel, useOverviewRuntime } from '@/components/dashboard/runtime-panels'
import { useRouter } from 'next/navigation'
import dynamic from 'next/dynamic'
import { ServerVolumeChart, ServerVolumeLegend, useServerVolume } from '@/components/dashboard/server-volume-chart'
import { RecentServers } from '@/components/dashboard/recent-servers'
import { ReorderableOverview } from '@/components/dashboard/reorderable-overview'
import { AppHeader } from '@/components/app-header'
import { Skeleton } from '@/components/ui/skeleton'
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
import { actorDrillTarget, type DrillTarget } from '@/components/dashboard/drill'
import { useGateways } from '@/lib/hooks/use-gateways'
import { useDashboardMetrics } from '@/lib/hooks/use-dashboard-metrics'
import {
  WINDOW_LABELS,
  buildLiveFleetStats,
  warningsSignature,
} from '@/lib/dashboard/dashboard-metrics'
import type { MetricsWindow } from '@/lib/types/metrics'
import { metricsLoadState } from '@/lib/dashboard/dashboard-load-state'
import { cn } from '@/lib/utils'
import {
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
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


export default function OverviewPage() {
  const router = useRouter()
  const runtime = useOverviewRuntime()
  const [chartMode, setChartMode] = useState<'servers' | 'volume' | 'outcomes' | 'errors'>('servers')
  const { data: gateways, isLoading: gatewaysLoading, error: gatewaysError, mutate: reloadGateways } = useGateways()
  const [activeWindow, setActiveWindow] = useState<MetricsWindow>('24h')
  const [drill, setDrill] = useState<DrillTarget | null>(null)
  const {
    data: metrics,
    error: metricsError,
    isLoading: isMetricsLoading,
    mutate: reloadMetrics,
  } = useDashboardMetrics(activeWindow)

  const serverVolume = useServerVolume(metrics)
  const live = buildLiveFleetStats(gateways ?? [])
  const warningsSig = warningsSignature(gateways ?? [])
  const metricsState = metricsLoadState(metrics, metricsError, isMetricsLoading)
  const metricsLoading = metricsState === 'loading'

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
          onRefresh={() => { reloadGateways(); reloadMetrics(); runtime.clients.mutate(); runtime.health.mutate(); runtime.host.mutate(); serverVolume.mutate() }}
          loadedAt={metricsLoadedAt}
        />

        {metricsState === 'unavailable' ? (
          <ErrorNotice message="Usage metrics aren't available on this Labby server yet." />
        ) : metricsState === 'error' ? (
          <ErrorNotice
            message="Couldn't load usage metrics for this window."
            onRetry={() => reloadMetrics()}
          />
        ) : null}

        {/* Two-thirds telemetry canvas and one-third insights rail; each lane
            retains its own visible reorder sequence. */}
        <ReorderableOverview cards={[
          { id: 'Call volume', wide: true, content: <DashboardPanel title="Upstream call volume" elevation="strong" headerStyle={{ padding: '12px 18px' }} bodyStyle={{ padding: '16px 18px 12px' }} titleControl={<OverviewChartMenu value={chartMode} onChange={setChartMode} />} meta={<span className="flex items-center gap-3">{chartMode === 'servers' ? <ServerVolumeLegend names={serverVolume.data?.names ?? []} /> : metrics ? <ToolVolumeLegend data={metrics.timeseries} mode={chartMode} /> : null}{WINDOW_LABELS[activeWindow]}</span>}>
              {metrics && chartMode === 'servers' ? (
                serverVolume.error ? <ErrorNotice message="Calls by server could not be reconciled for this window." onRetry={() => { reloadMetrics(); serverVolume.mutate() }} /> : serverVolume.data ? <ServerVolumeChart data={serverVolume.data} onSelectBucket={(from, to) => router.push(`/usage/?window=${activeWindow}&from=${Math.round(from)}&to=${Math.round(to)}`)} /> : <div role="status" className="grid h-[232px] place-items-center text-xs text-aurora-text-muted">Loading calls by server…</div>
              ) : metrics ? (
                <ToolVolumeChart
                  data={metrics.timeseries}
                  mode={chartMode === 'servers' ? 'outcomes' : chartMode}
                  window={activeWindow}
                  onSelectBucket={(from, to) => router.push(`/usage/?window=${activeWindow}&from=${Math.round(from)}&to=${Math.round(to)}`)}
                />
              ) : metricsLoading ? (
                <Skeleton className="h-[200px] w-full" />
              ) : (
                <MetricsUnavailable message="Upstream-call history is unavailable." />
              )}
            </DashboardPanel> },
          { id: 'Top targets', wide: false, content: <DashboardPanel title="Top targets" icon={<Wrench />} meta={WINDOW_LABELS[activeWindow]} headerStyle={{ padding: '10px 16px' }} bodyStyle={{ padding: '13px 16px' }}>
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
              { id: 'Call outcomes', content: <CallOutcomesPanel toolCalls={metrics.tool_calls} errors={metrics.errors} window={activeWindow} onSelectOutcome={(outcome) => router.push(`/usage/?window=${activeWindow}&outcome=${outcome}`)} onSelectError={(kind) => router.push(`/usage/?window=${activeWindow}&outcome=failed&error=${encodeURIComponent(kind)}`)}/> },
              { id: 'Least used', content: <LeastUsedPanel
                  tools={metrics.tools.least}
                  distinct={metrics.tools.distinct}
                  onSelect={(name) => setDrill({ type: 'tool', name })}
                /> },
              { id: 'Most active', rail: true, content: <MostActivePanel actors={metrics.actors} window={activeWindow} actorKindsCollected={metrics.collected.actor_kinds} onSelectActor={(entry) => setDrill(actorDrillTarget(entry))}/> },
              { id: 'Upstreams', rail: true, content: <UpstreamsPanel upstreams={metrics.upstreams} onSelect={(name) => router.push(`/usage/?window=${activeWindow}&upstream=${encodeURIComponent(name)}`)}/> },
            ] : metricsLoading ? (
              ['Call outcomes','Least used','Most active','Upstreams'].map((id) => ({ id, rail: id === 'Most active' || id === 'Upstreams', content: <Skeleton className="h-[176px] w-full rounded-aurora-2" /> }))
            ) : (
              ['Call outcomes','Least used','Most active','Upstreams'].map((id) => ({ id, rail: id === 'Most active' || id === 'Upstreams', content: <MetricsUnavailable message="Usage insights are unavailable." /> }))
            )),
          { id: 'Connected clients', rail: true, content: <ConnectedClientsPanel clients={runtime.clients.data} unavailable={Boolean(runtime.clients.error)} loading={runtime.clients.isLoading} onRetry={() => runtime.clients.mutate()} /> },
          { id: 'Gateway host', rail: true, content: <GatewayHostPanel metrics={runtime.host.data} health={runtime.health.data} clients={runtime.clients.error ? undefined : runtime.clients.data} loading={runtime.host.isLoading} /> },
          { id: 'Recent servers', rail: true, content: <RecentServers gateways={gateways ?? []} loading={gatewaysLoading} error={Boolean(gatewaysError)}/> },
        ]} />

        {/* ── Performance, cost & rhythm ─────────────────────────────── */}
          <details className="rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium">
            <summary className="cursor-pointer px-4 py-3 text-xs font-semibold text-aurora-text-muted focus-visible:outline-2 focus-visible:outline-aurora-accent-primary">Performance, cost &amp; rhythm</summary>
            <div className="flex flex-col gap-3 p-3 pt-0">
              <Link href="/skills/" className="text-xs text-aurora-accent-strong">Browse {gateways?.reduce((sum, gateway) => sum + (gateway.status.discovered_skill_count ?? 0), 0) ?? 0} discovered skills</Link>
              {!gatewaysLoading ? <WarningsBanner count={live.warnings} signature={warningsSig} /> : null}
              {metrics ? <>
              <FanOutPanel fanOut={metrics.fan_out} collected={metrics.collected.fan_out} />
          <AnalysisSection
            metrics={metrics}
            onSelectTool={(name) => setDrill({ type: 'tool', name })}
            onOpenUsage={(query) => router.push(`/usage/?window=${activeWindow}&${query}`)}
          />
              </> : null}
            </div>
          </details>


      </div>

      <ToolDetailDrawer
        tool={drill?.type === 'tool' ? drill.name : null}
        window={activeWindow}
        onClose={() => setDrill(null)}
        onDrill={setDrill}
      />
      <AgentDetailDrawer
        target={drill?.type === 'agent' ? drill : null}
        window={activeWindow}
        onClose={() => setDrill(null)}
        onDrill={setDrill}
      />
    </>
  )
}
