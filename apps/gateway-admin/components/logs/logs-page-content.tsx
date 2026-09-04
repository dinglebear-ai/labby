'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, ChevronRight, Download, Loader2, Pause, Play, Search, TriangleAlert } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { queryServerLogs } from '@/lib/api/server-logs-client'
import type { ServerLogEntry } from '@/lib/types/traces'
import { cn, getErrorMessage } from '@/lib/utils'

const LEVELS = ['ALL', 'ERROR', 'WARN', 'INFO', 'DEBUG'] as const

export function LogsPageContent() {
  const [entries, setEntries] = useState<ServerLogEntry[]>([])
  const [query, setQuery] = useState('')
  const [level, setLevel] = useState<(typeof LEVELS)[number]>('ALL')
  const [source, setSource] = useState('ALL')
  const [following, setFollowing] = useState(true)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [expandedLine, setExpandedLine] = useState<string | null>(null)
  const [meta, setMeta] = useState({ matched: 0, scanned: 0, malformed: 0, truncated: false })
  const streamRef = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    setError(null)
    try {
      const result = await queryServerLogs({
        limit: 250,
        level: level === 'ALL' ? undefined : level,
        service: source === 'ALL' ? undefined : source,
        query: query.trim() || undefined,
        stop_after_limit: true,
      })
      setEntries(result.entries)
      setMeta({ matched: result.matched, scanned: result.scanned_lines, malformed: result.malformed_lines, truncated: result.truncated })
    } catch (cause) {
      setError(getErrorMessage(cause, 'Logs unavailable'))
    } finally {
      setLoading(false)
    }
  }, [level, query, source])

  useEffect(() => { setLoading(true); void load() }, [load])
  useEffect(() => {
    if (!following) return
    const timer = window.setInterval(() => void load(), 5_000)
    return () => window.clearInterval(timer)
  }, [following, load])

  const sourceOptions = useMemo(
    () => [...new Set(entries.map((entry) => entry.service ?? entry.target ?? 'unknown'))].sort(),
    [entries],
  )
  const sources = sourceOptions.length
  const failures = entries.filter((entry) => entry.level === 'ERROR').length
  const levelCounts = useMemo(
    () => Object.fromEntries(LEVELS.slice(1).map((item) => [item, entries.filter((entry) => entry.level === item).length])),
    [entries],
  )
  const streamEntries = useMemo(() => [...entries].reverse(), [entries])

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
    <AppHeader breadcrumbs={[{ label: 'Observe' }, { label: 'Logs' }]} />
    <main className={`${AURORA_PAGE_SHELL} flex min-h-0 flex-col gap-4`}>
      <section className="rounded-aurora-2 border border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] px-6 py-5 shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,.05)]">
        <div className="flex items-center gap-2 text-[10px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Observability <span className={cn('size-1.5 rounded-full', following ? 'bg-aurora-success shadow-[0_0_7px_var(--aurora-success)]' : 'bg-aurora-warn')}/><span className={following ? 'text-aurora-success' : 'text-aurora-warn'}>{following ? 'Streaming · all sources' : 'Paused'}</span></div>
        <h1 className="mt-2 font-display text-3xl font-extrabold leading-none text-aurora-text-primary">Logs</h1>
      </section>
      <div className="flex h-[calc(100vh-14.25rem)] min-h-0 flex-col">
      <section className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-aurora-2 border border-aurora-border-strong bg-[rgba(3,12,18,0.94)] shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.04)]">
        <form className="flex flex-wrap items-center gap-2 border-b border-aurora-border-default bg-aurora-panel-strong px-3 py-2" onSubmit={(event) => { event.preventDefault(); void load() }}>
          <div className="relative min-w-64 flex-1 sm:max-w-80"><Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-muted"/><input value={query} onChange={(event) => setQuery(event.target.value)} aria-label="Filter log lines" className="h-8 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg pl-8 pr-3 font-mono text-xs outline-none focus:border-aurora-accent-primary" placeholder="Filter lines…"/></div>
          <span className="hidden h-6 w-px bg-aurora-border-default sm:block" aria-hidden="true" />
          <span className="font-mono text-[9px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Source</span>
          <label className="relative">
            <select value={source} onChange={(event) => setSource(event.target.value)} aria-label="Log source" className="h-8 min-w-36 appearance-none rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg pl-3 pr-8 text-xs font-semibold text-aurora-text-primary outline-none focus:border-aurora-accent-primary">
              <option value="ALL">Gateway (all)</option>
              {sourceOptions.map((item) => <option key={item} value={item}>{item}</option>)}
            </select>
            <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 size-3 -translate-y-1/2 text-aurora-text-muted" />
          </label>
          <div className="flex rounded-aurora-1 border border-aurora-border-default bg-aurora-page-bg">{LEVELS.slice(1).map((item) => <button key={item} type="button" aria-pressed={level === item} className={cn('h-7 px-2 font-mono text-[9px] font-bold transition-colors first:rounded-l-md last:rounded-r-md', level === item ? 'bg-aurora-selected-bg text-aurora-accent-strong' : 'text-aurora-text-muted hover:text-aurora-text-primary')} onClick={() => setLevel(level === item ? 'ALL' : item)}><span className={cn('mr-1', item === 'ERROR' ? 'text-aurora-error' : item === 'WARN' ? 'text-aurora-warn' : item === 'INFO' ? 'text-aurora-accent-primary' : '')}>•</span>{item} {levelCounts[item]}</button>)}</div>
          <div className="ml-auto flex items-center gap-1.5">
            <Button variant="outline" size="sm" className="h-8" title={following ? 'Pause live tail' : 'Follow live tail'} onClick={() => setFollowing((value) => !value)}>{following ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}{following ? 'Pause' : 'Follow'}</Button>
            <Button variant="outline" size="sm" className="h-8" title="Download JSONL" onClick={download} disabled={entries.length === 0}><Download className="size-3.5" />Download</Button>
            <span className="rounded-aurora-1 border border-aurora-border-default px-2.5 py-1.5 font-mono text-[10px] text-aurora-text-secondary"><span className="mr-1 text-aurora-warn">•</span>{entries.length} of {meta.matched} lines</span>
          </div>
        </form>
        {error ? <div role="alert" className="flex items-center gap-2 border-b border-aurora-error/30 bg-aurora-error/10 px-3 py-2 font-mono text-xs text-aurora-error"><TriangleAlert className="size-4"/>{error}</div> : null}
        <div className="grid min-w-[760px] grid-cols-[18px_90px_54px_130px_minmax(300px,1fr)] gap-2 border-b border-aurora-border-default bg-aurora-page-bg px-2 py-2 font-mono text-[9px] font-bold uppercase tracking-[.16em] text-aurora-text-muted"><span/><span>Time</span><span>Level</span><span>Source</span><span>Message</span></div>
        <div ref={streamRef} role="log" aria-live={following ? 'polite' : 'off'} className="aurora-scrollbar min-h-0 flex-1 overflow-auto font-mono text-[11px] leading-5 sm:text-xs">
          {streamEntries.map((entry, index) => {
            const lineKey = `${entry.timestamp}-${index}`
            const expanded = expandedLine === lineKey
            const levelTone = entry.level === 'ERROR' ? 'text-aurora-error' : entry.level === 'WARN' ? 'text-aurora-warn' : entry.level === 'DEBUG' ? 'text-aurora-text-muted' : 'text-aurora-success'
            const rowTone = entry.level === 'ERROR' ? 'hover:border-aurora-error/80 hover:bg-aurora-error/5' : entry.level === 'INFO' ? 'hover:border-aurora-success/80 hover:bg-aurora-success/5' : 'hover:border-aurora-accent-primary/60 hover:bg-aurora-hover-bg/50'
            return <div key={lineKey} data-zebra-row="1" className={cn('group border-l-2 border-transparent', rowTone)}>
              <button type="button" onClick={() => setExpandedLine(expanded ? null : lineKey)} className="grid w-full min-w-[760px] grid-cols-[14px_90px_54px_130px_minmax(300px,1fr)] items-baseline gap-2 px-2 py-px text-left">
                <ChevronRight className={cn('size-3 self-center text-aurora-text-muted transition-transform', expanded && 'rotate-90')}/>
                <span className="text-aurora-text-muted">{entry.timestamp ? new Date(entry.timestamp).toLocaleTimeString([], { hour12: false }) : '--:--:--'}</span>
                <span className={cn('font-bold', levelTone)}>{(entry.level ?? '—').padEnd(5)}</span>
                <span className={cn('truncate', entry.level === 'ERROR' ? 'text-aurora-error' : index % 3 === 0 ? 'text-aurora-success' : 'text-aurora-accent-strong')} title={entry.service ?? entry.target ?? ''}>{entry.service ?? entry.target ?? '—'}</span>
                <span className="whitespace-pre-wrap break-words text-aurora-text-primary">{entry.message ?? entry.action ?? '—'}</span>
              </button>
              {expanded ? <pre className="mx-4 mb-2 overflow-auto border-l border-aurora-border-strong bg-aurora-page-bg/70 px-4 py-2 text-[11px] leading-5 text-aurora-text-secondary"><code>{JSON.stringify(entry.fields, null, 2)}</code></pre> : null}
            </div>
          })}
          {loading && entries.length === 0 ? <div className="grid min-h-56 place-items-center text-aurora-text-muted"><Loader2 className="size-5 animate-spin"/></div> : null}
          {!loading && !error && entries.length === 0 ? <div className="grid min-h-56 place-items-center text-sm text-aurora-text-muted">No log lines match this filter.</div> : null}
        </div>
        <footer className="flex flex-wrap items-center justify-between gap-2 border-t border-aurora-border-default bg-aurora-panel-strong px-3 py-1.5 font-mono text-[10px] text-aurora-text-muted"><span>{entries.length}/{meta.matched} lines · {sources} sources · <span className={failures ? 'text-aurora-error' : ''}>{failures} errors</span></span><span>{meta.scanned} scanned{meta.malformed ? ` · ${meta.malformed} malformed` : ''}{meta.truncated ? ' · truncated' : ''}</span></footer>
      </section>
      </div>
    </main>
  </>
}
