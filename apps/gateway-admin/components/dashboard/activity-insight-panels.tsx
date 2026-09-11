'use client'

import { useState, type ReactNode } from 'react'
import Link from 'next/link'
import { Bot, Globe, HardDrive, Network, TrendingDown } from 'lucide-react'
import { DashboardPanel } from './panel'
import { MetricBarList } from './metric-bars'
import { DASH_METRIC_SM, dashPill } from './ui'
import { formatCompactNumber } from '@/lib/dashboard/dashboard-metrics'
import type {
  ActorFacet,
  ActorKind,
  ActorUsageEntry,
  DashboardMetrics,
  MetricsWindow,
  ToolUsageEntry,
} from '@/lib/types/metrics'
import { cn } from '@/lib/utils'

const ROW_BASE = '-mx-2 flex w-full items-center gap-3 rounded-aurora-1 px-2 py-1 text-left'
const ROW_INTERACTIVE =
  'transition-colors hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40'

function RowShell({
  onClick,
  children,
}: {
  onClick?: () => void
  children: ReactNode
}) {
  if (onClick) {
    return (
      <button type="button" onClick={onClick} className={cn(ROW_BASE, ROW_INTERACTIVE)}>
        {children}
      </button>
    )
  }
  return <div className={ROW_BASE}>{children}</div>
}

const ACTOR_FACETS: Array<{ key: ActorKind; label: string; unit: string }> = [
  { key: 'agent', label: 'Agents', unit: 'agent' },
  { key: 'device', label: 'Devices', unit: 'device' },
  { key: 'ip', label: 'IPs', unit: 'IP' },
]

function ActorIcon({ kind }: { kind: ActorKind }) {
  if (kind === 'agent') return <Bot className="size-3.5 shrink-0 text-aurora-accent-strong" />
  if (kind === 'device') return <HardDrive className="size-3.5 shrink-0 text-aurora-text-muted" />
  return <Globe className="size-3.5 shrink-0 text-aurora-text-muted" />
}

function ActorRowContent({ entry, maxCalls }: { entry: ActorUsageEntry; maxCalls: number }) {
  const ratio = maxCalls > 0 ? Math.max(0, Math.min(100, entry.calls / maxCalls * 100)) : 0
  const initials = entry.label.split(/\s+/).filter(Boolean).slice(0, 2).map(word => Array.from(word)[0]).join('').toUpperCase()
  return (
    <span className="flex min-w-0 flex-1 flex-col gap-1.5 py-0.5">
      <span className="flex min-w-0 items-center gap-2">
      {entry.kind === 'agent' ? <span aria-hidden="true" className="grid size-6 shrink-0 place-items-center rounded-[7px] border border-aurora-accent-pink-deep/35 bg-aurora-accent-pink/10 text-[9px] font-bold text-aurora-accent-pink">{initials}</span> : <ActorIcon kind={entry.kind} />}
      <span
        className={cn(
          'min-w-0 flex-1 truncate text-[12.5px] font-semibold text-aurora-text-primary',
          entry.kind === 'ip' && 'font-mono text-[13px]',
        )}
      >
        {entry.label}
      </span>
      <span className="shrink-0 text-[11.5px] font-semibold tabular-nums text-aurora-text-primary">
        {formatCompactNumber(entry.calls)} calls
      </span>
      </span>
      <span aria-hidden="true" className="h-[3px] overflow-hidden rounded-full bg-aurora-control-surface"><span data-actor-volume-bar className="block h-full rounded-full bg-gradient-to-r from-aurora-accent-pink-deep to-aurora-accent-pink" style={{ width: `${ratio}%` }}/></span>
    </span>
  )
}

/**
 * Busiest actors, ranked within a single population. Agents, devices, and
 * source IPs are separate facets — never compared in one count.
 */
