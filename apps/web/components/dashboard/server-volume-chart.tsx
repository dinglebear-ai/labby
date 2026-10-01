'use client'

import { useSyncExternalStore } from 'react'
import useSWR, { useSWRConfig } from 'swr'
import { fetchServerVolumeMetrics } from '@/lib/api/metrics-client'
import { shouldRetryMetrics } from '@/lib/dashboard/dashboard-load-state'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { getBrowserSessionContextIdentity, getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import type { DashboardMetrics, MetricsBucket } from '@/lib/types/metrics'
import type { GatewayUsageMetrics } from '@/lib/dashboard/gateway-usage-adapter'

const COLORS = ['var(--aurora-accent-primary)', 'var(--aurora-accent-pink)', 'var(--aurora-success)', 'var(--aurora-warn)', 'var(--aurora-accent-deep)']
export type ServerVolume = { sampledAt?: number; names: string[]; buckets: Array<{ ts: number; total: number; values: number[] }> }

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

export const SERVER_VOLUME_REFRESH_MS = 60_000

export function serverVolumeKey(window: DashboardMetrics['window'] | undefined, base: string, context: string, epoch: number) {
  // Keep the key bounded: a new aggregate timestamp must not create a new
  // cache entry and fan out four additional aggregate queries every pulse.
  return window ? ['overview-server-volume', base, context, epoch, window] as const : null
}

/** A provider owns at most four window entries. Every automatic trigger enters
 * here, including SWR remounts, focus, reconnect, and timer revalidation. */
export function createServerVolumeSampler(load: (window: DashboardMetrics['window'], signal: AbortSignal) => Promise<ServerVolume>) {
  const entries = new Map<DashboardMetrics['window'], { deadline: number; flight: Promise<ServerVolume> }>()
  const controllers = new Set<AbortController>()
  return {
    clear() { for (const controller of controllers) controller.abort(); controllers.clear(); entries.clear() },
    fetch(window: DashboardMetrics['window']) {
      const existing = entries.get(window)
      if (existing && Date.now() < existing.deadline) return existing.flight
      const controller = new AbortController()
      controllers.add(controller)
      const flight = load(window, controller.signal).finally(() => {
        controllers.delete(controller)
        entry.deadline = Date.now() + SERVER_VOLUME_REFRESH_MS
      })
      const entry = { deadline: Infinity, flight }
      entries.set(window, entry)
      return flight
    },
  }
}

const serverSamplers = new WeakMap<object, { scope: string; sampler: ReturnType<typeof createServerVolumeSampler> }>()

export function useServerVolume(metrics?: DashboardMetrics) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  const context = getBrowserSessionContextIdentity()
  const base = normalizeGatewayApiBase()
  const { cache } = useSWRConfig()
  const scope = JSON.stringify([base, context, epoch])
  let owned = serverSamplers.get(cache)
  if (!owned || owned.scope !== scope) {
    owned?.sampler.clear()
    owned = { scope, sampler: createServerVolumeSampler(async (window, signal) => {
      const summary = await fetchServerVolumeMetrics(window, { baseUrl: base, signal })
      if (signal.aborted || epoch !== getBrowserSessionEpoch() || context !== getBrowserSessionContextIdentity() || base !== normalizeGatewayApiBase()) throw new DOMException('Authority or API target changed', 'AbortError')
      const names = summary.upstreams.map(row => row.upstream).filter(name => Object.hasOwn(summary.upstream_timeseries!, name)).slice(0, 4)
      return { ...combineServerVolume(summary.timeseries.map(bucket => ({ ts: bucket.ts_unix * 1000, calls: bucket.calls, failed: bucket.failed })), names, names.map(name => ({ timeseries: summary.upstream_timeseries![name] }))), sampledAt: Date.now() }
    }) }
    serverSamplers.set(cache, owned)
  }
  const sampler = owned.sampler
  return useSWR(serverVolumeKey(metrics?.window, base, context, epoch), () => sampler.fetch(metrics!.window), {
    refreshInterval: SERVER_VOLUME_REFRESH_MS,
    refreshWhenHidden: false,
    refreshWhenOffline: false,
    revalidateOnFocus: true,
    focusThrottleInterval: SERVER_VOLUME_REFRESH_MS,
    keepPreviousData: false,
    shouldRetryOnError: shouldRetryMetrics,
    errorRetryInterval: SERVER_VOLUME_REFRESH_MS,
    errorRetryCount: 2,
  })
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
  </div>{data.sampledAt !== undefined ? <p className="mt-2 text-[10px] text-aurora-text-muted">Server breakdown sampled <time dateTime={new Date(data.sampledAt).toISOString()}>{new Date(data.sampledAt).toLocaleTimeString()}</time> · refreshes at most once per minute.</p> : null}{data.buckets.length ? <div className="mt-2 flex justify-between text-[10px] tabular-nums text-aurora-text-muted">{Array.from(new Set(Array.from({ length: Math.min(5, data.buckets.length) }, (_, index) => Math.round(index * (data.buckets.length - 1) / Math.max(1, Math.min(5, data.buckets.length) - 1))))).map(index => <time key={index} dateTime={new Date(data.buckets[index].ts).toISOString()}>{new Date(data.buckets[index].ts).toLocaleString(undefined, width < 86_400_000 ? { hour: '2-digit', minute: '2-digit' } : { month: 'short', day: 'numeric' })}</time>)}</div> : null}</div>
}
