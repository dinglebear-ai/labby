'use client'

import { useEffect, useState } from 'react'
import Link from 'next/link'
import { ChartNoAxesCombined, Layers, Sparkles, Wrench, Zap } from 'lucide-react'
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
  FailuresPanel,
  HourlyHeatPanel,
  LatencyPanel,
  SurfacesPanel,
  ThroughputPanel,
  TokensByToolPanel,
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
  const warningNotificationKeys = (gateways ?? []).flatMap((gateway) =>
    gateway.warnings.map((warning) => `gateway:${gateway.name}:warning:${warning.code}`),
  )
  const discoveredSkills = gateways?.reduce((sum, gateway) => sum + (gateway.status.discovered_skill_count ?? 0), 0) ?? 0
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

        <div className="flex flex-col gap-3">
          <Link
            href="/skills/"
            aria-label={`Browse ${discoveredSkills} discovered skills`}
            title="Browse discovered skills"
            className="inline-flex w-fit items-center gap-2 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface px-3 py-2 text-xs font-semibold text-aurora-text-muted transition-colors hover:border-aurora-border-strong hover:bg-aurora-hover-bg hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"
          >
            <Sparkles aria-hidden="true" className="size-3.5 text-aurora-accent-strong" />
            <span>{discoveredSkills} discovered skills</span>
          </Link>
          {!gatewaysLoading ? <WarningsBanner count={live.warnings} signature={warningsSig} notificationKeys={warningNotificationKeys} /> : null}
        </div>

        {/* Two-thirds telemetry canvas and one-third insights rail; each lane
            retains its own visible reorder sequence. */}
        <ReorderableOverview cards={[
          { id: 'Call volume', wide: true, content: <DashboardPanel title="Upstream call volume" icon={<ChartNoAxesCombined />} elevation="strong" headerStyle={{ padding: '12px 18px' }} bodyStyle={{ padding: '16px 18px 12px' }} titleControl={<OverviewChartMenu value={chartMode} onChange={setChartMode} />} meta={<span className="flex items-center gap-3">{chartMode === 'servers' ? <ServerVolumeLegend names={serverVolume.data?.names ?? []} /> : metrics ? <ToolVolumeLegend data={metrics.timeseries} mode={chartMode} /> : null}{WINDOW_LABELS[activeWindow]}</span>}>
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
              { id: 'Code Mode fan-out', content: <FanOutPanel fanOut={metrics.fan_out} collected={metrics.collected.fan_out} /> },
              { id: 'Latency', content: <LatencyPanel latency={metrics.latency} onSelectMetric={(metric) => router.push(`/usage/?window=${activeWindow}&focus=latency&percentile=${metric}`)} onSelectTool={(name) => setDrill({ type: 'tool', name })} /> },
              { id: 'Failures by kind', content: <FailuresPanel errors={metrics.errors} onSelect={(kind) => router.push(`/usage/?window=${activeWindow}&outcome=failed&error=${encodeURIComponent(kind)}`)} /> },
              { id: 'By surface', content: metrics.collected.surfaces
                ? <SurfacesPanel surfaces={metrics.surfaces} />
                : <DashboardPanel title="By surface" icon={<Layers />}><p className="text-sm text-aurora-text-muted">Surface attribution is not collected.</p></DashboardPanel> },
              { id: 'Tokens by tool', content: metrics.collected.tokens
                ? <TokensByToolPanel tokens={metrics.tokens_by_tool} onSelect={(name) => setDrill({ type: 'tool', name })} />
                : <DashboardPanel title="Tokens by tool" icon={<Zap />}><p className="text-sm text-aurora-text-muted">Token usage is not collected.</p></DashboardPanel> },
              { id: 'Throughput', content: <ThroughputPanel throughput={metrics.throughput} agentsSeen={metrics.agents_seen} showAgents={metrics.collected.actor_kinds} onSelect={(metric) => router.push(`/usage/?window=${activeWindow}&focus=throughput&metric=${metric}`)} /> },
              { id: 'Activity by hour', content: <HourlyHeatPanel hourly={metrics.hourly} busiestHour={metrics.throughput.busiest_hour} onSelectHour={(hour) => router.push(`/usage/?window=${activeWindow}&focus=hour&hour=${hour}`)} /> },
              { id: 'Most active', rail: true, content: <MostActivePanel actors={metrics.actors} window={activeWindow} actorKindsCollected={metrics.collected.actor_kinds} onSelectActor={(entry) => setDrill(actorDrillTarget(entry))}/> },
              { id: 'Upstreams', rail: true, content: <UpstreamsPanel upstreams={metrics.upstreams} onSelect={(name) => router.push(`/usage/?window=${activeWindow}&upstream=${encodeURIComponent(name)}`)}/> },
            ] : metricsLoading ? (
              ['Call outcomes','Least used','Code Mode fan-out','Latency','Failures by kind','By surface','Tokens by tool','Throughput','Activity by hour','Most active','Upstreams'].map((id) => ({ id, rail: id === 'Most active' || id === 'Upstreams', content: <Skeleton className="h-[176px] w-full rounded-aurora-2" /> }))
            ) : (
              ['Call outcomes','Least used','Code Mode fan-out','Latency','Failures by kind','By surface','Tokens by tool','Throughput','Activity by hour','Most active','Upstreams'].map((id) => ({ id, rail: id === 'Most active' || id === 'Upstreams', content: <MetricsUnavailable message="Usage insights are unavailable." /> }))
            )),
          { id: 'Connected clients', rail: true, content: <ConnectedClientsPanel clients={runtime.clients.data} unavailable={Boolean(runtime.clients.error)} loading={runtime.clients.isLoading} onRetry={() => runtime.clients.mutate()} /> },
          { id: 'Gateway host', rail: true, content: <GatewayHostPanel metrics={runtime.host.data} health={runtime.health.data} clients={runtime.clients.error ? undefined : runtime.clients.data} loading={runtime.host.isLoading} /> },
          { id: 'Recent servers', rail: true, content: <RecentServers gateways={gateways ?? []} loading={gatewaysLoading} error={Boolean(gatewaysError)}/> },
        ]} />
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
