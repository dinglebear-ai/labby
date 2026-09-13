'use client'

import useSWR from 'swr'
import { useSyncExternalStore } from 'react'
import { getBrowserSessionContextIdentity, getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { Cable, ArrowDown, ArrowUp, ServerCog } from 'lucide-react'
import { gatewayAction } from '@/lib/api/gateway-client'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { DashboardPanel } from './panel'

/** Serialized GatewayClientView; connected_at is the observed connection time. */
export interface ConnectedClient {
  subject: string | null
  client_name: string | null
  client_version: string | null
  transport: string
  connected_at: string
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
  const clients = useSWR<ConnectedClient[]>(overviewRuntimeKey('overview-connected-clients', base, context, epoch), () => gatewayAction('gateway.clients.list', {}), { refreshInterval: 30_000, shouldRetryOnError: false })
  const health = useSWR<HostHealth>(overviewRuntimeKey('overview-host-health', base, context, epoch), async () => {
    const response = await fetch(overviewHealthUrl(base, window.location.href), { credentials: 'include', cache: 'no-store' })
    if (!response.ok) throw new Error('Host health is unavailable')
    const result: HostHealth = await response.json()
    if (epoch !== getBrowserSessionEpoch() || context !== getBrowserSessionContextIdentity()) throw new DOMException('Authority or project context changed', 'AbortError')
    return result
  }, { refreshInterval: 30_000, shouldRetryOnError: false })
  const host = useSWR<HostMetrics>(overviewRuntimeKey('overview-host-metrics', base, context, epoch), () => gatewayAction('gateway.host.metrics', {}), { refreshInterval: 30_000, shouldRetryOnError: false })
  return { clients, health, host }
}

export function ConnectedClientsPanel({ clients, unavailable, loading, onRetry }: { clients?: ConnectedClient[]; unavailable?: boolean; loading?: boolean; onRetry?: () => void }) {
  return <DashboardPanel title="Connected clients" icon={<Cable />} iconTone="success" meta="observed sessions">
    {unavailable ? <p className="text-xs text-aurora-text-muted">Connected clients are unavailable.{onRetry ? <button type="button" onClick={onRetry} className="ml-2 rounded text-aurora-accent-strong underline focus-visible:outline-2">Retry</button> : null}</p>
      : loading ? <p role="status" className="text-xs text-aurora-text-muted">Loading connected clients…</p>
      : !clients?.length ? <p className="text-xs text-aurora-text-muted">No connected clients observed.</p>
      : <ul className="flex flex-col gap-[9px]">{clients.map((client, index) => <li key={`${client.connected_at}:${index}`} className="flex min-w-0 items-baseline gap-1.5">
        <span className="min-w-0 truncate text-xs font-medium text-aurora-text-primary">{client.client_name || 'Unnamed client'}</span>
        <span className="min-w-0 flex-1 truncate text-[10px] text-aurora-text-muted">{[client.client_version && `v${client.client_version}`, client.transport].filter(Boolean).join(' · ')}</span>
        <time dateTime={client.connected_at} title={client.connected_at} className="shrink-0 text-[11px] font-semibold tabular-nums text-aurora-text-muted">{Number.isFinite(Date.parse(client.connected_at)) ? connectionAge((Date.now() - Date.parse(client.connected_at)) / 1000) : '—'}</time>
      </li>)}</ul>}
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
      <span>{clients ? `${clients.length} observed MCP sessions` : 'MCP sessions unavailable'}</span>
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
