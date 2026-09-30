'use client'

import { Bar, BarChart, Line, LineChart, XAxis } from 'recharts'
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '@/components/ui/chart'
import type { MetricsBucket, MetricsWindow } from '@/lib/types/metrics'

const WINDOW_MS: Record<MetricsWindow, number> = {
  '1h': 60 * 60 * 1000,
  '24h': 24 * 60 * 60 * 1000,
  '7d': 7 * 24 * 60 * 60 * 1000,
  '30d': 30 * 24 * 60 * 60 * 1000,
}

const OUTCOME_SERIES = [
  { key: 'succeeded', label: 'Succeeded', color: 'var(--aurora-accent-primary)' },
  { key: 'upstreamError', label: 'Upstream Error', color: 'var(--aurora-error)' },
  { key: 'timedOut', label: 'Timed Out', color: 'var(--aurora-warn)' },
  { key: 'responseTooLarge', label: 'Response Too Large', color: 'var(--aurora-accent-pink-deep)' },
  { key: 'connectionFailed', label: 'Connection Failed', color: 'var(--aurora-text-muted)' },
  { key: 'unknown', label: 'Other / Unknown', color: 'var(--aurora-border-strong)' },
  { key: 'unclassifiedFailed', label: 'Failed (unclassified)', color: 'var(--aurora-error)' },
] as const

type OutcomeKey = (typeof OUTCOME_SERIES)[number]['key']
type OutcomeRow = Record<OutcomeKey, number> & {
  ts: number
  label: string
  calls: number
  failed: number
}

const CONFIG: ChartConfig = {
  calls: { label: 'Calls', color: 'var(--aurora-accent-primary)' },
  failed: { label: 'Failed', color: 'var(--aurora-error)' },
  ...Object.fromEntries(OUTCOME_SERIES.map((series) => [series.key, {
    label: series.label,
    color: series.color,
  }])),
}

function bucketLabel(ts: number, window: MetricsWindow): string {
  const date = new Date(ts)
  if (window === '30d') return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
  if (window === '7d') return date.toLocaleDateString(undefined, { weekday: 'short' })
  return date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
}

function count(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0
}

function outcomeKey(kind: string): Exclude<OutcomeKey, 'succeeded' | 'unclassifiedFailed'> {
  if (kind === 'upstream_error') return 'upstreamError'
  if (kind === 'timeout') return 'timedOut'
  if (kind === 'response_too_large') return 'responseTooLarge'
  if (kind === 'connection_failed') return 'connectionFailed'
  return 'unknown'
}

/** Reconciles optional outcome detail against the bucket's authoritative totals. */
export function buildOutcomeRows(data: MetricsBucket[], window: MetricsWindow): OutcomeRow[] {
  return data.map((bucket) => {
    const calls = count(bucket.calls)
    const failed = Math.min(calls, count(bucket.failed))
    const requested = new Map<Exclude<OutcomeKey, 'succeeded' | 'unclassifiedFailed'>, number>()
    const collected = Array.isArray(bucket.outcomes)

    for (const outcome of bucket.outcomes ?? []) {
      if (outcome.kind === 'succeeded' || outcome.kind === 'ok') continue
      const key = outcomeKey(outcome.kind)
      requested.set(key, (requested.get(key) ?? 0) + count(outcome.count))
    }

    let remaining = failed
    const take = (key: Exclude<OutcomeKey, 'succeeded' | 'unclassifiedFailed'>) => {
      const value = Math.min(remaining, requested.get(key) ?? 0)
      remaining -= value
      return value
    }
    const upstreamError = take('upstreamError')
    const timedOut = take('timedOut')
    const responseTooLarge = take('responseTooLarge')
    const connectionFailed = take('connectionFailed')
    const explicitUnknown = take('unknown')

    return {
      ts: bucket.ts,
      label: bucketLabel(bucket.ts, window),
      calls,
      failed,
      succeeded: calls - failed,
      upstreamError,
      timedOut,
      responseTooLarge,
      connectionFailed,
      unknown: collected ? explicitUnknown + remaining : 0,
      unclassifiedFailed: collected ? 0 : failed,
    }
  })
}

/** At most five evenly distributed labels, including both endpoints. */
export function chartTickIndexes(length: number): number[] {
  if (length <= 0) return []
  const tickCount = Math.min(5, length)
  return [...new Set(Array.from(
    { length: tickCount },
    (_, index) => Math.round(index * (length - 1) / Math.max(1, tickCount - 1)),
  ))]
}

