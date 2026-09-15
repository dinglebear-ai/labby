'use client'

import { useEffect, useState } from 'react'
import { Copy } from 'lucide-react'
import { toast } from 'sonner'
import { useToolCalls } from '@/lib/hooks/use-usage-drilldown'
import { resolveGatewayEndpoint } from '@/components/settings/SettingsOverview'
import { setupApi } from '@/lib/api/setup-client'
import { getErrorMessage } from '@/lib/utils'
import { connectionAge, useOverviewRuntime } from '@/components/dashboard/runtime-panels'

export function GatewayFleetMetadata() {
  const { health } = useOverviewRuntime()
  const { data } = useToolCalls({ window: '1h', limit: 1 })
  const [endpoint, setEndpoint] = useState<string>()
  useEffect(() => {
    const controller = new AbortController()
    void setupApi.settingsState('surfaces', controller.signal).then((settings) => { if (!controller.signal.aborted) setEndpoint(resolveGatewayEndpoint(settings.values)) }, () => undefined)
    return () => controller.abort()
  }, [])
  const endpointLabel = (() => { if (!endpoint) return undefined; try { const parsed = new URL(endpoint); return `${parsed.host} · ${parsed.pathname}` } catch { return endpoint } })()
  return <div className="flex flex-wrap items-center justify-end gap-2.5 text-[11px] text-aurora-text-muted">
    <span title={health.error ? 'Host uptime unavailable' : 'Gateway host uptime'}>{health.data?.uptime_s !== undefined ? `up ${connectionAge(health.data.uptime_s)}` : 'uptime —'}</span>
    <span title="Gateway health does not report a version">version —</span>
    <span title="Retained upstream calls per minute, averaged over the last hour" className="inline-flex items-baseline gap-1 tabular-nums"><strong className="font-display text-sm font-extrabold text-aurora-text-primary">{data ? data.analytics.avg_per_min.toLocaleString(undefined, { maximumFractionDigits: 1 }) : '—'}</strong><span className="text-[10px]">calls/min · 1h avg</span></span>
    {endpoint ? <button type="button" title="Copy gateway endpoint URL" className="group inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 hover:bg-aurora-hover-bg hover:text-aurora-text-primary" onClick={() => { void navigator.clipboard.writeText(endpoint).then(() => toast.success('Gateway endpoint copied'), (error: unknown) => toast.error(getErrorMessage(error, 'Could not copy endpoint'))) }}>{endpointLabel}<Copy size={10} className="opacity-0 group-hover:opacity-100 group-focus-visible:opacity-100"/></button> : <span title="No gateway endpoint is advertised in surface settings">endpoint —</span>}
  </div>
}
