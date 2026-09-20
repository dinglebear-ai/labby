'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Check, Copy, Download, Loader2, Pause, Play, Search, Server, TriangleAlert, X } from 'lucide-react'
import { ConsoleHero } from '@/components/console/console-hero'
import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { queryServerLogs } from '@/lib/api/server-logs-client'
import { getBrowserSessionContextIdentity, subscribeToBrowserSession } from '@/lib/auth/session-store'
import type { ServerLogEntry } from '@/lib/types/traces'
import { cn, getErrorMessage } from '@/lib/utils'

const LEVELS = ['ALL', 'ERROR', 'WARN', 'INFO', 'DEBUG'] as const
const LOG_LEVELS = LEVELS.slice(1)
type LogLevel = (typeof LOG_LEVELS)[number]

function logSource(entry: ServerLogEntry) {
  return entry.service ?? entry.target ?? 'unknown'
}

export function mergeKnownLogSources(current: string[], entries: ServerLogEntry[]) {
  return [...new Set([
    ...current,
    ...entries.flatMap((entry) => entry.service ? [entry.service] : []),
  ])].sort()
}

function logLineText(entry: ServerLogEntry) {
  return [entry.timestamp, entry.level, logSource(entry), entry.message ?? entry.action]
    .filter(Boolean)
    .join(' ')
}

function fieldValue(value: unknown) {
  if (typeof value === 'string') return value
  if (value === null || value === undefined) return '—'
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  try {
    return JSON.stringify(value)
  } catch {
    return '[unavailable]'
  }
}

