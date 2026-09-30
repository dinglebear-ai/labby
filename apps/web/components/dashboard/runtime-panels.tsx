'use client'

import useSWR from 'swr'
import { useSyncExternalStore } from 'react'
import { getBrowserSessionContextIdentity, getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { Cable, ArrowDown, ArrowUp, ServerCog } from 'lucide-react'
import { gatewayAction } from '@/lib/api/gateway-client'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { clientApplicationLabel } from '@/lib/dashboard/gateway-usage-adapter'
import { DashboardPanel } from './panel'

/** Serialized GatewayClientView; connected_at is the observed connection time. */
export interface ConnectedClient {
  subject: string | null
  authorized_client_id: string | null
  client_name: string | null
  client_version: string | null
  transport: string
  connected_at: string
  last_seen_at?: string | null
  observation_count?: number
}
export interface HostMetrics { hostname?: string | null; platform?: string; cpu_cores?: number | null; memory_limit_is_cgroup?: boolean; child_process_count?: number | null; child_rss_bytes?: number | null; available: boolean; scope: string; cpu_percent: number | null; memory_used_bytes: number | null; memory_total_bytes: number | null; disk_used_bytes: number | null; disk_total_bytes: number | null; network_rx_bytes_per_second: number | null; network_tx_bytes_per_second: number | null; sample_ms: number }
interface HostHealth { status: string; pid?: number; uptime_s?: number; mode?: string }

export function connectionAge(seconds: number): string {
  const minutes = Math.max(0, Math.floor(seconds / 60))
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ${minutes % 60}m`
  return `${Math.floor(hours / 24)}d ${hours % 24}h`
}

export function overviewHealthUrl(base: string, origin: string): string {
  const api = new URL(base, origin)
  api.pathname = api.pathname.replace(/\/v1(?:\/gateway)?\/?$/, '/health')
  // Custom API mounts still resolve their sibling health route.
  if (!api.pathname.endsWith('/health')) api.pathname = `${api.pathname.replace(/\/[^/]*\/?$/, '')}/health`
  api.search = ''
  api.hash = ''
  return api.href
}

export function overviewRuntimeKey(resource: string, base: string, context: string, epoch: number) {
  return [resource, base, context, epoch] as const
}

export function useOverviewRuntime() {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  const context = getBrowserSessionContextIdentity()
  const base = normalizeGatewayApiBase()
  const lightRefresh = { revalidateOnFocus: true, focusThrottleInterval: 15_000, refreshWhenHidden: false, refreshWhenOffline: false, shouldRetryOnError: false }
  const clients = useSWR<ConnectedClient[]>(overviewRuntimeKey('overview-connected-clients', base, context, epoch), () => gatewayAction('gateway.clients.list', {}), { ...lightRefresh, refreshInterval: 10_000 })
  const health = useSWR<HostHealth>(overviewRuntimeKey('overview-host-health', base, context, epoch), async () => {
    const response = await fetch(overviewHealthUrl(base, window.location.href), { credentials: 'include', cache: 'no-store' })
    if (!response.ok) throw new Error('Host health is unavailable')
    const result: HostHealth = await response.json()
    if (epoch !== getBrowserSessionEpoch() || context !== getBrowserSessionContextIdentity() || base !== normalizeGatewayApiBase()) throw new DOMException('Authority or API target changed', 'AbortError')
    return result
  }, { ...lightRefresh, refreshInterval: 30_000 })
  // Host metrics take a five-second sampling window on Linux. Keep that
  // separate from cheap health and in-memory discovery history reads.
  const host = useSWR<HostMetrics>(overviewRuntimeKey('overview-host-metrics', base, context, epoch), () => gatewayAction('gateway.host.metrics', {}), { ...lightRefresh, refreshInterval: 60_000 })
  return { clients, health, host }
}

export function ConnectedClientsPanel({ clients, unavailable, loading, onRetry }: { clients?: ConnectedClient[]; unavailable?: boolean; loading?: boolean; onRetry?: () => void }) {
  return <DashboardPanel title="Observed clients" icon={<Cable />} iconTone="success" meta="recent observations">
    {unavailable ? <p className="text-xs text-aurora-text-muted">Client observation history is unavailable.{onRetry ? <button type="button" onClick={onRetry} className="ml-2 min-h-11 rounded text-aurora-accent-strong underline focus-visible:outline-2">Retry</button> : null}</p>
      : loading ? <p role="status" className="text-xs text-aurora-text-muted">Loading observed clients…</p>
      : !clients?.length ? <p className="text-xs text-aurora-text-muted">No client activity observed.</p>
      : <ul aria-label="Observed MCP clients" tabIndex={0} className="flex max-h-80 min-w-0 flex-col gap-2 overflow-y-auto overscroll-contain pr-1 focus-visible:outline-2 focus-visible:outline-aurora-accent-primary">
        {/* The registry returns least-recently observed first. Bound the panel,
            not the available records, so busy clients cannot stretch the rail. */}
        {[...clients].reverse().map((client, index) => {
          const lastSeen = client.last_seen_at ?? client.connected_at
          return <li key={`${client.connected_at}:${index}`} className="min-w-0 border-b border-aurora-border-subtle pb-2 last:border-0 last:pb-0">
            <div className="flex min-w-0 items-baseline justify-between gap-2">
              <span className="min-w-0 break-words text-xs font-medium text-aurora-text-primary" title={client.client_name ? `Self-reported client metadata: ${client.client_name}` : undefined}>{clientApplicationLabel(client.client_name)}</span>
              <time dateTime={lastSeen} title={`Last observed: ${lastSeen}`} className="shrink-0 text-[11px] font-semibold tabular-nums text-aurora-text-muted">{Number.isFinite(Date.parse(lastSeen)) ? connectionAge((Date.now() - Date.parse(lastSeen)) / 1000) : '—'}</time>
            </div>
            <p className="mt-1 break-all text-[10px] leading-relaxed text-aurora-text-muted">{[client.client_name && `Reported: ${client.client_name}`, client.client_version && `reported v${client.client_version}`, client.transport].filter(Boolean).join(' · ')}</p>
            {client.authorized_client_id ? <p className="break-all text-[10px] leading-relaxed text-aurora-text-muted">Verified OAuth client {client.authorized_client_id}</p> : null}
            {client.observation_count ? <p className="text-[10px] text-aurora-text-muted">{client.observation_count} observations</p> : null}
          </li>
        })}
      </ul>}
  </DashboardPanel>
}

export function GatewayHostPanel({ health, clients, metrics, loading }: { health?: HostHealth; clients?: ConnectedClient[]; metrics?: HostMetrics; loading?: boolean }) {
  return <DashboardPanel title="Gateway host" icon={<ServerCog />} meta={[metrics?.hostname, metrics?.platform, health?.uptime_s == null ? undefined : `up ${connectionAge(health.uptime_s)}`].filter(Boolean).join(' · ') || undefined} headerStyle={{ padding: '11px 14px', background: 'none' }} bodyStyle={{ padding: '11px 14px' }}>
    <div className="flex flex-col gap-[9px]" aria-label="Host resource metrics" title={metrics?.scope}>
      {[
        { label: 'CPU', value: metrics?.cpu_percent, detail: loading ? 'Sampling…' : metrics?.cpu_percent == null ? 'Unavailable' : `${metrics.cpu_percent.toFixed(1)}%${metrics.cpu_cores ? ` · ${metrics.cpu_cores} cores` : ''}`, title: `Processes observed in both samples · ${metrics?.sample_ms ? (metrics.sample_ms / 1000).toFixed(1) : '—'}s average`, color: 'var(--aurora-accent-primary)' },
        { label: 'Memory', value: metrics?.memory_total_bytes && metrics.memory_used_bytes != null ? metrics.memory_used_bytes / metrics.memory_total_bytes * 100 : null, detail: loading ? 'Sampling…' : metrics?.memory_used_bytes == null ? 'Unavailable' : `${formatHostBytes(metrics.memory_used_bytes)}${metrics.memory_total_bytes == null ? '' : ` / ${formatHostBytes(metrics.memory_total_bytes)}`}`, title: `Daemon + child RSS against ${metrics?.memory_limit_is_cgroup ? 'cgroup limit' : 'host physical memory'}`, color: 'var(--aurora-accent-pink)' },
        { label: 'Disk', value: metrics?.disk_total_bytes && metrics.disk_used_bytes != null ? metrics.disk_used_bytes / metrics.disk_total_bytes * 100 : null, detail: loading ? 'Sampling…' : metrics?.disk_used_bytes == null ? 'Unavailable' : `${formatHostBytes(metrics.disk_used_bytes)}${metrics.disk_total_bytes == null ? '' : ` / ${formatHostBytes(metrics.disk_total_bytes)}`}`, title: 'Filesystem containing the configured Labby data directory', color: 'var(--aurora-warn)' },
      ].map(row => <div key={row.label} title={row.title} className="flex items-center gap-2"><span className="w-[42px] shrink-0 text-[10px] font-bold uppercase tracking-[.09em] text-aurora-text-muted">{row.label}</span><span className="h-1 flex-1 overflow-hidden rounded-full bg-aurora-control-surface"><span className="block h-full rounded-full transition-[width] duration-300" style={{ width: `${Math.min(100, Math.max(0, row.value ?? 0))}%`, background: row.color }} /></span><span className="min-w-[114px] text-right text-[10.5px] text-aurora-text-muted">{row.detail}</span></div>)}
    </div>
    <div className="-mx-[14px] my-px border-t border-aurora-border-subtle" />
    <div className="grid grid-cols-2 gap-2">{[metrics?.network_rx_bytes_per_second, metrics?.network_tx_bytes_per_second].map((value, index) => { const Icon = index ? ArrowUp : ArrowDown; return <div key={index} title={`${index ? 'Outbound' : 'Inbound'} network rate; excludes loopback`} className="flex items-center gap-[7px] rounded-lg border border-aurora-border-subtle bg-aurora-control-surface px-[9px] py-1.5 text-[11px] text-aurora-text-muted"><Icon className="size-3" /><span>{loading ? 'Sampling…' : value == null ? 'Unavailable' : `${formatHostBytes(value)}/s`}</span></div> })}</div>
    <div className="flex flex-wrap gap-2.5 text-[10px] tabular-nums text-aurora-text-muted">
      <span>{clients ? `${clients.length} observed MCP identities` : 'MCP observation history unavailable'}</span>
      {metrics?.child_process_count != null ? <span>{metrics.child_process_count} child processes</span> : null}
      {metrics?.child_rss_bytes != null ? <span>{formatHostBytes(metrics.child_rss_bytes)} child RSS</span> : null}
    </div>
  </DashboardPanel>
}

export function formatHostBytes(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  const index = Math.min(4, Math.max(0, Math.floor(Math.log2(Math.max(1, bytes)) / 10)))
  return `${(bytes / 1024 ** index).toFixed(index ? 1 : 0)} ${units[index]}`
}
