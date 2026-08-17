'use client'

import { Bar, BarChart, CartesianGrid, XAxis } from 'recharts'
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '@/components/ui/chart'
import type { MetricsBucket, MetricsWindow } from '@/lib/types/metrics'

const CONFIG: ChartConfig = {
  calls: { label: 'Tool calls', color: 'var(--aurora-accent-primary)' },
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
}: {
  data: MetricsBucket[]
  window: MetricsWindow
}) {
  const rows = data.map((bucket) => ({
    label: bucketLabel(bucket.ts, window),
    calls: Math.max(0, bucket.calls - bucket.failed),
    failed: bucket.failed,
  }))

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
          dataKey="calls"
          stackId="calls"
          fill="var(--color-calls)"
          radius={[2, 2, 0, 0]}
          isAnimationActive={false}
        />
        <Bar
          dataKey="failed"
          stackId="calls"
          fill="var(--color-failed)"
          radius={[2, 2, 0, 0]}
          isAnimationActive={false}
        />
      </BarChart>
    </ChartContainer>
  )
}
