'use client'

import { useMemo, useState } from 'react'
import useSWR from 'swr'
import {
  Activity,
  AlertTriangle,
  ChevronRight,
  Clock3,
  Database,
  GitBranch,
  Network,
  RefreshCw,
  Search,
} from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { queryServerLogs, ServerLogsApiError } from '@/lib/api/server-logs-client'
import { buildTraceSummary } from '@/lib/observability/trace-model'
import { formatDuration, formatRelativeTime } from '@/lib/dashboard/dashboard-metrics'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'

const TRACE_QUERY_LIMIT = 500

function valueLabel(value: unknown): string {
  if (value === null || value === undefined) return '—'
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  return JSON.stringify(value)
}

export default function TracesPage() {
  const [search, setSearch] = useState('')
  const { data, error, isLoading, isValidating, mutate } = useSWR(
    ['server-traces', TRACE_QUERY_LIMIT],
    () => queryServerLogs({
      limit: TRACE_QUERY_LIMIT,
      max_scan_bytes: 4 * 1024 * 1024,
      stop_after_limit: true,
      correlated_only: true,
    }),
    { refreshInterval: 30_000, revalidateOnFocus: false },
  )
  const summary = useMemo(
    () => buildTraceSummary(data?.entries ?? [], { truncated: data?.truncated ?? false }),
    [data],
  )
  const traceErrorMessage = error instanceof ServerLogsApiError && error.status === 403
    ? 'Retained traces require the lab:admin scope.'
    : error instanceof Error
      ? error.message
      : 'Could not load retained traces.'
  const needle = search.trim().toLowerCase()
  const traces = needle
    ? summary.traces.filter((trace) =>
        `${trace.id} ${trace.surface} ${trace.service} ${trace.action} ${trace.error_kind ?? ''} ${trace.upstreams.join(' ')}`
          .toLowerCase()
          .includes(needle),
      )
    : summary.traces

  const heroStats = [
    { label: 'Requests', value: summary.total, icon: <GitBranch size={12} /> },
    { label: 'Failed', value: summary.failed, icon: <AlertTriangle size={12} /> },
    { label: 'P50', value: formatDuration(summary.p50_ms), icon: <Clock3 size={12} /> },
    { label: 'P95', value: formatDuration(summary.p95_ms), icon: <Activity size={12} /> },
    { label: 'Upstreams', value: summary.upstreams.length, icon: <Network size={12} /> },
  ]

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Traces' }]} />
      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
        <ConsoleHero
          eyebrow="Observe"
          title="Request Traces"
          actions={
            <Button variant="outline" size="sm" onClick={() => void mutate()} disabled={isValidating}>
              <RefreshCw className={cn('mr-2 size-4', isValidating && 'animate-spin')} />
              Refresh
            </Button>
          }
          stats={heroStats}
        />

        <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_280px]">
          <DashboardPanel
            title="Request flow"
            icon={<GitBranch className="size-4" />}
            meta={data?.truncated ? 'Bounded log sample' : `${traces.length} correlated requests`}
          >
            <div className="relative mb-3">
              <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
              <Input
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder="Search request, service, action, upstream, or error…"
                className="pl-9"
              />
            </div>

            <div className="mb-2 hidden grid-cols-[88px_minmax(0,1fr)_70px_64px_20px] gap-3 border-b border-aurora-border-subtle px-3 pb-2 text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted sm:grid">
              <span>Started</span><span>Request</span><span>Surface</span><span className="text-right">Duration</span><span />
            </div>

            {error && !data ? (
              <div className="rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/8 p-4 text-sm text-aurora-error">
                {traceErrorMessage}
              </div>
            ) : traces.length === 0 ? (
              <div className="py-10 text-center text-sm text-aurora-text-muted">
                {isLoading ? 'Loading retained traces…' : 'No request traces match this search.'}
              </div>
            ) : (
              <div className="divide-y divide-aurora-border-subtle overflow-hidden rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-low/55">
                {traces.map((trace) => (
                  <details
                    key={trace.id}
                    className="group overflow-hidden transition-colors open:bg-aurora-selected-bg/35 hover:bg-aurora-hover-bg"
                  >
                    <summary className="grid min-h-14 cursor-pointer list-none grid-cols-[88px_minmax(0,1fr)_70px_64px_20px] items-center gap-3 px-3 py-2.5 [&::-webkit-details-marker]:hidden">
                      <span className="text-[11px] tabular-nums text-aurora-text-muted">
                        {formatRelativeTime(trace.started_at)}
                      </span>
                      <span className="min-w-0">
                        <span className="flex min-w-0 items-center gap-2">
                          <span className={cn(
                            'size-2 shrink-0 rounded-full',
                            trace.outcome === 'ok' ? 'bg-aurora-success' : trace.outcome === 'failed' ? 'bg-aurora-error' : 'bg-aurora-warn',
                          )} />
                          <span className="truncate font-mono text-[12px] font-semibold text-aurora-text-primary">
                            {trace.service}.{trace.action}
                          </span>
                        </span>
                        <span className="mt-1 block truncate font-mono text-[10px] text-aurora-text-muted">
                          {trace.id} {trace.upstreams.length > 0 ? `· ${trace.upstreams.join(', ')}` : ''}
                        </span>
                      </span>
                      <Badge variant="outline" className="justify-self-start">{trace.surface}</Badge>
                      <span className="text-right text-[11px] tabular-nums text-aurora-text-muted">{formatDuration(trace.elapsed_ms)}</span>
                      <ChevronRight className="size-4 text-aurora-text-muted transition-transform group-open:rotate-90" />
                    </summary>
                    <div className="border-t border-aurora-border-subtle bg-aurora-page-bg/25 px-4 py-4">
                      <div className="mb-3 flex flex-wrap gap-2 text-[10px] text-aurora-text-muted">
                        <Badge variant="outline">{trace.events.length} events</Badge>
                        {trace.actor_key ? <Badge variant="outline">actor {trace.actor_key.slice(0, 12)}</Badge> : null}
                        {trace.error_kind ? <Badge variant="outline" status="error">{trace.error_kind}</Badge> : null}
                        {trace.response_bytes > 0 ? <Badge variant="outline">{trace.response_bytes.toLocaleString()} bytes</Badge> : null}
                        {trace.input_tokens + trace.output_tokens > 0 ? (
                          <Badge variant="outline">{trace.input_tokens + trace.output_tokens} tokens</Badge>
                        ) : null}
                      </div>
                      <ol className="relative ml-2 border-l border-aurora-border-strong pl-4">
                        {trace.events.map((event, index) => (
                          <li key={`${event.file}:${event.timestamp}:${index}`} className="relative pb-3 last:pb-0">
                            <span className="absolute -left-[19px] top-1 size-2 rounded-full border border-aurora-accent-primary/50 bg-aurora-page-bg" />
                            <div className="flex flex-wrap items-baseline justify-between gap-2">
                              <span className="font-mono text-[11px] text-aurora-text-primary">
                                {event.message ?? `${event.service ?? 'runtime'}.${event.action ?? 'event'}`}
                              </span>
                              <span className="text-[10px] tabular-nums text-aurora-text-muted">
                                {event.timestamp ? new Date(event.timestamp).toLocaleTimeString() : 'No timestamp'}
                              </span>
                            </div>
                            <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-[9.5px] text-aurora-text-muted">
                              {['event', 'upstream', 'operation', 'kind', 'elapsed_ms', 'response_bytes'].map((key) =>
                                event.fields[key] === undefined ? null : (
                                  <span key={key}><span className="text-aurora-text-subtle">{key}</span> {valueLabel(event.fields[key])}</span>
                                ),
                              )}
                            </div>
                          </li>
                        ))}
                      </ol>
                    </div>
                  </details>
                ))}
              </div>
            )}
          </DashboardPanel>

          <div className="space-y-4">
            <DashboardPanel title="Surfaces" icon={<Activity className="size-4" />}>
              <RankedList items={summary.surfaces} empty="No surface fields in the retained window." />
            </DashboardPanel>
            <DashboardPanel title="Upstreams" icon={<Database className="size-4" />}>
              <RankedList items={summary.upstreams} empty="No upstream calls in the retained window." />
            </DashboardPanel>
            <DashboardPanel title="Collection" icon={<Network className="size-4" />}>
              <dl className="space-y-2 text-[11px]">
                <CollectionRow label="Log entries" value={data?.matched ?? 0} />
                <CollectionRow label="Scanned lines" value={data?.scanned_lines ?? 0} />
                <CollectionRow label="Malformed" value={data?.malformed_lines ?? 0} />
                <CollectionRow label="Incomplete flows" value={summary.incomplete} />
              </dl>
            </DashboardPanel>
          </div>
        </div>
      </div>
    </>
  )
}

function RankedList({ items, empty }: { items: Array<{ name: string; count: number }>; empty: string }) {
  if (items.length === 0) return <p className="text-[11px] text-aurora-text-muted">{empty}</p>
  const max = items[0]?.count ?? 1
  return (
    <div className="space-y-2.5">
      {items.slice(0, 8).map((item) => (
        <div key={item.name}>
          <div className="mb-1 flex justify-between gap-2 text-[11px]">
            <span className="truncate text-aurora-text-primary">{item.name}</span>
            <span className="tabular-nums text-aurora-text-muted">{item.count}</span>
          </div>
          <div className="h-1 overflow-hidden rounded-full bg-aurora-border-subtle">
            <div className="h-full rounded-full bg-aurora-accent-primary" style={{ width: `${Math.max(4, item.count / max * 100)}%` }} />
          </div>
        </div>
      ))}
    </div>
  )
}

function CollectionRow({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <dt className="text-aurora-text-muted">{label}</dt>
      <dd className="font-mono tabular-nums text-aurora-text-primary">{value.toLocaleString()}</dd>
    </div>
  )
}
