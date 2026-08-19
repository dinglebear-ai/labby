'use client'

import { Bar, BarChart, CartesianGrid, XAxis } from 'recharts'
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
    const ts = (entry as { ts?: unknown }).ts
    if (typeof ts !== 'number') return
    const index = rows.findIndex((row) => row.ts === ts)
    const width = rows.length > 0 ? WINDOW_MS[window] / rows.length : WINDOW_MS[window]
    onSelectBucket(ts, rows[index + 1]?.ts ?? ts + width)
  }

  return (
    <ChartContainer config={CONFIG} className="aspect-auto h-[200px] w-full">
      <BarChart data={rows} margin={{ left: 4, right: 4, top: 8, bottom: 0 }} barCategoryGap="8%">
        <CartesianGrid vertical={false} strokeDasharray="3 3" />
        <XAxis
          dataKey="label"
          tickLine={false}
          axisLine={false}
          tickMargin={8}
          minTickGap={24}
        />
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
  )
}