/** Inclusive persisted-second range for one chart bucket. */
export function bucketDrillRange(rows: Array<{ ts: number }>, index: number, window: MetricsWindow): [number, number] | null {
  const row = rows[index]
  if (!row) return null
  const width = rows.length > 0 ? WINDOW_MS[window] / rows.length : WINDOW_MS[window]
  const nextTs = rows[index + 1]?.ts ?? row.ts + width
  return [row.ts, Math.max(row.ts, nextTs - 1000)]
}

function visibleOutcomeSeries(rows: OutcomeRow[]) {
  return OUTCOME_SERIES.filter((series) => rows.some((row) => row[series.key] > 0))
}

export function ToolVolumeLegend({
  data,
  mode,
  className = '',
}: {
  data: MetricsBucket[]
  mode: 'volume' | 'outcomes' | 'errors'
  className?: string
}) {
  if (mode === 'volume') return null
  const rows = buildOutcomeRows(data, '24h')
  const series = mode === 'errors'
    ? [
        { key: 'succeeded', label: 'Succeeded', color: 'var(--aurora-accent-primary)' },
        { key: 'failed', label: 'Failed', color: 'var(--aurora-error)' },
      ]
    : visibleOutcomeSeries(rows)
  return <span aria-label="Chart legend" className={`hidden flex-wrap items-center gap-x-3 gap-y-1 sm:inline-flex ${className}`}>{series.map((item) => <span key={item.key} className="inline-flex items-center gap-1.5"><span aria-hidden className="h-[3px] w-2.5 rounded-full" style={{ background: item.color }} />{item.label}</span>)}</span>
}

export function ToolVolumeChart({
  data,
  window,
  onSelectBucket,
  mode = 'outcomes',
}: {
  data: MetricsBucket[]
  mode?: 'volume' | 'outcomes' | 'errors'
  window: MetricsWindow
  onSelectBucket?: (sinceMs: number, untilMs: number) => void
}) {
  const rows = buildOutcomeRows(data, window)
  const outcomeSeries = visibleOutcomeSeries(rows)
  const selectRow = (entry: unknown) => {
    if (!onSelectBucket) return
    const event = entry as { ts?: unknown; payload?: { ts?: unknown } }
    const ts = typeof event.ts === 'number' ? event.ts : event.payload?.ts
    if (typeof ts !== 'number') return
    const index = rows.findIndex((row) => row.ts === ts)
    const range = bucketDrillRange(rows, index, window)
    if (range) onSelectBucket(...range)
  }

  return (
    <div className="min-w-0">
      <ChartContainer config={CONFIG} className="aspect-auto h-[210px] w-full">
        {mode === 'errors' ? <LineChart data={rows} margin={{ left: 0, right: 0, top: 0, bottom: 0 }}>
          <XAxis dataKey="label" hide />
          <ChartTooltip cursor={false} content={<ChartTooltipContent />} />
          <Line dataKey="succeeded" stroke="var(--color-succeeded)" strokeWidth={1.7} dot={false} isAnimationActive={false} />
          <Line dataKey="failed" stroke="var(--color-failed)" strokeWidth={1.4} dot={false} isAnimationActive={false} />
        </LineChart> : <BarChart data={rows} margin={{ left: 0, right: 0, top: 0, bottom: 0 }} barCategoryGap={1.5}>
          <XAxis dataKey="label" hide />
          <ChartTooltip cursor={false} content={<ChartTooltipContent />} />
          {mode === 'volume' ? <Bar
            dataKey="calls"
            fill="var(--color-calls)"
            radius={[2, 2, 0, 0]}
            isAnimationActive={false}
            onClick={selectRow}
            cursor={onSelectBucket ? 'pointer' : undefined}
          /> : outcomeSeries.map((series, index) => <Bar
            key={series.key}
            dataKey={series.key}
            stackId="calls"
            fill={`var(--color-${series.key})`}
            radius={index === outcomeSeries.length - 1 ? [2, 2, 0, 0] : undefined}
            isAnimationActive={false}
            onClick={selectRow}
            cursor={onSelectBucket ? 'pointer' : undefined}
          />)}
        </BarChart>}
      </ChartContainer>
      {rows.length ? <div aria-label="Chart time ticks" className="mt-2 flex justify-between text-[10px] tabular-nums text-aurora-text-muted">{chartTickIndexes(rows.length).map((index) => <span key={`${rows[index].ts}-${index}`}>{rows[index].label}</span>)}</div> : null}
    </div>
  )
}