function formatBytes(bytes: number) {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(bytes % (1024 * 1024) === 0 ? 0 : 1)} MiB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(bytes % 1024 === 0 ? 0 : 1)} KiB`
  return `${bytes} B`
}

function logDetailFields(entry: ServerLogEntry) {
  const fields = new Map<string, unknown>([
    ['timestamp', entry.timestamp],
    ['source', logSource(entry)],
    ['target', entry.target],
    ['action', entry.action],
    ['kind', entry.kind],
    ['file', entry.file],
  ])
  for (const [key, value] of Object.entries(entry.fields)) fields.set(key, value)
  return [...fields].filter(([, value]) => value !== null && value !== undefined)
}

export function LogsPageContent({ embedded = false, upstream }: { embedded?: boolean; upstream?: string }) {
  const [entries, setEntries] = useState<ServerLogEntry[]>([])
  const [query, setQuery] = useState('')
  const [levels, setLevels] = useState<LogLevel[]>([])
  const [source, setSource] = useState('ALL')
  const [following, setFollowing] = useState(true)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [expandedLine, setExpandedLine] = useState<string | null>(null)
  const [copiedLine, setCopiedLine] = useState<string | null>(null)
  const [knownSources, setKnownSources] = useState<string[]>([])
  const [sourceFacetComplete, setSourceFacetComplete] = useState<boolean | null>(null)
  const [sessionKey, setSessionKey] = useState(() => getBrowserSessionContextIdentity())
  const [meta, setMeta] = useState({ matched: 0, scanned: 0, malformed: 0, scannedBytes: 0, maxScanBytes: 0, truncated: false })
  const requestRef = useRef<AbortController | null>(null)
  const streamRef = useRef<HTMLDivElement>(null)

  useEffect(() => subscribeToBrowserSession(() => {
    setSessionKey(getBrowserSessionContextIdentity())
  }), [])

  useEffect(() => {
    requestRef.current?.abort()
    setEntries([])
    setKnownSources([])
    setSourceFacetComplete(null)
    setSource('ALL')
  }, [sessionKey])

  const load = useCallback(async () => {
    requestRef.current?.abort()
    const controller = new AbortController()
    const requestSessionKey = sessionKey
    requestRef.current = controller
    setError(null)
    try {
      const result = await queryServerLogs({
        limit: 250,
        levels: levels.length ? levels : undefined,
        service: source === 'ALL' ? undefined : source,
        query: upstream || query.trim() || undefined,
        stop_after_limit: embedded,
      }, { signal: controller.signal })
      if (controller.signal.aborted || requestSessionKey !== getBrowserSessionContextIdentity()) return
      const scopedEntries = upstream ? result.entries.filter((entry) => entry.fields.upstream === upstream) : result.entries
      const needle = query.trim().toLowerCase()
      setEntries(upstream && needle ? scopedEntries.filter((entry) => JSON.stringify(entry).toLowerCase().includes(needle)) : scopedEntries)
      if (Array.isArray(result.available_sources)) {
        setKnownSources((current) => result.available_sources_complete
          ? [...result.available_sources!].sort()
          : [...new Set([...current, ...result.available_sources!])].sort())
        setSourceFacetComplete(result.available_sources_complete === true)
      } else {
        setKnownSources((current) => mergeKnownLogSources(current, result.entries))
        setSourceFacetComplete(null)
      }
      setMeta({
        matched: result.matched,
        scanned: result.scanned_lines,
        malformed: result.malformed_lines,
        scannedBytes: result.scanned_bytes,
        maxScanBytes: result.max_scan_bytes,
        truncated: result.truncated,
      })
    } catch (cause) {
      if (!controller.signal.aborted && requestSessionKey === getBrowserSessionContextIdentity()) {
        setError(getErrorMessage(cause, 'Logs unavailable'))
      }
    } finally {
      if (!controller.signal.aborted && requestSessionKey === getBrowserSessionContextIdentity()) {
        setLoading(false)
      }
    }
  }, [embedded, levels, query, sessionKey, source, upstream])

  useEffect(() => { setLoading(true); setEntries([]); void load(); return () => requestRef.current?.abort() }, [load])
  useEffect(() => {
    if (!following) return
    const timer = window.setInterval(() => void load(), 5_000)
    return () => window.clearInterval(timer)
  }, [following, load])

  const sourceOptions = knownSources
  const observedSources = useMemo(() => new Set(entries.map(logSource)).size, [entries])
  const failures = entries.filter((entry) => entry.level === 'ERROR').length
  const levelCounts = useMemo(
    () => Object.fromEntries(LOG_LEVELS.map((item) => [item, entries.filter((entry) => entry.level === item).length])),
    [entries],
  )
  const streamEntries = useMemo(() => [...entries].reverse(), [entries])
  const sourceLabel = source === 'ALL' ? 'all sources' : source
  const pollingLabel = following ? `Polling · 5s · ${sourceLabel}` : 'Paused'
  const hasFilters = Boolean(query.trim() || levels.length || source !== 'ALL')
  const scanBudget = meta.maxScanBytes ? formatBytes(meta.maxScanBytes) : null
  const sourceFacetLabel = sourceFacetComplete === true
    ? `Source list complete for this retained-file query${scanBudget ? ` within the ${scanBudget} scan budget` : ''}`
    : sourceFacetComplete === false
      ? `Source list observed across ${formatBytes(meta.scannedBytes)} of a bounded${scanBudget ? ` ${scanBudget}` : ''} scan`
      : 'Source list observed in returned rows'

  useEffect(() => {
    if (!following || !streamRef.current) return
    streamRef.current.scrollTop = streamRef.current.scrollHeight
  }, [following, streamEntries])

  const download = () => {
    const blob = new Blob([entries.map((entry) => JSON.stringify(entry)).join('\n')], { type: 'application/x-ndjson' })
    const href = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = href
    anchor.download = `labby-logs-${new Date().toISOString().replaceAll(':', '-')}.jsonl`
    anchor.click()
    URL.revokeObjectURL(href)
  }

  return <>
    {!embedded ? <AppHeader breadcrumbs={[{ label: 'Logs' }]} /> : null}
    <div className={embedded ? 'flex min-h-0 flex-col gap-3.5' : cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL)}>
      {!embedded ? <ConsoleHero eyebrow="Observability" title="Logs" pulse={{ color: following ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: pollingLabel }} /> : null}
      <div className={cn('flex min-h-0 flex-col', embedded ? 'h-[480px]' : 'h-[calc(100vh-14.25rem)]')}>
      <section aria-label="Log stream" className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-strong">
        <form className="flex flex-wrap items-center gap-2 border-b border-aurora-border-default bg-[var(--gw0-0_38)] px-3.5 py-[9px]" onSubmit={(event) => { event.preventDefault(); void load() }}>
          {!embedded ? <>
            <span data-heroecho="1" className="inline-flex shrink-0 items-center gap-2 pr-0.5">
              <span className="font-display text-[15px] font-extrabold text-aurora-text-primary">Logs</span>
              <span className={cn('inline-flex items-center gap-1.5 text-[10.5px] font-semibold', following ? 'text-aurora-success' : 'text-aurora-warn')}>
                <span className={cn('size-1.5 rounded-full', following ? 'bg-aurora-success shadow-[0_0_4px_var(--aurora-success)]' : 'bg-aurora-warn shadow-[0_0_4px_var(--aurora-warn)]')} />
                {pollingLabel}
              </span>
            </span>
            <span data-heroecho="1" className="h-[18px] w-px shrink-0 bg-aurora-border-default" aria-hidden="true" />
          </> : null}
          <div className="relative min-w-[150px] flex-1 sm:max-w-80"><Search aria-hidden="true" className="pointer-events-none absolute left-2.5 top-1/2 size-3 -translate-y-1/2 text-aurora-text-muted"/><input value={query} onChange={(event) => setQuery(event.target.value)} aria-label="Filter log lines" className="h-[30px] w-full rounded-[9px] border border-aurora-border-default bg-aurora-control-surface pl-[30px] pr-8 text-xs text-aurora-text-primary outline-none focus:border-aurora-accent-primary focus:ring-2 focus:ring-aurora-accent-primary/20" placeholder="Filter lines…"/>{query ? <button type="button" aria-label="Clear filter" title="Clear filter" onClick={() => setQuery('')} className="absolute right-1 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded text-aurora-text-muted hover:bg-aurora-hover-bg focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><X aria-hidden="true" className="size-3" /></button> : null}</div>
          <span className="hidden h-6 w-px bg-aurora-border-default sm:block" aria-hidden="true" />
          <span title={sourceFacetLabel} className="font-mono text-[9px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Source</span>
          <Select value={source} onValueChange={setSource}>
            <SelectTrigger aria-label="Log source" data-visible-label="1" className="data-[size=default]:h-[30px] max-w-[220px] gap-[7px] rounded-[9px] border-aurora-border-default bg-aurora-control-surface px-[11px] py-0 text-xs font-[650] text-aurora-text-primary hover:bg-aurora-hover-bg [&>svg]:size-[11px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="start" sideOffset={1} className="max-h-80 w-[220px] rounded-xl border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] text-aurora-text-primary shadow-aurora-strong">
              {['ALL', ...new Set([...sourceOptions, ...(source === 'ALL' ? [] : [source])])].map((item) => <SelectItem key={item} value={item} textValue={item === 'ALL' ? 'Gateway (all)' : item} className="rounded-lg text-xs data-[state=checked]:bg-aurora-selected-bg data-[state=checked]:text-aurora-accent-strong">
                <Server aria-hidden="true" className="size-[15px] text-aurora-accent-strong" />
                <span className="truncate">{item === 'ALL' ? 'Gateway (all)' : item}</span>
              </SelectItem>)}
            </SelectContent>
          </Select>
          <div className="flex rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg">{LOG_LEVELS.map((item) => {
            const selected = levels.includes(item)
            return <button key={item} type="button" aria-pressed={selected} className={cn('h-7 px-2 font-mono text-[9px] font-bold transition-colors first:rounded-l-md last:rounded-r-md', selected ? 'bg-aurora-selected-bg text-aurora-accent-strong' : 'text-aurora-text-muted hover:text-aurora-text-primary')} onClick={() => setLevels((current) => current.includes(item) ? current.filter((level) => level !== item) : [...current, item])}><span className={cn('mr-1', item === 'ERROR' ? 'text-aurora-error' : item === 'WARN' ? 'text-aurora-warn' : item === 'INFO' ? 'text-aurora-accent-primary' : '')}>•</span>{item} {levelCounts[item]}</button>
          })}</div>
          <div className="ml-auto flex items-center gap-1.5">
            <Button data-visible-label variant="outline" size="sm" aria-pressed={following} className="h-[30px] gap-[7px] rounded-[9px] px-[11px] text-xs" title={following ? 'Pause live tail' : 'Follow live tail'} onClick={() => setFollowing((value) => !value)}>{following ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}{following ? 'Pause' : 'Follow'}</Button>
            <Button data-visible-label variant="outline" size="sm" className="h-[30px] gap-[7px] rounded-[9px] px-[11px] text-xs" title="Download current view as JSONL" onClick={download} disabled={entries.length === 0}><Download className="size-3.5" />Download</Button>
            <span className="rounded-aurora-1 border border-aurora-border-default px-2.5 py-1.5 font-mono text-[10px] text-aurora-text-secondary"><span className="mr-1 text-aurora-warn">•</span>{entries.length} of {meta.matched} lines</span>
          </div>
        </form>
        {error ? <div role="alert" className="flex items-center gap-2 border-b border-aurora-error/30 bg-aurora-error/10 px-3 py-2 font-mono text-xs text-aurora-error"><TriangleAlert className="size-4"/>{error}</div> : null}
        <div ref={streamRef} role="log" aria-live={following ? 'polite' : 'off'} className="aurora-scrollbar min-h-0 flex-1 overflow-auto font-mono text-[11px] leading-5 sm:text-xs">
          <div className="sticky top-0 z-10 grid min-w-[760px] grid-cols-[88px_52px_110px_minmax(300px,1fr)_28px] h-7 items-center gap-2.5 border-b border-aurora-border-strong bg-[var(--gw0-0_48)] px-4 font-mono text-[9px] font-bold uppercase tracking-[.16em] text-aurora-text-muted"><span>Time</span><span>Level</span><span>Source</span><span>Message</span><span/></div>
          {streamEntries.map((entry, index) => {
            const lineKey = `${entry.timestamp}-${index}`
            const expanded = expandedLine === lineKey
            const levelTone = entry.level === 'ERROR' ? 'text-aurora-error' : entry.level === 'WARN' ? 'text-aurora-warn' : entry.level === 'DEBUG' ? 'text-aurora-text-muted' : 'text-aurora-success'
            const rowTone = entry.level === 'ERROR'
              ? 'bg-aurora-error/5 hover:bg-aurora-error/10'
              : entry.level === 'WARN'
                ? 'bg-aurora-warn/5 hover:bg-aurora-warn/10'
                : index % 2 === 0
                  ? 'bg-[var(--gw1-0_30)] hover:bg-aurora-hover-bg/50'
                  : 'hover:bg-aurora-hover-bg/50'
            const detailTone = entry.level === 'ERROR'
              ? 'border-aurora-error/25 shadow-[inset_2px_0_0_color-mix(in_srgb,var(--aurora-error)_55%,transparent)]'
              : entry.level === 'WARN'
                ? 'border-aurora-warn/25 shadow-[inset_2px_0_0_color-mix(in_srgb,var(--aurora-warn)_55%,transparent)]'
                : 'border-aurora-border-default shadow-[inset_2px_0_0_color-mix(in_srgb,var(--aurora-accent-primary)_45%,transparent)]'
            return <div key={lineKey} data-zebra-row="1" className={cn('group', rowTone)}>
              <div
                data-log-line="1"
                role="button"
                tabIndex={0}
                aria-expanded={expanded}
                onClick={() => setExpandedLine(expanded ? null : lineKey)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault()
                    setExpandedLine(expanded ? null : lineKey)
                  }
                }}
                className="grid w-full min-w-[760px] grid-cols-[88px_52px_110px_minmax(300px,1fr)_28px] items-baseline gap-2.5 px-4 py-[2.5px] text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary"
              >
                <span className="text-aurora-text-muted">{entry.timestamp ? new Date(entry.timestamp).toLocaleTimeString([], { hour12: false }) : '--:--:--'}</span>
                <span className={cn('font-bold', levelTone)}>{(entry.level ?? '—').padEnd(5)}</span>
                <span className="truncate text-aurora-accent-strong" title={logSource(entry)}>{logSource(entry)}</span>
                <span className={cn('truncate', entry.level === 'ERROR' ? 'text-aurora-error' : entry.level === 'WARN' ? 'text-aurora-warn' : 'text-aurora-text-primary/85')}>{entry.message ?? entry.action ?? '—'}</span>
                <button
                  type="button"
                  aria-label={copiedLine === lineKey ? 'Log line copied' : 'Copy log line'}
                  title={copiedLine === lineKey ? 'Copied' : 'Copy line'}
                  className="grid size-[18px] place-items-center rounded-[5px] text-aurora-text-muted opacity-0 transition-opacity hover:bg-aurora-hover-bg hover:text-aurora-accent-strong focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary group-hover:opacity-100"
                  onClick={(event) => {
                    event.stopPropagation()
                    void navigator.clipboard?.writeText(logLineText(entry)).then(
                      () => setCopiedLine(lineKey),
                      () => setCopiedLine(null),
                    )
                  }}
                >
                  {copiedLine === lineKey ? <Check className="size-2.5" /> : <Copy className="size-2.5" />}
                </button>
              </div>
              {expanded ? (
                <div data-log-detail="1" className={cn('mx-4 mb-1.5 ml-[98px] grid grid-cols-[repeat(auto-fit,minmax(180px,1fr))] gap-x-4 gap-y-1 rounded-[9px] border bg-[var(--gw0-0_55)] px-3 py-2', detailTone)}>
                  {logDetailFields(entry).map(([key, value]) => (
                    <div key={key} className="flex min-w-0 items-baseline gap-2">
                      <span className="shrink-0 text-[10px] text-aurora-text-secondary">{key}</span>
                      <span className="min-w-0 truncate text-[10.5px] text-aurora-text-primary" title={fieldValue(value)}>{fieldValue(value)}</span>
                    </div>
                  ))}
                </div>
              ) : null}
            </div>
          })}
          {loading && entries.length === 0 ? <div className="grid min-h-56 place-items-center text-aurora-text-muted"><Loader2 className="size-5 animate-spin"/></div> : null}
          {!loading && !error && entries.length === 0 ? <div className="grid min-h-56 place-items-center text-center text-sm text-aurora-text-muted"><div><p>No lines match the current source, level, or filter.</p>{hasFilters ? <Button variant="outline" size="sm" className="mt-2 h-[26px] text-[11px]" onClick={() => { setQuery(''); setLevels([]); setSource('ALL') }}>Clear Filters</Button> : null}</div></div> : null}
        </div>
        <footer className="flex flex-wrap items-center gap-2 border-t border-aurora-border-default bg-[var(--gw0-0_38)] px-4 py-2 font-mono text-[10px] text-aurora-text-muted">
          <span className={cn('size-1.5 rounded-full', following ? 'bg-aurora-success shadow-[0_0_4px_var(--aurora-success)]' : 'bg-aurora-warn shadow-[0_0_4px_var(--aurora-warn)]')} />
          <span>{following ? 'Polling every 5s' : 'Paused'} · {upstream ? `${upstream} · bounded sample` : `${sourceLabel} · ${observedSources} observed source${observedSources === 1 ? '' : 's'} · ${sourceFacetLabel.toLowerCase()}`}</span>
          <span className="ml-auto">{entries.length}/{meta.matched} lines · <span className={failures ? 'text-aurora-error' : ''}>{failures} errors</span> · {meta.scanned} scanned{meta.malformed ? ` · ${meta.malformed} malformed` : ''}{meta.truncated ? ' · truncated' : ''}</span>
        </footer>
      </section>
      </div>
    </div>
  </>
}
