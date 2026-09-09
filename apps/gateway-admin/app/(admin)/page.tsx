'use client'

import { useEffect, useState } from 'react'
import { useRouter } from 'next/navigation'
import dynamic from 'next/dynamic'
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
import type { DrillTarget } from '@/components/dashboard/drill'
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
  const { data: gateways, isLoading: gatewaysLoading, error: gatewaysError } = useGateways()
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

        {/* Match the reference's two-thirds telemetry canvas and one-third
            insights rail; each lane retains its own visible reorder sequence. */}
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
              { id: 'Call outcomes', content: <CallOutcomesPanel toolCalls={metrics.tool_calls} errors={metrics.errors} window={activeWindow} onSelectOutcome={(outcome) => router.push(`/usage/?window=${activeWindow}&outcome=${outcome}`)} onSelectError={(kind) => router.push(`/usage/?window=${activeWindow}&outcome=failed&error=${encodeURIComponent(kind)}`)}/> },
              { id: 'Least used', content: <LeastUsedPanel
                  tools={metrics.tools.least}
                  distinct={metrics.tools.distinct}
                  onSelect={(name) => setDrill({ type: 'tool', name })}
                /> },
              { id: 'Most active', rail: true, content: <MostActivePanel actors={metrics.actors} window={activeWindow} actorKindsCollected={metrics.collected.actor_kinds} onSelectActor={(entry) => setDrill({ type: 'agent', id: entry.id })}/> },
              { id: 'Upstreams', rail: true, content: <UpstreamsPanel upstreams={metrics.upstreams} onSelect={(name) => router.push(`/usage/?window=${activeWindow}&upstream=${encodeURIComponent(name)}`)}/> },
              { id: 'Code Mode fan-out', content: <FanOutPanel fanOut={metrics.fan_out} collected={metrics.collected.fan_out} /> },
            ] : metricsLoading ? (
              ['Call outcomes','Least used','Most active','Upstreams','Code Mode fan-out'].map((id) => ({ id, rail: id === 'Most active' || id === 'Upstreams', content: <Skeleton className="h-[176px] w-full rounded-aurora-2" /> }))
            ) : (
              ['Call outcomes','Least used','Most active','Upstreams','Code Mode fan-out'].map((id) => ({ id, rail: id === 'Most active' || id === 'Upstreams', content: <MetricsUnavailable message="Usage insights are unavailable." /> }))
            )),
          { id: 'Recent servers', rail: true, content: <RecentServers gateways={gateways ?? []} loading={gatewaysLoading} error={Boolean(gatewaysError)}/> },
        ]} />

        {/* ── Performance, cost & rhythm ─────────────────────────────── */}
        {metrics ? (
          <AnalysisSection
            metrics={metrics}
            onSelectTool={(name) => setDrill({ type: 'tool', name })}
            onOpenUsage={(query) => router.push(`/usage/?window=${activeWindow}&${query}`)}
          />
        ) : null}


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
