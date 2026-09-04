'use client'

import { Suspense, useEffect, useMemo, useState } from 'react'
import { useRouter, useSearchParams } from 'next/navigation'
import Link from 'next/link'
import {
  Activity,
  AlertTriangle,
  ChevronLeft,
  ChevronRight,
  Clock,
  Gauge,
  Network,
  Search,
  SlidersHorizontal,
  Users,
  Wrench,
  X,
  Zap,
} from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Skeleton } from '@/components/ui/skeleton'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { WindowSelector } from '@/components/dashboard/window-selector'
import { OutcomeDot, SurfaceTag } from '@/components/dashboard/recent-calls'
import { UsageCallCards } from '@/components/dashboard/usage-call-cards'
import { useToolCalls } from '@/lib/hooks/use-usage-drilldown'
import {
  WINDOW_LABELS,
  formatCompactNumber,
  formatDuration,
  formatRelativeTime,
} from '@/lib/dashboard/dashboard-metrics'
import { METRICS_WINDOWS, type CallOutcome, type MetricsWindow } from '@/lib/types/metrics'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { cn, getErrorMessage } from '@/lib/utils'

const PAGE_SIZE = 50
const ALL = 'all'
const SEARCH_DEBOUNCE_MS = 300

function isWindow(value: string | null): value is MetricsWindow {
  return value !== null && (METRICS_WINDOWS as readonly string[]).includes(value)
}

function parseEpochMs(value: string | null): number | undefined {
  if (!value) return undefined
  const parsed = Number.parseInt(value, 10)
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : undefined
}

function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value)

  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), delayMs)
    return () => window.clearTimeout(timer)
  }, [delayMs, value])

  return debounced
}

function formatBytes(value: number | null | undefined) {
  if (value === null || value === undefined) return '—'
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}

function formatSliceTime(value: number) {
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(value)
}

