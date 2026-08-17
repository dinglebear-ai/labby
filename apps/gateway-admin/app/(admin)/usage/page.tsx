'use client'

import { Suspense, useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import Link from 'next/link'
import {
  Activity,
  ChevronLeft,
  ChevronRight,
  Clock,
  Network,
  Search,
  SlidersHorizontal,
  Users,
  Wrench,
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
import { useToolCalls } from '@/lib/hooks/use-usage-drilldown'
import {
  WINDOW_LABELS,
  formatCompactNumber,
  formatDuration,
  formatRelativeTime,
} from '@/lib/dashboard/dashboard-metrics'
import { METRICS_WINDOWS, type CallOutcome, type MetricsWindow } from '@/lib/types/metrics'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'

const PAGE_SIZE = 50
const ALL = 'all'
const SEARCH_DEBOUNCE_MS = 300

function isWindow(value: string | null): value is MetricsWindow {
  return value !== null && (METRICS_WINDOWS as readonly string[]).includes(value)
}

function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value)

  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), delayMs)
    return () => window.clearTimeout(timer)
  }, [delayMs, value])

  return debounced
}

function UsageExplorer() {
  const params = useSearchParams()
  const initialWindow = isWindow(params.get('window')) ? (params.get('window') as MetricsWindow) : '24h'

  const [window, setWindow] = useState<MetricsWindow>(initialWindow)
  const [tool, setTool] = useState<string>(params.get('tool') ?? ALL)
  const [agent, setAgent] = useState<string>(params.get('agent') ?? ALL)
  const [ip, setIp] = useState<string>(params.get('ip') ?? ALL)
  const [outcome, setOutcome] = useState<string>(ALL)
  const [search, setSearch] = useState('')
  const [offset, setOffset] = useState(0)
  const debouncedSearch = useDebouncedValue(search.trim(), SEARCH_DEBOUNCE_MS)

  const { data, isLoading, error, mutate } = useToolCalls({
    window,
    tool: tool === ALL ? undefined : tool,
    agent: agent === ALL ? undefined : agent,
    ip: ip === ALL ? undefined : ip,
    outcome: outcome === ALL ? undefined : (outcome as CallOutcome),
    search: debouncedSearch || undefined,
    limit: PAGE_SIZE,
    offset,
  })
  const toolOptions = data?.facets.tools ?? []
  const agentOptions = useMemo(
    () => (data?.facets.agents ?? []).map((entry) => [entry.id, entry.label] as const),
    [data],
  )
  const ipOptions = data?.facets.ips ?? []
  const collected = data?.collected
  const showIps = collected?.ips ?? false
  const showSurfaces = collected?.surfaces ?? false
  const showTokens = collected?.tokens ?? false
  const tableColumns = 4 + Number(showSurfaces) + Number(showTokens)

  const resetPaging = () => setOffset(0)
  const filtered = data?.filtered ?? 0
  const showingFrom = filtered === 0 ? 0 : offset + 1
  const showingTo = Math.min(offset + PAGE_SIZE, filtered)

  // Only counts the explorer response actually carries. Outcome/latency
  // aggregates are page-local here, so they are deliberately not promoted into
  // the strip — a "failed" number computed from 50 visible rows would read as
  // a window total and be wrong.
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
      label: 'Tools',
      value: data ? data.facets.tools.length : '—',
      icon: <Wrench size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Agents',
      value: data ? data.facets.agents.length : '—',
      icon: <Users size={12} strokeWidth={1.8} />,
    },
    {
      label: 'Source IPs',
      value: data ? (showIps ? data.facets.ips.length : 'Not collected') : '—',
      icon: <Network size={12} strokeWidth={1.8} />,
    },
  ]

  const tableMeta = isLoading && !data
    ? 'Loading…'
    : `${formatCompactNumber(filtered)} matching call${filtered === 1 ? '' : 's'}${
        data ? ` of ${formatCompactNumber(data.total)}` : ''
      }${filtered > 0 ? ` · ${showingFrom}–${showingTo}` : ''}`

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Overview', href: '/' }, { label: 'Usage explorer' }]} />

      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
        {/* Hero — the mock's eyebrow + title + action cluster with the stat
            strip welded to the card's bottom edge, not floating cards. */}
        <ConsoleHero
          eyebrow="Observe"
          title="Usage Explorer"
          actions={
            <WindowSelector value={window} onChange={(w) => { setWindow(w); resetPaging() }} />
          }
          stats={heroStats}
        />

        <DashboardPanel
          title="Filters"
          icon={<SlidersHorizontal className="size-4" />}
          meta={WINDOW_LABELS[window]}
        >
          <p className="text-[11px] leading-[1.35] text-aurora-text-muted">
            Every dispatched tool call in the window — filter by tool, agent, outcome, or text.
          </p>
          <div className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center">
            <div className="relative flex-1 sm:min-w-[220px]">
              <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" />
              <Input
                value={search}
                onChange={(e) => { setSearch(e.target.value); resetPaging() }}
                placeholder="Search tool, action, agent, error…"
                className="pl-9"
              />
            </div>
            <Select value={tool} onValueChange={(v) => { setTool(v); resetPaging() }}>
              <SelectTrigger className="sm:w-40"><SelectValue placeholder="Tool" /></SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>All tools</SelectItem>
                {toolOptions.map((name) => (
                  <SelectItem key={name} value={name}>{name}</SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={agent} onValueChange={(v) => { setAgent(v); resetPaging() }}>
              <SelectTrigger className="sm:w-44"><SelectValue placeholder="Agent" /></SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>All agents</SelectItem>
                {agentOptions.map(([id, label]) => (
                  <SelectItem key={id} value={id}>{label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
            {showIps ? (
              <Select value={ip} onValueChange={(v) => { setIp(v); resetPaging() }}>
                <SelectTrigger className="sm:w-40"><SelectValue placeholder="IP" /></SelectTrigger>
                <SelectContent>
                  <SelectItem value={ALL}>All IPs</SelectItem>
                  {ipOptions.map((addr) => (
                    <SelectItem key={addr} value={addr}>{addr}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : null}
            <Select value={outcome} onValueChange={(v) => { setOutcome(v); resetPaging() }}>
              <SelectTrigger className="sm:w-36"><SelectValue placeholder="Outcome" /></SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>All outcomes</SelectItem>
                <SelectItem value="ok">Succeeded</SelectItem>
                <SelectItem value="failed">Failed</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </DashboardPanel>

        <DashboardPanel title="Tool calls" icon={<Activity className="size-4" />} meta={tableMeta}>
          {/* The panel body carries the mock's 12/14 padding; a dense table
              wants the card's full width, so it is pulled back flush. */}
          <div style={{ margin: '-12px -14px' }}>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[120px]">Time</TableHead>
                  <TableHead>Tool · action</TableHead>
                  <TableHead>Agent</TableHead>
                  {showSurfaces ? <TableHead className="w-[80px]">Surface</TableHead> : null}
                  <TableHead className="w-[110px]">Outcome</TableHead>
                  {showTokens ? <TableHead className="w-[90px] text-right">Tokens</TableHead> : null}
                  <TableHead className="w-[90px] text-right">Latency</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {error && !data ? (
                  <TableRow>
                      <TableCell colSpan={tableColumns} className="py-8 text-center">
                      <span className="text-sm text-aurora-error">Couldn&apos;t load calls. </span>
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

        {/* Pagination */}
        {filtered > PAGE_SIZE ? (
          <div className="flex items-center justify-end gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={offset === 0}
              onClick={() => setOffset((o) => Math.max(0, o - PAGE_SIZE))}
            >
              <ChevronLeft className="size-4" /> Prev
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={showingTo >= filtered}
              onClick={() => setOffset((o) => o + PAGE_SIZE)}
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
