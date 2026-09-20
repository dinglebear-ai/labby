'use client'

import { useSyncExternalStore } from 'react'
import useSWR from 'swr'
import { fetchGatewayUsageMetrics } from '@/lib/api/metrics-client'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { getBrowserSessionContextIdentity, getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import type { DashboardMetrics, MetricsBucket } from '@/lib/types/metrics'
import type { GatewayUsageMetrics } from '@/lib/dashboard/gateway-usage-adapter'

const COLORS = ['var(--aurora-accent-primary)', 'var(--aurora-accent-pink)', 'var(--aurora-success)', 'var(--aurora-warn)', 'var(--aurora-accent-deep)']
export type ServerVolume = { names: string[]; buckets: Array<{ ts: number; total: number; values: number[] }> }

/** A missing response or mismatched bucket must never be counted as Other. */
export function combineServerVolume(total: MetricsBucket[], names: string[], summaries: Pick<GatewayUsageMetrics, 'timeseries'>[]): ServerVolume {
  if (names.length !== summaries.length) throw new Error('Server call history is incomplete.')
  const series = summaries.map(summary => {
    if (summary.timeseries.length !== total.length) throw new Error('Server call history has different time buckets.')
    return new Map(summary.timeseries.map(bucket => [bucket.ts_unix * 1000, bucket.calls]))
  })
  const buckets = total.map(bucket => {
    const values = series.map(rows => rows.get(bucket.ts))
    if (values.some(value => value === undefined || !Number.isFinite(value) || value < 0)) throw new Error('Server call history is incomplete.')
    const known = (values as number[]).reduce((sum, value) => sum + value, 0)
    if (known > bucket.calls) throw new Error('Call history changed while loading. Refresh to compare a consistent window.')
    return { ts: bucket.ts, total: bucket.calls, values: [...values as number[], bucket.calls - known] }
  })
  return { names: [...names, 'Other'], buckets }
}

export function useServerVolume(metrics?: DashboardMetrics) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  const context = getBrowserSessionContextIdentity()
  const names = metrics ? [...metrics.upstreams].sort((a, b) => b.calls - a.calls).slice(0, 4).map(row => row.name) : []
  return useSWR(metrics ? ['overview-server-volume', normalizeGatewayApiBase(), context, epoch, metrics.window, metrics.until_ms, ...names] : null, async () => {
    const summaries = await Promise.all(names.map(name => fetchGatewayUsageMetrics(metrics!.window, name, metrics!.until_ms)))
    if (epoch !== getBrowserSessionEpoch() || context !== getBrowserSessionContextIdentity()) throw new DOMException('Authority or project context changed', 'AbortError')
    return combineServerVolume(metrics!.timeseries, names, summaries)
  }, { shouldRetryOnError: false, revalidateOnFocus: false })
}

export function ServerVolumeLegend({ names }: { names: string[] }) {
  return <span className="hidden min-w-0 items-center gap-2 lg:inline-flex">{names.map((name, index) => <span key={name} title={name} className="inline-flex min-w-0 items-center gap-1"><span className="h-[3px] w-2.5 shrink-0 rounded-full" style={{ background: COLORS[index] }} /><span className="max-w-[72px] truncate">{name}</span></span>)}</span>
}

export function ServerVolumeChart({ data, onSelectBucket }: { data: ServerVolume; onSelectBucket: (from: number, to: number) => void }) {
  const maximum = Math.max(1, ...data.buckets.map(bucket => bucket.total))
  const width = data.buckets.length > 1 ? data.buckets[1].ts - data.buckets[0].ts : 1000
  return <div><div className="flex h-[210px] items-end gap-[3px]" aria-label="Calls by server">
    {data.buckets.map((bucket, index) => <button key={bucket.ts} type="button" onClick={() => onSelectBucket(bucket.ts, (data.buckets[index + 1]?.ts ?? bucket.ts + width) - 1000)} title={`${new Date(bucket.ts).toLocaleString()} · ${data.names.map((name, item) => `${name}: ${bucket.values[item]}`).join(' · ')}`} aria-label={`${new Date(bucket.ts).toLocaleString()}: ${bucket.total} calls`} className="flex h-full min-w-0 flex-1 flex-col-reverse justify-start overflow-hidden rounded-t-[3px] transition-[filter] duration-150 hover:brightness-125 focus-visible:outline-2 focus-visible:outline-aurora-accent-primary">
      {bucket.values.map((value, item) => <span key={item} className="block w-full" style={{ height: `${value / maximum * 100}%`, background: COLORS[item] }} />)}
    </button>)}
  </div>{data.buckets.length ? <div className="mt-2 flex justify-between text-[10px] tabular-nums text-aurora-text-muted">{Array.from(new Set(Array.from({ length: Math.min(5, data.buckets.length) }, (_, index) => Math.round(index * (data.buckets.length - 1) / Math.max(1, Math.min(5, data.buckets.length) - 1))))).map(index => <time key={index} dateTime={new Date(data.buckets[index].ts).toISOString()}>{new Date(data.buckets[index].ts).toLocaleString(undefined, width < 86_400_000 ? { hour: '2-digit', minute: '2-digit' } : { month: 'short', day: 'numeric' })}</time>)}</div> : null}</div>
}