function UsageExplorer() {
  const params = useSearchParams()
  const router = useRouter()
  const initialWindow = isWindow(params.get('window')) ? (params.get('window') as MetricsWindow) : '24h'

  const focus = params.get('focus')
  const focusPercentile = params.get('percentile')
  const focusMetric = params.get('metric')
  const focusHour = params.get('hour')
  const [window, setWindow] = useState<MetricsWindow>(initialWindow)
  const [upstream, setUpstream] = useState<string>(params.get('upstream') ?? ALL)
  const [tool, setTool] = useState<string>(params.get('tool') ?? ALL)
  const [capability, setCapability] = useState<string>(params.get('capability') ?? ALL)
  const [operation, setOperation] = useState<string>(params.get('operation') ?? ALL)
  const [subjectScope, setSubjectScope] = useState<string>(params.get('subject') ?? ALL)
  const [agent, setAgent] = useState<string>(params.get('agent') ?? ALL)
  const [ip, setIp] = useState<string>(params.get('ip') ?? ALL)
  const [outcome, setOutcome] = useState<string>(params.get('outcome') ?? ALL)
  const [errorKind, setErrorKind] = useState<string>(params.get('error') ?? ALL)
  const [search, setSearch] = useState(params.get('search') ?? '')
  const [sinceMs, setSinceMs] = useState<number | undefined>(() => parseEpochMs(params.get('from')))
  const [untilMs, setUntilMs] = useState<number | undefined>(() => parseEpochMs(params.get('to')))
  const [cursorStack, setCursorStack] = useState<Array<string | null>>([null])
  const debouncedSearch = useDebouncedValue(search.trim(), SEARCH_DEBOUNCE_MS)
  const cursor = cursorStack[cursorStack.length - 1] ?? undefined
  const pageIndex = cursorStack.length - 1
  const resetPaging = () => setCursorStack([null])

  useEffect(() => {
    const next = new URLSearchParams()
    if (window !== '24h') next.set('window', window)
    if (upstream !== ALL) next.set('upstream', upstream)
    if (tool !== ALL) next.set('tool', tool)
    if (capability !== ALL) next.set('capability', capability)
    if (operation !== ALL) next.set('operation', operation)
    if (subjectScope !== ALL) next.set('subject', subjectScope)
    if (agent !== ALL) next.set('agent', agent)
    if (ip !== ALL) next.set('ip', ip)
    if (outcome !== ALL) next.set('outcome', outcome)
    if (errorKind !== ALL) next.set('error', errorKind)
    if (debouncedSearch) next.set('search', debouncedSearch)
    if (sinceMs !== undefined) next.set('from', String(sinceMs))
    if (untilMs !== undefined) next.set('to', String(untilMs))
    if (focus) next.set('focus', focus)
    if (focusPercentile) next.set('percentile', focusPercentile)
    if (focusMetric) next.set('metric', focusMetric)
    if (focusHour) next.set('hour', focusHour)
    const query = next.toString()
    router.replace(query ? `/usage/?${query}` : '/usage/', { scroll: false })
  }, [agent, capability, debouncedSearch, errorKind, focus, focusHour, focusMetric, focusPercentile, ip, operation, outcome, router, sinceMs, subjectScope, tool, untilMs, upstream, window])

  const { data, isLoading, error, mutate } = useToolCalls({
    window,
    since_ms: sinceMs,
    until_ms: untilMs,
    upstream: upstream === ALL ? undefined : upstream,
    tool: tool === ALL ? undefined : tool,
    capability: capability === ALL ? undefined : capability,
    operation: operation === ALL ? undefined : operation,
    subject_scoped: subjectScope === ALL ? undefined : subjectScope === 'subject',
    agent: agent === ALL ? undefined : agent,
    ip: ip === ALL ? undefined : ip,
    outcome: outcome === ALL ? undefined : (outcome as CallOutcome),
    error_kind: errorKind === ALL ? undefined : errorKind,
    search: debouncedSearch || undefined,
    limit: PAGE_SIZE,
    cursor,
  })
  const upstreamOptions = data?.facets.upstreams ?? []
  const toolOptions = data?.facets.tools ?? []
  const capabilityOptions = data?.facets.capabilities ?? []
  const operationOptions = data?.facets.operations ?? []
  const agentOptions = useMemo(
    () => (data?.facets.agents ?? []).map((entry) => [entry.id, entry.label] as const),
    [data],
  )
  const errorOptions = useMemo(
    () => (data?.facets.outcomes ?? []).filter((entry) => entry !== 'ok'),
    [data],
  )
  const ipOptions = data?.facets.ips ?? []
  const collected = data?.collected
  const showIps = collected?.ips ?? false
  const showSurfaces = collected?.surfaces ?? false
  const showTokens = collected?.tokens ?? false
  const tableColumns = 6 + Number(showSurfaces) + Number(showTokens)

  const filtered = data?.filtered ?? 0
  const showingFrom = filtered === 0 ? 0 : pageIndex * PAGE_SIZE + 1
  const showingTo = Math.min(pageIndex * PAGE_SIZE + (data?.calls.length ?? 0), filtered)
  const hasTimeSlice = sinceMs !== undefined || untilMs !== undefined

  const heroStats = [
    {
      label: 'Matched',
      value: data ? formatCompactNumber(data.filtered) : '—',
      icon: <Activity size={12} strokeWidth={1.8} />,
    },
    {
      label: 'In window',
      value: data ? formatCompactNumber(data.total) : '—',
      icon: <Clock size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Failed',
      value: data ? formatCompactNumber(data.analytics.failed) : '—',
      icon: <AlertTriangle size={12} strokeWidth={1.8} />,
    },
    {
      label: 'P95 latency',
      value: data ? formatDuration(data.analytics.p95_elapsed_ms) : '—',
      icon: <Gauge size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Peak / min',
      value: data ? formatCompactNumber(data.analytics.peak_per_min) : '—',
      icon: <Zap size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Targets',
      value: data ? data.facets.tools.length : '—',
      icon: <Wrench size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Agents',
      value: data ? data.facets.agents.length : '—',
      icon: <Users size={12} strokeWidth={1.8} />,
    },
    ...(showIps ? [{
      label: 'Source IPs',
      value: data ? data.facets.ips.length : '—',
      icon: <Network size={12} strokeWidth={1.8} />,
    }] : []),
  ]

  const tableMeta = isLoading && !data
    ? 'Loading…'
    : `${formatCompactNumber(filtered)} matching call${filtered === 1 ? '' : 's'}${
        data ? ` of ${formatCompactNumber(data.total)}` : ''
      }${filtered > 0 ? ` · ${showingFrom}–${showingTo}` : ''}`

  const focusMessage = data && focus ? (() => {
    if (focus === 'latency') {
      const percentile = focusPercentile === 'p50' || focusPercentile === 'p99' ? focusPercentile : 'p95'
      const value = percentile === 'p50'
        ? data.analytics.p50_elapsed_ms
        : percentile === 'p99'
          ? data.analytics.p99_elapsed_ms
          : data.analytics.p95_elapsed_ms
      return `${percentile.toUpperCase()} latency is ${formatDuration(value)} across ${formatCompactNumber(data.filtered)} matching calls.`
    }
    if (focus === 'throughput') {
      if (focusMetric === 'peak') return `Peak throughput is ${formatCompactNumber(data.analytics.peak_per_min)} calls/minute.`
      if (focusMetric === 'average') return `Average throughput is ${data.analytics.avg_per_min.toFixed(2)} calls/minute.`
      const hour = data.analytics.busiest_hour
      return `Busiest local hour is ${String(hour).padStart(2, '0')}:00.`
    }
    if (focus === 'hour') {
      const hour = Number.parseInt(focusHour ?? '', 10)
      const count = data.analytics.hourly.find((entry) => entry.hour === hour)?.calls ?? 0
      return Number.isInteger(hour) && hour >= 0 && hour < 24
        ? `${String(hour).padStart(2, '0')}:00 local hour contains ${formatCompactNumber(count)} calls in this window.`
        : 'Hourly activity for the selected window.'
    }
    if (focus === 'tokens') {
      return 'Token attribution is not currently collected by durable gateway usage telemetry. Call volume, failures, latency, actors, and targets remain exact.'
    }
    return null
  })() : null

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Usage' }]} />

      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
        {/* Hero — the mock's eyebrow + title + action cluster with the stat
            strip welded to the card's bottom edge, not floating cards. */}
        <ConsoleHero
          eyebrow="Observe"
          title="Usage Explorer"
          actions={
            <WindowSelector
              value={window}
              onChange={(nextWindow) => {
                setWindow(nextWindow)
                setSinceMs(undefined)
                setUntilMs(undefined)
                resetPaging()
              }}
            />
          }
          stats={heroStats}
        />

        {focusMessage ? (
          <div className="rounded-aurora-2 border border-aurora-accent-primary/25 bg-aurora-accent-primary/5 px-4 py-3 text-sm text-aurora-text-primary">
            <span className="font-semibold text-aurora-accent-strong">Metric drill-down:</span> {focusMessage}
          </div>
        ) : null}

        <DashboardPanel
          title="Upstream calls"
          icon={<SlidersHorizontal className="size-4" />}
          meta={`${tableMeta} · ${WINDOW_LABELS[window]}`}
        >
          <div className="space-y-3">
            <p className="text-[11px] leading-[1.35] text-aurora-text-muted">
              Every retained upstream call in the selected slice. Filters and chart drill-downs stay in the URL so this view can be shared or reloaded.
            </p>
            {hasTimeSlice ? (
              <div className="flex flex-wrap items-center gap-2 rounded-lg border border-aurora-accent-primary/25 bg-aurora-accent-primary/5 px-3 py-2 text-xs">
                <Clock className="size-3.5 text-aurora-accent-strong" />
                <span className="text-aurora-text-primary">
                  Time slice: {sinceMs ? formatSliceTime(sinceMs) : 'window start'} → {untilMs ? formatSliceTime(untilMs) : 'now'}
                </span>
                <button
                  type="button"
                  onClick={() => { setSinceMs(undefined); setUntilMs(undefined); resetPaging() }}
                  className="ml-auto inline-flex min-h-8 items-center gap-1 rounded-md px-2 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                >
                  <X className="size-3.5" /> Clear slice
                </button>
              </div>
            ) : null}
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-[minmax(0,1fr)_12rem_10rem_auto]">
              <div className="relative min-w-0">
                <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
                <Input value={search} onChange={(event) => { setSearch(event.target.value); resetPaging() }} placeholder="Search target, operation, agent, error…" className="h-10 pl-9" />
              </div>
              <Select value={upstream} onValueChange={(value) => { setUpstream(value); resetPaging() }}>
                <SelectTrigger className="h-10 w-full"><SelectValue placeholder="Server" /></SelectTrigger>
                <SelectContent><SelectItem value={ALL}>All servers</SelectItem>{upstreamOptions.map((name) => <SelectItem key={name} value={name}>{name}</SelectItem>)}</SelectContent>
              </Select>
              <Select value={outcome} onValueChange={(value) => { setOutcome(value); if (value !== 'failed') setErrorKind(ALL); resetPaging() }}>
                <SelectTrigger className="h-10 w-full"><SelectValue placeholder="Outcome" /></SelectTrigger>
                <SelectContent><SelectItem value={ALL}>All outcomes</SelectItem><SelectItem value="ok">Succeeded</SelectItem><SelectItem value="failed">Failed</SelectItem></SelectContent>
              </Select>
              <details className="group relative">
                <summary className="flex h-10 cursor-pointer list-none items-center justify-center gap-2 rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm font-medium text-aurora-text-muted hover:text-aurora-text-primary [&::-webkit-details-marker]:hidden">
                  <SlidersHorizontal className="size-4" /> More filters
                </summary>
                <div className="absolute right-0 z-30 mt-2 grid w-[min(44rem,85vw)] grid-cols-2 gap-2 rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-strong p-3 shadow-aurora-panel md:grid-cols-3">
                  <Select value={tool} onValueChange={(value) => { setTool(value); resetPaging() }}><SelectTrigger className="h-10 w-full"><SelectValue placeholder="Target" /></SelectTrigger><SelectContent><SelectItem value={ALL}>All targets</SelectItem>{toolOptions.map((name) => <SelectItem key={name} value={name}>{name}</SelectItem>)}</SelectContent></Select>
                  <Select value={capability} onValueChange={(value) => { setCapability(value); resetPaging() }}><SelectTrigger className="h-10 w-full"><SelectValue placeholder="Capability" /></SelectTrigger><SelectContent><SelectItem value={ALL}>All capabilities</SelectItem>{capabilityOptions.map((name) => <SelectItem key={name} value={name}>{name}</SelectItem>)}</SelectContent></Select>
                  <Select value={operation} onValueChange={(value) => { setOperation(value); resetPaging() }}><SelectTrigger className="h-10 w-full"><SelectValue placeholder="Operation" /></SelectTrigger><SelectContent><SelectItem value={ALL}>All operations</SelectItem>{operationOptions.map((name) => <SelectItem key={name} value={name}>{name}</SelectItem>)}</SelectContent></Select>
                  <Select value={subjectScope} onValueChange={(value) => { setSubjectScope(value); resetPaging() }}><SelectTrigger className="h-10 w-full"><SelectValue placeholder="Scope" /></SelectTrigger><SelectContent><SelectItem value={ALL}>All scopes</SelectItem><SelectItem value="shared">Shared</SelectItem><SelectItem value="subject">OAuth subject</SelectItem></SelectContent></Select>
                  <Select value={agent} onValueChange={(value) => { setAgent(value); resetPaging() }}><SelectTrigger className="h-10 w-full"><SelectValue placeholder="Agent" /></SelectTrigger><SelectContent><SelectItem value={ALL}>All agents</SelectItem>{agentOptions.map(([id, label]) => <SelectItem key={id} value={id}>{label}</SelectItem>)}</SelectContent></Select>
                  <Select value={errorKind} onValueChange={(value) => { setErrorKind(value); if (value !== ALL) setOutcome('failed'); resetPaging() }}><SelectTrigger className="h-10 w-full"><SelectValue placeholder="Failure kind" /></SelectTrigger><SelectContent><SelectItem value={ALL}>All failure kinds</SelectItem>{errorOptions.map((kind) => <SelectItem key={kind} value={kind}>{kind}</SelectItem>)}</SelectContent></Select>
                  {showIps ? <Select value={ip} onValueChange={(value) => { setIp(value); resetPaging() }}><SelectTrigger className="h-10 w-full"><SelectValue placeholder="IP" /></SelectTrigger><SelectContent><SelectItem value={ALL}>All IPs</SelectItem>{ipOptions.map((addr) => <SelectItem key={addr} value={addr}>{addr}</SelectItem>)}</SelectContent></Select> : null}
                </div>
              </details>
            </div>
          </div>
          <div className="my-3 border-t border-aurora-border-subtle" />
          <div className="md:hidden">
            <UsageCallCards calls={data?.calls} isLoading={isLoading} error={error} onRetry={() => { void mutate() }} />
          </div>
          {/* Dense desktop table. Phones get purpose-built cards above instead of horizontal scrolling. */}
          <div className="hidden overflow-x-auto md:block" style={{ margin: '-12px -14px' }}>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[120px]">Time</TableHead>
                  <TableHead>Target · operation</TableHead>
                  <TableHead>Agent</TableHead>
                  {showSurfaces ? <TableHead className="w-[80px]">Surface</TableHead> : null}
                  <TableHead className="w-[110px]">Outcome</TableHead>
                  {showTokens ? <TableHead className="w-[90px] text-right">Tokens</TableHead> : null}
                  <TableHead className="w-[90px] text-right">Response</TableHead>
                  <TableHead className="w-[90px] text-right">Latency</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {error && !data ? (
                  <TableRow>
                      <TableCell colSpan={tableColumns} className="py-8 text-center">
                      <span className="text-sm text-aurora-error">Couldn&apos;t load calls. </span>
                      <span className="text-sm text-aurora-text-muted">{getErrorMessage(error, 'Usage request failed')} </span>
                      <button
                        type="button"
                        onClick={() => mutate()}
                        className="text-sm font-medium text-aurora-accent-primary underline-offset-4 hover:underline"
                      >
                        Retry
                      </button>
                    </TableCell>
                  </TableRow>
                ) : isLoading && !data ? (
                  Array.from({ length: 8 }, (_, i) => (
                    <TableRow key={i}>
                      <TableCell colSpan={tableColumns}><Skeleton className="h-5 w-full" /></TableCell>
                    </TableRow>
                  ))
                ) : !data || data.calls.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={tableColumns} className="py-10 text-center text-sm text-aurora-text-muted">
                      No calls match these filters.
                    </TableCell>
                  </TableRow>
                ) : (
                  data.calls.map((call) => (
                    <TableRow key={call.id}>
                      <TableCell className="text-aurora-text-muted">{formatRelativeTime(call.ts)}</TableCell>
                      <TableCell>
                        <span className="font-mono text-[13px] text-aurora-text-primary">{call.tool}</span>
                        {call.action ? (
                          <span className="font-mono text-[12px] text-aurora-text-muted">.{call.action}</span>
                        ) : null}
                        {call.capability && call.capability !== 'tools' ? (
                          <span className="ml-2 text-[11px] text-aurora-text-muted">{call.capability}</span>
                        ) : null}
                      </TableCell>
                      <TableCell>
                        <div className="text-aurora-text-primary">
                          {call.agent_label === 'unattributed' ? 'Not attributed' : call.agent_label}
                        </div>
                        {showIps ? (
                          <div className="font-mono text-[11px] text-aurora-text-muted">{call.ip}</div>
                        ) : null}
                      </TableCell>
                      {showSurfaces ? <TableCell><SurfaceTag surface={call.surface} /></TableCell> : null}
                      <TableCell>
                        <span className="inline-flex items-center gap-2">
                          <OutcomeDot outcome={call.outcome} />
                          <span className={call.outcome === 'failed' ? 'text-aurora-error' : 'text-aurora-text-muted'}>
                            {call.outcome === 'failed' ? (call.error_kind ?? 'failed') : 'ok'}
                          </span>
                        </span>
                      </TableCell>
                      {showTokens ? (
                        <TableCell className="text-right tabular-nums text-aurora-text-muted">
                          {formatCompactNumber(call.input_tokens + call.output_tokens)}
                        </TableCell>
                      ) : null}
                      <TableCell className="text-right font-mono text-[11px] tabular-nums text-aurora-text-muted">
                        {formatBytes(call.response_bytes)}
                      </TableCell>
                      <TableCell className="text-right tabular-nums text-aurora-text-muted">
                        {formatDuration(call.elapsed_ms)}
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </div>
        </DashboardPanel>

        {/* Cursor pagination keeps deep pages O(page size), even over large retained windows. */}
        {pageIndex > 0 || data?.next_cursor ? (
          <div className="flex items-center justify-between gap-2 sm:justify-end">
            <Button
              variant="outline"
              size="sm"
              className="min-h-10 flex-1 sm:flex-none"
              disabled={pageIndex === 0}
              onClick={() => setCursorStack((stack) => stack.length > 1 ? stack.slice(0, -1) : stack)}
            >
              <ChevronLeft className="size-4" /> Prev
            </Button>
            <span className="hidden text-xs text-aurora-text-muted sm:inline">Page {pageIndex + 1}</span>
            <Button
              variant="outline"
              size="sm"
              className="min-h-10 flex-1 sm:flex-none"
              disabled={!data?.next_cursor}
              onClick={() => data?.next_cursor && setCursorStack((stack) => [...stack, data.next_cursor ?? null])}
            >
              Next <ChevronRight className="size-4" />
            </Button>
          </div>
        ) : null}

        <div>
          <Button variant="ghost" size="sm" asChild>
            <Link href="/">← Back to overview</Link>
          </Button>
        </div>
      </div>
    </>
  )
}

export default function UsageExplorerPage() {
  return (
    <Suspense fallback={null}>
      <UsageExplorer />
    </Suspense>
  )
}