export function MostActivePanel({
  actors,
  window,
  onSelectActor,
  actorKindsCollected = true,
}: {
  actors: { agent: ActorFacet; device: ActorFacet; ip: ActorFacet }
  window: MetricsWindow
  onSelectActor: (entry: ActorUsageEntry) => void
  actorKindsCollected?: boolean
}) {
  const [facet, setFacet] = useState<ActorKind>('agent')
  const current = actors[facet]
  const meta = ACTOR_FACETS.find((f) => f.key === facet)!
  const top = current.top.slice(0, 5)
  const maxCalls = Math.max(0, ...top.map(entry => entry.calls))

  return (
    <DashboardPanel
      title={actorKindsCollected ? 'Most active' : 'Most active subjects'}
      iconTone="pink"
      icon={<Bot className="size-4" />}
      meta={actorKindsCollected
        ? `${current.active} ${meta.unit}${current.active === 1 ? '' : 's'}`
        : `${current.active} subject${current.active === 1 ? '' : 's'}`}
    >
      {actorKindsCollected ? <div
        role="tablist"
        aria-label="Actor facet"
        className="inline-flex items-center gap-1 rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface p-0.5"
      >
        {ACTOR_FACETS.map((f) => {
          const active = f.key === facet
          return (
            <button
              key={f.key}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => setFacet(f.key)}
              className={cn(
                'flex-1 rounded-aurora-1 border px-2 py-1 text-xs font-semibold transition-colors',
                'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40',
                dashPill(active),
              )}
            >
              {f.label}
            </button>
          )
        })}
      </div> : null}

      {top.length === 0 ? (
        <p className="text-sm text-aurora-text-muted">No {meta.unit} activity in this window.</p>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {top.map((entry) =>
            entry.kind === 'ip' ? (
              <li key={entry.id}>
                <Link
                  href={`/usage?ip=${encodeURIComponent(entry.id)}&window=${window}`}
                  className={cn(ROW_BASE, ROW_INTERACTIVE)}
                >
                  <ActorRowContent entry={entry} maxCalls={maxCalls} />
                </Link>
              </li>
            ) : (
              <li key={entry.id}>
                <RowShell onClick={() => onSelectActor(entry)}>
                  <ActorRowContent entry={entry} maxCalls={maxCalls} />
                </RowShell>
              </li>
            ),
          )}
        </ul>
      )}
    </DashboardPanel>
  )
}

function FanOutStat({ value, label }: { value: ReactNode; label: string }) {
  return (
    <div>
      <p className={cn(DASH_METRIC_SM, 'text-aurora-text-primary')}>{value}</p>
      <p className="mt-1 text-xs text-aurora-text-muted">{label}</p>
    </div>
  )
}

/** Code Mode fan-out — orchestrated multi-tool execute runs. */
export function FanOutPanel({
  fanOut,
  collected = true,
}: {
  fanOut: DashboardMetrics['fan_out']
  collected?: boolean
}) {
  const pct = (rate: number) => `${Math.round(rate * 100)}%`
  if (!collected) {
    return (
      <DashboardPanel title="Code Mode fan-out" icon={<Network className="size-4" />}>
        <p className="text-sm text-aurora-text-muted">Fan-out telemetry is not collected.</p>
      </DashboardPanel>
    )
  }
  return (
    <DashboardPanel
      title="Code Mode fan-out"
      icon={<Network className="size-4" />}
      meta={`${pct(fanOut.truncation_rate)} truncated · retained sample`}
    >
      <div className="grid grid-cols-3 gap-x-4 gap-y-4">
        <FanOutStat value={formatCompactNumber(fanOut.total_calls)} label="Fanned-out" />
        <FanOutStat value={formatCompactNumber(fanOut.runs)} label="Execute runs" />
        <FanOutStat value={fanOut.avg_calls_per_run} label="Avg / run" />
        <FanOutStat value={fanOut.max_calls_in_run} label="Max in run" />
        <FanOutStat value={pct(fanOut.timeout_rate)} label="Timeout rate" />
        <FanOutStat value={formatCompactNumber(fanOut.artifact_writes)} label="Artifacts" />
      </div>
    </DashboardPanel>
  )
}

/** Lowest-traffic upstream targets — candidates to prune or investigate. */
export function LeastUsedPanel({
  tools,
  distinct,
  onSelect,
}: {
  tools: ToolUsageEntry[]
  distinct: number
  onSelect?: (tool: string) => void
}) {
  return (
    <DashboardPanel title="Least used" iconTone="warn" icon={<TrendingDown className="size-4" />} meta={`of ${distinct} distinct`}>
      <MetricBarList tone="warn" empty="No upstream calls in this window." items={tools.slice(0, 4).map(tool => ({
        key: tool.id ?? tool.name, label: tool.label ?? tool.name, value: tool.calls,
        display: `${formatCompactNumber(tool.calls)} calls`,
        onSelect: onSelect ? () => onSelect(tool.name) : undefined,
      }))}/>
    </DashboardPanel>
  )
}
