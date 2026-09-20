'use client'

import Link from 'next/link'
import { ArrowUpRight, Bot, HardDrive } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { useAgentDetail } from '@/lib/hooks/use-usage-drilldown'
import { ToolVolumeChart } from './tool-volume-chart'
import { RecentCallsList } from './recent-calls'
import { DetailDrawer, DrawerSection, DrawerStatGrid, RankRow } from './detail-drawer'
import { ErrorNotice } from './error-notice'
import {
  actorUsageHref,
  sameAttributionDrillFilter,
  type DrillTarget,
} from './drill'
import {
  WINDOW_LABELS,
  formatCompactNumber,
  successRatePercent,
} from '@/lib/dashboard/dashboard-metrics'
import type { ActorDrillTarget, MetricsWindow } from '@/lib/types/metrics'
import { getErrorMessage } from '@/lib/utils'

export function AgentDetailDrawer({
  target,
  window,
  onClose,
  onDrill,
}: {
  target: ActorDrillTarget | null
  window: MetricsWindow
  onClose: () => void
  onDrill: (target: DrillTarget) => void
}) {
  const { data, isLoading, error, mutate } = useAgentDetail(target, window)
  const detail = data && target && sameAttributionDrillFilter(data.filter, target.filter)
    ? data
    : undefined
  const rate = detail ? successRatePercent(detail.calls, detail.failed) : null
  const kind = detail?.kind ?? target?.kind
  const Icon = kind === 'device' ? HardDrive : Bot

  return (
    <DetailDrawer
      open={target !== null}
      onClose={onClose}
      icon={<Icon className="size-5" />}
      title={detail?.label ?? target?.label ?? ''}
      subtitle={`${kind === 'device' ? 'Device' : kind === 'agent' ? 'Agent' : kind === 'client' ? 'Client' : kind === 'unknown' ? 'Unknown identity' : 'Subject'} activity · ${WINDOW_LABELS[window]}`}
    >
      {error && !detail ? (
        <ErrorNotice message={getErrorMessage(error, "Couldn't load activity details.")} onRetry={() => mutate()} />
      ) : !detail || isLoading ? (
        <div className="flex flex-col gap-6">
          <Skeleton className="h-20 w-full" />
          <Skeleton className="h-[180px] w-full" />
          <Skeleton className="h-40 w-full" />
        </div>
      ) : (
        <>
          <p className="break-all text-xs text-aurora-text-muted">
            {target?.kind === 'client'
              ? `Exact initialized client: ${target.filter.client_name} ${target.filter.client_version} · actor ${target.filter.actor}`
              : target?.kind === 'agent'
                ? `Exact Agent ID: ${target.filter.agent_id} · actor ${target.filter.actor}`
                : `Recorded identity: ${detail.id}`}
          </p>
          <DrawerStatGrid
            items={[
              { label: 'Calls', value: formatCompactNumber(detail.calls) },
              {
                label: 'Failed',
                value: formatCompactNumber(detail.failed),
                tone: detail.failed > 0 ? 'error' : 'success',
              },
              { label: 'Success', value: rate !== null ? `${rate}%` : '—' },
              {
                label: 'Tokens',
                value: detail.tokens_collected ? formatCompactNumber(detail.total_tokens) : '—',
              },
            ]}
          />

          <DrawerSection title="Activity">
            <ToolVolumeChart data={detail.timeseries} window={window} />
          </DrawerSection>

          <DrawerSection title="Targets used">
            {detail.tools_used.length === 0 ? (
              <p className="text-sm text-aurora-text-muted">No upstream targets in this window.</p>
            ) : (
              <div className="flex flex-col">
                {detail.tools_used.slice(0, 8).map((tool) => (
                  <RankRow
                    key={tool.name}
                    label={tool.name}
                    mono
                    value={formatCompactNumber(tool.calls)}
                    onClick={() => onDrill({ type: 'tool', name: tool.name })}
                  />
                ))}
              </div>
            )}
          </DrawerSection>

          <DrawerSection
            title="Recent calls"
            action={
              <Button variant="ghost" size="sm" asChild>
                <Link href={target ? actorUsageHref(target, window) : '/usage'}>
                  Open in explorer
                  <ArrowUpRight className="ml-1 size-3.5" />
                </Link>
              </Button>
            }
          >
            <RecentCallsList calls={detail.recent} />
          </DrawerSection>
        </>
      )}
    </DetailDrawer>
  )
}
