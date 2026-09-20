'use client'

import { useState } from 'react'
import { ChevronDown } from 'lucide-react'
import type { GatewayUsageMetrics } from '@/lib/dashboard/gateway-usage-adapter'
import type { ToolCallPage } from '@/lib/types/metrics'
import { formatDuration, formatRelativeTime } from '@/lib/dashboard/dashboard-metrics'
import { getErrorMessage } from '@/lib/utils'
import { readableTarget } from '@/lib/dashboard/readable-target'
import { DetailCard } from './gateway-detail-chrome'

/** Timestamped API buckets retain the actual rolling-window boundaries. */
export function gatewayCallSeries(metrics: Pick<GatewayUsageMetrics, 'timeseries'>) {
  return [...metrics.timeseries].sort((a, b) => a.ts_unix - b.ts_unix).map((bucket) => ({
    ts: bucket.ts_unix * 1000,
    calls: bucket.calls,
    failed: bucket.failed,
    succeeded: Math.max(0, bucket.calls - bucket.failed),
  }))
}

const headerClass = 'flex items-center justify-between gap-3 border-b border-aurora-border-default px-4 py-2.5'
const labelClass = 'text-[10.5px] font-bold uppercase tracking-[.15em] text-aurora-text-muted'

export function GatewayActivityPanels({ data, metrics, error, metricsError, isLoading, onOpenActivity, onRetry, detailed = false }: {
  detailed?: boolean
  data?: ToolCallPage
  metrics?: GatewayUsageMetrics
  error?: unknown
  metricsError?: unknown
  isLoading: boolean
  onOpenActivity: (callId?: string) => void
  onRetry: () => void
}) {
  const [chartMode, setChartMode] = useState<'outcomes' | 'calls'>('outcomes')
  const rows = metrics ? gatewayCallSeries(metrics) : []
  const maximum = Math.max(1, ...rows.map((row) => row.calls))
  const line = (key: 'calls' | 'failed' | 'succeeded') => rows.map((row, index) => `${index * 100 / Math.max(1, rows.length - 1)},${100 - row[key] * 92 / maximum}`).join(' ')
  const breakdown = chartMode === 'outcomes'
  const loadingText = isLoading ? 'Loading call telemetry…' : error ? getErrorMessage(error, 'Call telemetry could not be loaded') : 'No calls in the last 24 hours'
  return <div className={detailed ? "grid gap-3.5 lg:grid-cols-2" : "grid gap-3.5 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]"}>
    {detailed ? <DetailCard padding="0" className="overflow-hidden"><div className={headerClass} style={{ background: 'var(--gw0-0_38)' }}><span className={labelClass}>Calls by tool</span><span className="text-[10px] text-aurora-text-muted">Last 24h</span></div><div className="p-4">{metrics && !metricsError ? metrics.top_tools.length ? metrics.top_tools.map((tool) => <button key={tool.tool} type="button" onClick={() => onOpenActivity()} className="mb-3 block w-full text-left"><div className="mb-1.5 flex justify-between gap-3 text-[11px]"><span className="truncate" title={tool.tool}>{readableTarget(tool.tool)}</span><span className="tabular-nums text-aurora-text-muted">{tool.calls.toLocaleString()}</span></div><div className="h-1.5 overflow-hidden rounded-full bg-aurora-control-surface"><div className="h-full rounded-full bg-aurora-accent-primary" style={{ width: `${tool.calls / Math.max(1, ...metrics.top_tools.map((row) => row.calls)) * 100}%` }}/></div></button>) : <p className="py-8 text-center text-xs text-aurora-text-muted">No calls in the last 24 hours</p> : <p className="py-8 text-center text-xs text-aurora-text-muted">{metricsError ? getErrorMessage(metricsError, 'Tool usage could not be loaded') : 'Loading tool usage…'}</p>}</div></DetailCard> : (
    <DetailCard padding="0" className="overflow-hidden">
      <div className={headerClass} style={{ background: 'var(--gw0-0_38)' }}>
        <label className="relative inline-flex min-w-0 items-center gap-1.5">
          <select aria-label="Server activity chart" value={chartMode} onChange={(event) => setChartMode(event.target.value as 'outcomes' | 'calls')} className={`${labelClass} min-w-0 appearance-none bg-transparent pr-4 outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary`}>
            <option value="outcomes">Tool calls · success vs errors</option>
            <option value="calls">Tool calls · total</option>
          </select>
          <ChevronDown size={11} className="pointer-events-none absolute right-0 text-aurora-text-muted"/>
        </label>
        <span className="flex shrink-0 items-center gap-3 text-[10.5px] text-aurora-text-muted"><span className="flex items-center gap-1"><i className="h-0.5 w-2.5 bg-aurora-accent-primary"/>{breakdown ? 'Success' : 'Calls'}</span>{breakdown ? <span className="flex items-center gap-1"><i className="h-0.5 w-2.5 bg-aurora-error"/>Errors</span> : null}</span>
      </div>
      <div className="relative h-44 px-4 pb-6 pt-4">
        {metrics && !metricsError && metrics.total_calls > 0 ? <>
          <svg viewBox="0 0 100 100" preserveAspectRatio="none" role="img" aria-label="Server tool calls over the last 24 hours" className="h-full overflow-visible" style={{ width: 'calc(100% - 24px)' }}>
            {[0, 50, 100].map((y) => <line key={y} x1="0" x2="100" y1={y} y2={y} stroke="var(--aurora-chart-grid)" strokeWidth="1" vectorEffect="non-scaling-stroke" strokeDasharray={y === 100 ? undefined : '3 4'}/>)}
            <polygon points={`0,100 ${line(breakdown ? 'succeeded' : 'calls')} 100,100`} fill="color-mix(in srgb, var(--aurora-accent-primary) 10%, transparent)"/>
            <polyline points={line(breakdown ? 'succeeded' : 'calls')} fill="none" stroke="var(--aurora-accent-primary)" strokeWidth="1.7" vectorEffect="non-scaling-stroke" strokeLinejoin="round"/>
            {breakdown ? <polyline points={line('failed')} fill="none" stroke="var(--aurora-error)" strokeWidth="1.4" vectorEffect="non-scaling-stroke" strokeLinejoin="round"/> : null}
          </svg>
          <div className="absolute right-2 top-4 bottom-6 flex flex-col justify-between text-right text-[9.5px] tabular-nums text-aurora-text-muted"><span>{maximum}</span><span>{Math.round(maximum / 2)}</span><span>0</span></div>
          <div className="absolute left-4 right-10 bottom-2 flex justify-between text-[9.5px] tabular-nums text-aurora-text-muted">{[rows[0], rows[Math.floor(rows.length / 2)], rows.at(-1)].map((row, index) => <span key={index}>{row ? new Date(row.ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '—'}</span>)}</div>
        </> : <div className="grid h-full place-content-center gap-2 text-center text-xs text-aurora-text-muted"><span>{metricsError ? getErrorMessage(metricsError, 'Call chart could not be loaded') : metrics ? 'No calls in the last 24 hours' : 'Loading call chart…'}</span>{metricsError ? <button type="button" onClick={onRetry} className="text-aurora-accent-strong">Retry</button> : null}</div>}
      </div>

    </DetailCard>
    )}
    <DetailCard padding="0" className="overflow-hidden">
      <div className={headerClass} style={{ background: 'var(--gw0-0_38)' }}><span className={labelClass}>Recent calls</span><button type="button" onClick={() => onOpenActivity()} className="text-[10.5px] font-semibold text-aurora-text-muted hover:text-aurora-accent-strong">View Activity →</button></div>
      {detailed && data && !error && data.calls.length ? <div className="overflow-x-auto"><table className="w-full text-left text-[10.5px]"><thead><tr className="border-b border-aurora-border-subtle text-[9px] uppercase text-aurora-text-muted">{['Time', 'Tool', 'Client', 'Duration', 'Tokens', 'Status'].map((label) => <th key={label} className="px-2.5 py-2 font-semibold">{label}</th>)}</tr></thead><tbody>{data.calls.map((call) => <tr key={call.id} className="border-b border-aurora-border-subtle last:border-0"><td className="whitespace-nowrap px-2.5 py-2 tabular-nums text-aurora-text-muted" title={new Date(call.ts).toLocaleString()}>{new Date(call.ts).toLocaleTimeString([], { hour12: false })}</td><td className="max-w-36 truncate px-2.5 py-2"><button type="button" title={call.tool} className="hover:text-aurora-accent-strong" onClick={() => onOpenActivity(call.id)}>{readableTarget(call.tool.split('::').at(-1) ?? call.tool)}</button></td><td className="max-w-24 truncate px-2.5 py-2 text-aurora-text-muted">{call.agent_label || '—'}</td><td className="px-2.5 py-2 tabular-nums">{formatDuration(call.elapsed_ms)}</td><td className="px-2.5 py-2 tabular-nums" title={data.collected.tokens ? undefined : 'Per-call tokens are not collected'}>{data.collected.tokens ? (call.input_tokens + call.output_tokens).toLocaleString() : '—'}</td><td className="px-2.5 py-2"><span className={`rounded px-1 py-0.5 text-[9px] font-bold uppercase ${call.outcome === 'ok' ? 'bg-aurora-success/10 text-aurora-success' : 'bg-aurora-error/10 text-aurora-error'}`}>{call.outcome === 'ok' ? 'OK' : 'Error'}</span></td></tr>)}</tbody></table></div> :
      data && !error && data.calls.length ? <div>{data.calls.slice(0, 5).map((call) => <button key={call.id} type="button" onClick={() => onOpenActivity(call.id)} className="flex w-full items-center gap-2 border-b border-aurora-border-subtle px-4 py-2 text-left last:border-0 hover:bg-aurora-hover-bg">
        <span className="w-14 shrink-0 text-[10px] tabular-nums text-aurora-text-muted" title={new Date(call.ts).toLocaleString()}>{formatRelativeTime(call.ts)}</span>
        <span title={call.tool} className="min-w-0 flex-1 truncate text-[11px] font-semibold text-aurora-text-primary">{readableTarget(call.tool.split('::').at(-1) ?? call.tool)}{call.action ? ` · ${call.action}` : ''}</span>
        <span className="max-w-20 truncate text-[10px] text-aurora-text-muted" title={call.agent_label}>{call.agent_label || '—'}</span>
        <span className="text-[10px] tabular-nums text-aurora-text-muted">{formatDuration(call.elapsed_ms)}</span>
        <span className={`rounded px-1 py-0.5 text-[9px] font-bold uppercase ${call.outcome === 'ok' ? 'bg-aurora-success/10 text-aurora-success' : 'bg-aurora-error/10 text-aurora-error'}`}>{call.outcome === 'ok' ? 'OK' : 'Error'}</span>
      </button>)}</div> : <div className="grid h-44 place-items-center px-4 text-center text-xs text-aurora-text-muted">{loadingText}</div>}
    </DetailCard>
  </div>
}
