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
import { UsageCallDetail } from '@/components/dashboard/usage-call-detail'
import { useToolCalls } from '@/lib/hooks/use-usage-drilldown'
import {
  WINDOW_LABELS,
  formatCompactNumber,
  formatDuration,
  formatRelativeTime,
} from '@/lib/dashboard/dashboard-metrics'
import { METRICS_WINDOWS, type CallOutcome, type MetricsWindow, type ToolCallRecord } from '@/lib/types/metrics'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { cn, getErrorMessage } from '@/lib/utils'
import { usageTraceHref } from '@/lib/observability/usage-trace-link'

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
  const [selectedCall, setSelectedCall] = useState<ToolCallRecord | null>(null)
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
  const activityGridColumns = [
    '96px', 'minmax(0, 1.6fr)', 'minmax(0, 1fr)',
    showSurfaces ? '70px' : null, '110px', showTokens ? '80px' : null, '80px', '80px',
  ].filter(Boolean).join(' ')

  const filtered = data?.filtered ?? 0
  const showingFrom = filtered === 0 ? 0 : pageIndex * PAGE_SIZE + 1
  const showingTo = Math.min(pageIndex * PAGE_SIZE + (data?.calls.length ?? 0), filtered)
  const hasTimeSlice = sinceMs !== undefined || untilMs !== undefined

  const heroStats = [
    {
      label: 'Matched',
      tone: 'var(--aurora-accent-strong)',
      value: data ? formatCompactNumber(data.filtered) : '—',
      icon: <Activity size={11} strokeWidth={1.8} />,
    },
    {
      label: 'In window',
      value: data ? formatCompactNumber(data.total) : '—',
      icon: <Clock size={11} strokeWidth={1.8} />,
    },
    {
      label: 'Failed',
      tone: 'var(--aurora-error)',
      value: data ? formatCompactNumber(data.analytics.failed) : '—',
      icon: <AlertTriangle size={11} strokeWidth={1.8} />,
    },
    {
      label: 'P95 latency',
      tone: 'var(--aurora-warn)',
      value: data ? formatDuration(data.analytics.p95_elapsed_ms) : '—',
      icon: <Gauge size={11} strokeWidth={1.8} />,
    },
    {
      label: 'Peak / min',
      tone: 'var(--aurora-success)',
      value: data ? formatCompactNumber(data.analytics.peak_per_min) : '—',
      icon: <Zap size={11} strokeWidth={1.8} />,
    },
    {
      label: 'Targets',
      value: data ? data.facets.tools.length : '—',
      icon: <Wrench size={11} strokeWidth={1.8} />,
    },
    {
      label: 'Agents',
      tone: 'var(--aurora-accent-pink)',
      value: data ? data.facets.agents.length : '—',
      icon: <Users size={11} strokeWidth={1.8} />,
    },
    ...(showIps ? [{
      label: 'Source IPs',
      value: data ? data.facets.ips.length : '—',
      icon: <Network size={11} strokeWidth={1.8} />,
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
      <AppHeader icon={<Activity className="size-3.5" />} breadcrumbs={[{ label: 'Activity' }]} />

      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)} style={{ gap: 14 }}>
        {/* Hero — eyebrow + title + action cluster with the stat
            strip welded to the card's bottom edge, not floating cards. */}
        <ConsoleHero
          eyebrow="Observe"
          pulse={{ color: 'var(--aurora-success)', label: 'complete-window analytics' }}
          title="Usage Explorer"
          description="Every retained upstream call in the selected slice. Filters and chart drill-downs stay in the URL so this view can be shared or reloaded."
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

        {hasTimeSlice ? (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 9,
              padding: '9px 14px',
              borderRadius: 'var(--radius-2)',
              border: '1px solid color-mix(in srgb, var(--aurora-accent-primary) 25%, transparent)',
              background: 'color-mix(in srgb, var(--aurora-accent-primary) 5%, var(--aurora-panel-strong))',
              fontSize: 12,
            }}
          >
            <Clock size={13} strokeWidth={1.8} className="shrink-0 text-aurora-accent-strong" />
            <span className="text-aurora-text-primary">
              Time slice: {sinceMs ? formatSliceTime(sinceMs) : 'window start'} → {untilMs ? formatSliceTime(untilMs) : 'now'}
            </span>
            <span style={{ flex: 1 }} />
            <button
              type="button"
              onClick={() => { setSinceMs(undefined); setUntilMs(undefined); resetPaging() }}
              className="inline-flex items-center gap-[5px] rounded-[7px] px-[9px] text-[11.5px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
              style={{ height: 26 }}
            >
              <X size={11} strokeWidth={2} /> Clear slice
            </button>
          </div>
        ) : null}

        <DashboardPanel
          title="Upstream calls"
          iconVariant="plain"
          icon={<SlidersHorizontal className="size-4" />}
          meta={`${tableMeta} · ${WINDOW_LABELS[window]}`}
          headerStyle={{ padding: '10px 15px', lineHeight: 'normal' }}
          titleStyle={{ fontSize: 9.5, lineHeight: 'normal', letterSpacing: '0.13em' }}
          metaStyle={{ fontSize: 11, lineHeight: 'normal', color: 'var(--aurora-text-muted)' }}
          bodyStyle={{ padding: 0, gap: 0 }}
        >
          <div className="space-y-3">
            <div data-activity-filters="1" className="grid grid-cols-1 gap-2 p-3 sm:grid-cols-[minmax(0,1fr)_auto_auto_auto]">
              <div className="flex h-[36px] min-w-0 items-center gap-2 rounded-[9px] border border-aurora-border-default bg-[var(--gw0-0_40)] px-[11px]">
                <Search aria-hidden="true" className="size-3.5 shrink-0 text-aurora-text-muted" strokeWidth={1.7} />
                <input
                  name="search"
                  aria-label="Search upstream calls"
                  value={search}
                  onChange={(event) => { setSearch(event.target.value); resetPaging() }}
                  placeholder="Search target, operation, agent, error…"
                  className="min-w-0 flex-1 border-0 bg-transparent p-0 text-[12.5px] text-aurora-text-primary outline-none placeholder:text-aurora-text-muted"
                  style={{ height: 17, lineHeight: 'normal' }}
                />
              </div>
              <Select value={upstream} onValueChange={(value) => { setUpstream(value); resetPaging() }}>
                <SelectTrigger aria-label="Server" style={{ height: 34 }} className="w-full gap-[6px] rounded-[9px] border-aurora-border-default bg-[var(--gw0-0_40)] px-[11px] text-xs font-semibold shadow-none [&_svg]:size-[11px]"><SelectValue placeholder="Server" /></SelectTrigger>
                <SelectContent><SelectItem value={ALL}>All servers</SelectItem>{upstreamOptions.map((name) => <SelectItem key={name} value={name}>{name}</SelectItem>)}</SelectContent>
              </Select>
              <Select value={outcome} onValueChange={(value) => { setOutcome(value); if (value !== 'failed') setErrorKind(ALL); resetPaging() }}>
                <SelectTrigger aria-label="Outcome" style={{ height: 34 }} className="w-full gap-[6px] rounded-[9px] border-aurora-border-default bg-[var(--gw0-0_40)] px-[11px] text-xs font-semibold shadow-none [&_svg]:size-[11px]"><SelectValue placeholder="Outcome" /></SelectTrigger>
                <SelectContent><SelectItem value={ALL}>All outcomes</SelectItem><SelectItem value="ok">Succeeded</SelectItem><SelectItem value="failed">Failed</SelectItem></SelectContent>
              </Select>
              <details className="group relative">
                <summary className="flex h-[34px] min-w-[115.765625px] cursor-pointer list-none items-center justify-center gap-[6px] rounded-[9px] border border-aurora-border-default bg-[var(--gw0-0_40)] px-[11px] text-xs font-semibold leading-normal text-aurora-text-primary hover:border-aurora-border-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary [&::-webkit-details-marker]:hidden">
                  <SlidersHorizontal className="size-[13px]" strokeWidth={1.8} /> More filters
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
          <div className="md:hidden">
            <UsageCallCards calls={data?.calls} isLoading={isLoading} error={error} onRetry={() => { void mutate() }} onSelectCall={setSelectedCall} />
          </div>
          {/* Dense desktop table. Phones get purpose-built cards above instead of horizontal scrolling. */}
          <div className="aurora-scrollbar hidden overflow-x-auto md:block">
            <Table data-density="default" className="block w-full text-xs [&_th]:h-auto [&_th]:p-0 [&_th]:text-[9px] [&_th]:leading-normal [&_td]:h-auto [&_td]:p-0">
              <TableHeader className="block bg-[var(--gw0-0_30)]">
                <TableRow
                  className="w-full border-b-0 bg-[var(--gw0-0_30)]"
                  style={{
                    display: 'grid',
                    gridTemplateColumns: activityGridColumns,
                    gap: 12,
                    height: 28,
                    padding: '8px 15px',
                    borderTop: '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
                  }}
                >
                  <TableHead className="w-[96px]">Time</TableHead>
                  <TableHead>Target · operation</TableHead>
                  <TableHead>Agent</TableHead>
                  {showSurfaces ? <TableHead className="w-[70px]">Surface</TableHead> : null}
                  <TableHead className="w-[110px]">Outcome</TableHead>
                  {showTokens ? <TableHead className="w-[90px] text-right">Tokens</TableHead> : null}
                  <TableHead className="w-[80px] text-right">Response</TableHead>
                  <TableHead className="w-[80px] text-right">Latency</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody className="block">
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
                    <TableRow
                      key={call.id}
                      className="cursor-pointer border-t border-b-0 border-aurora-border-default/35 odd:bg-[var(--gw1-0_62)] even:bg-[var(--gw2-0_55)] hover:!bg-aurora-hover-bg"
                      style={{
                        display: 'grid',
                        gridTemplateColumns: activityGridColumns,
                        gap: 12,
                        alignItems: 'center',
                        width: '100%',
                        height: 59,
                        padding: '7px 15px',
                      }}
                      onClick={(event) => {
                        if (!(event.target as HTMLElement).closest('a, button')) setSelectedCall(call)
                      }}
                    >
                      <TableCell className="text-[11px] tabular-nums text-aurora-text-muted">
                        <button
                          type="button"
                          aria-label={`Inspect call ${[call.tool, call.action].filter(Boolean).join('.')} from ${formatRelativeTime(call.ts)}`}
                          onClick={() => setSelectedCall(call)}
                          className="rounded text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"
                        >
                          {formatRelativeTime(call.ts)}
                        </button>
                      </TableCell>
                      <TableCell className="truncate" title={[call.tool, call.action].filter(Boolean).join('.')}>
                        <Link href={usageTraceHref(call.tool)} aria-label={`View traces for ${call.tool}`} className="rounded font-mono text-[12.5px] text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">{call.tool}</Link>
                        {call.action ? (
                          <span className="font-mono text-[11.5px] text-aurora-text-muted">.{call.action}</span>
                        ) : null}
                        {call.capability && call.capability !== 'tools' ? (
                          <span className="ml-2 text-[11px] text-aurora-text-muted">{call.capability}</span>
                        ) : null}
                      </TableCell>
                      <TableCell>
                        <div className="truncate text-aurora-text-primary" title={call.agent_label === 'unattributed' ? 'Not attributed' : call.agent_label}>
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
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 8 }}>
          <button
            type="button"
            disabled={pageIndex === 0}
            onClick={() => setCursorStack((stack) => stack.length > 1 ? stack.slice(0, -1) : stack)}
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 5,
              height: 32,
              padding: '0 12px',
              borderRadius: 9,
              border: '1px solid color-mix(in srgb, var(--aurora-border-strong) 70%, var(--aurora-page-bg))',
              background: 'var(--aurora-control-surface)',
              color: 'var(--aurora-text-muted)',
              fontFamily: 'inherit',
              fontSize: 12,
              fontWeight: 650,
              cursor: 'pointer',
              opacity: pageIndex === 0 ? 0.4 : 1,
            }}
          >
            <ChevronLeft size={12} strokeWidth={2} /> Prev
          </button>
          <span style={{ fontSize: 11, lineHeight: 'normal', color: 'var(--aurora-text-muted)' }}>Page {pageIndex + 1}</span>
          <button
            type="button"
            disabled={!data?.next_cursor}
            onClick={() => data?.next_cursor && setCursorStack((stack) => [...stack, data.next_cursor ?? null])}
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 5,
              height: 32,
              padding: '0 12px',
              borderRadius: 9,
              border: '1px solid color-mix(in srgb, var(--aurora-border-strong) 70%, var(--aurora-page-bg))',
              background: 'var(--aurora-control-surface)',
              color: 'var(--aurora-text-muted)',
              fontFamily: 'inherit',
              fontSize: 12,
              fontWeight: 650,
              cursor: 'pointer',
              opacity: data?.next_cursor ? 1 : 0.4,
            }}
          >
            Next <ChevronRight size={12} strokeWidth={2} />
          </button>
        </div>

      </div>

      <UsageCallDetail
        call={selectedCall}
        onClose={() => setSelectedCall(null)}
        tokensCollected={showTokens}
        ipsCollected={showIps}
        surfacesCollected={showSurfaces}
      />
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
