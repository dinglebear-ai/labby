'use client'

import { Bar, BarChart, XAxis } from 'recharts'
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
}

const CONFIG: ChartConfig = {
  succeeded: { label: 'Succeeded', color: 'var(--aurora-accent-primary)' },
  failed: { label: 'Failed', color: 'var(--aurora-error)' },
}

function bucketLabel(ts: number, window: MetricsWindow): string {
  const date = new Date(ts)
  if (window === '7d') {
    return date.toLocaleDateString(undefined, { weekday: 'short' })
  }
  return date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
}

export function ToolVolumeChart({
  data,
  window,
  onSelectBucket,
}: {
  data: MetricsBucket[]
  window: MetricsWindow
  onSelectBucket?: (sinceMs: number, untilMs: number) => void
}) {
  const rows = data.map((bucket) => ({
    ts: bucket.ts,
    label: bucketLabel(bucket.ts, window),
    succeeded: Math.max(0, bucket.calls - bucket.failed),
    failed: bucket.failed,
  }))
  const selectRow = (entry: unknown) => {
    if (!onSelectBucket) return
    const event = entry as { ts?: unknown; payload?: { ts?: unknown } }
    const ts = typeof event.ts === 'number' ? event.ts : event.payload?.ts
    if (typeof ts !== 'number') return
    const index = rows.findIndex((row) => row.ts === ts)
    if (index < 0) return
    const width = rows.length > 0 ? WINDOW_MS[window] / rows.length : WINDOW_MS[window]
    const nextTs = rows[index + 1]?.ts ?? ts + width
    // Backend until bounds are inclusive. Stop one persisted second before the
    // next bucket so a boundary call belongs to exactly one drill-down slice.
    onSelectBucket(ts, Math.max(ts, nextTs - 1000))
  }

  return (
    <div className="min-w-0">
    <ChartContainer config={CONFIG} className="aspect-auto h-[210px] w-full">
      <BarChart data={rows} margin={{ left: 0, right: 0, top: 0, bottom: 0 }} barCategoryGap={1.5}>
        <XAxis dataKey="label" hide />
        <ChartTooltip cursor={false} content={<ChartTooltipContent />} />
        <Bar
          dataKey="succeeded"
          stackId="calls"
          fill="var(--color-succeeded)"
          radius={[2, 2, 0, 0]}
          isAnimationActive={false}
          onClick={selectRow}
          cursor={onSelectBucket ? 'pointer' : undefined}
        />
        <Bar
          dataKey="failed"
          stackId="calls"
          fill="var(--color-failed)"
          radius={[2, 2, 0, 0]}
          isAnimationActive={false}
          onClick={selectRow}
          cursor={onSelectBucket ? 'pointer' : undefined}
        />
      </BarChart>
    </ChartContainer>
    {rows.length ? <div aria-label="Chart time range" className="mt-2 flex justify-between text-[10px] tabular-nums text-aurora-text-muted"><span>{rows[0].label}</span><span>{rows.at(-1)!.label}</span></div> : null}
    </div>
  )
}
