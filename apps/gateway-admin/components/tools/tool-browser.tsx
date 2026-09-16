'use client'

import { FormEvent, useEffect, useRef, useState } from 'react'
import { Grid2X2, List, RefreshCw, Search, ShieldCheck, Table2, TriangleAlert, Wrench } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { LibraryTabs } from '@/components/depot/depot-workspace-pages'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL, AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { subscribeToBrowserSession } from '@/lib/auth/session-store'
import { describeCodeModeTool, searchCodeModeTools, type ToolDescription, type ToolSearchHit } from '@/lib/api/tool-browser-client'
import { GatewayApiError } from '@/lib/api/gateway-client-core'
import { useGatewayCodeModeConfig } from '@/lib/hooks/use-gateways'

type BrowserError = { message: string; status?: number; requestId?: string; retry?: () => void }

function isAbortError(error: unknown) {
  return error instanceof DOMException && error.name === 'AbortError'
}

export function toolBrowserError(error: unknown, fallback: string): BrowserError {
  if (error instanceof GatewayApiError) {
    if (error.status === 401) return { message: 'Sign in to search tools.', status: 401, requestId: error.requestId }
    if (error.status === 403) return { message: 'Administrator access is required.', status: 403, requestId: error.requestId }
    if (error.status >= 500) return { message: 'Tools are temporarily unavailable.', status: error.status, requestId: error.requestId }
    return { message: error.message, status: error.status, requestId: error.requestId }
  }
  return { message: error instanceof Error ? error.message : fallback }
}

export interface ResultsSummaryState {
  total: number
  shown: number
  hasError: boolean
  /** The query that produced the current results, or `null` before any search. */
  executedQuery: string | null
  loading: boolean
  /** `codeModeConfig?.enabled` — `undefined` when unknown. */
  codeModeEnabled: boolean | undefined
  codeModeConfigMissing: boolean
  codeModeConfigFailed: boolean
}

/**
 * Pick the one-line label under the search box.
 *
 * Exported and pure because this is the whole point of the fix: a completed
 * "Browse all" that finds nothing used to render the same placeholder as
 * before any search ran, so nothing told the operator the request had
 * happened — which is exactly what a disabled Code Mode looks like, since the
 * search endpoint reports 0 tools rather than why. The rules are subtle enough
 * to deserve direct tests, and they cannot be driven through the component:
 * the unit-test DOM does not implement React's synthetic change events, so a
 * simulated keystroke silently does nothing and any test written that way
 * passes against the bug as readily as against the fix.
 */
export function resultsSummary(state: ResultsSummaryState): string {
  // Keep the count when a *detail* fetch fails: `loadDetail` sets `error`
  // without touching results, and blanking the label while result cards are
  // still on screen just looks broken.
  if (state.total > 0) {
    return `${state.total} matches${state.shown < state.total ? ` · showing ${state.shown}` : ''}`
  }
  if (state.hasError) return ''
  if (state.executedQuery === null || state.loading) {
    return 'Search, or browse the live catalog without a query'
  }
  // Branch on the query that produced these results, not the live input —
  // otherwise clearing the box after a failed search silently upgrades
  // "no matches for zzz" into a claim about every connected server.
  if (state.executedQuery.trim()) return 'No matching tools'
  if (state.codeModeEnabled === false) {
    return 'Code Mode is disabled, so this catalog is empty. Enable it from Gateway.'
  }
  // Check the failure before the missing check, not inside it. SWR serves the
  // last-good `data` when a revalidation fails, so a stale-but-present config
  // with a live fetch error would otherwise fall through to the confident
  // gateway-wide claim below while we are in fact flying blind.
  if (state.codeModeConfigFailed) {
    return 'No tools returned, and the Code Mode setting could not be read to explain why.'
  }
  if (state.codeModeConfigMissing || state.codeModeEnabled === undefined) {
    // Still loading, or loaded without a usable `enabled` flag. Either way we
    // cannot say whether Code Mode is the reason, so do not assert that it is
    // not.
    return 'No tools returned.'
  }
  return 'No tools exposed by any connected server.'
}

export function ToolBrowser({ initialQuery = '' }: { initialQuery?: string } = {}) {
  const [query, setQuery] = useState(initialQuery)
  const [results, setResults] = useState<ToolSearchHit[]>([])
  const [total, setTotal] = useState(0)
  const [detail, setDetail] = useState<ToolDescription | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<BrowserError | null>(null)
  // The query that produced `results`/`total`, NOT the live input value.
  // `query` changes on every keystroke while results only change on submit,
  // so branching the summary on `query` let a cleared input turn a stale
  // zero-result search into a confident claim about the whole gateway.
  const [executedQuery, setExecutedQuery] = useState<string | null>(null)
  const [view, setView] = useState<'table' | 'list' | 'cards'>('table')
  const activeRequest = useRef<AbortController | null>(null)
  const { data: codeModeConfig, error: codeModeConfigError } = useGatewayCodeModeConfig()

  useEffect(() => {
    const clearForSessionChange = () => {
      activeRequest.current?.abort()
      activeRequest.current = null
      setResults([]); setTotal(0); setDetail(null); setSelectedId(null); setError(null); setLoading(false); setExecutedQuery(null)
    }
    const unsubscribe = subscribeToBrowserSession(clearForSessionChange)
    return () => { unsubscribe(); activeRequest.current?.abort() }
  }, [])

  async function runSearch(value: string) {
    activeRequest.current?.abort(); const controller = new AbortController(); activeRequest.current = controller
    setDetail(null); setSelectedId(null); setError(null); setResults([]); setTotal(0)
    setLoading(true)
    try {
      const response = await searchCodeModeTools(value, controller.signal)
      if (activeRequest.current !== controller) return
      setResults(response.results); setTotal(response.total); setExecutedQuery(value)
    } catch (cause) {
      if (activeRequest.current === controller && !isAbortError(cause)) {
        setError({ ...toolBrowserError(cause, 'Tools unavailable'), retry: () => void runSearch(value) })
      }
    } finally { if (activeRequest.current === controller) setLoading(false) }
  }

  async function selectTool(hit: ToolSearchHit) {
    setSelectedId(hit.id)
    await loadDetail(hit.id)
  }

  async function loadDetail(target: string) {
    activeRequest.current?.abort(); const controller = new AbortController(); activeRequest.current = controller
    setDetail(null); setError(null); setLoading(true)
    try {
      const response = await describeCodeModeTool(target, controller.signal)
      if (activeRequest.current === controller) setDetail(response)
    }
    catch (cause) {
      if (activeRequest.current === controller && !isAbortError(cause)) {
        setError({ ...toolBrowserError(cause, 'Tool not found'), retry: () => void loadDetail(target) })
      }
    }
    finally { if (activeRequest.current === controller) setLoading(false) }
  }

  const summary = resultsSummary({
    total,
    shown: results.length,
    hasError: error !== null,
    executedQuery,
    loading,
    codeModeEnabled: codeModeConfig?.enabled,
    codeModeConfigMissing: codeModeConfig === undefined,
    codeModeConfigFailed: Boolean(codeModeConfigError),
  })

  const readOnlyCount = results.filter((hit) => hit.safety?.read_only).length
  const destructiveCount = results.filter((hit) => hit.safety?.destructive).length

  const serverCount = new Set(results.map((hit) => hit.namespace)).size

  return <>
    <AppHeader breadcrumbs={[{ label: 'Labby' }, { label: 'Library' }, { label: 'Tools' }]} />
    <main className={AURORA_PAGE_SHELL + ' flex-1'}>
      <div className={AURORA_PAGE_FRAME}>
        <ConsoleHero
          eyebrow="Labby · Library"
          title="Tools"
          pulse={executedQuery !== null && !error ? { color: 'var(--aurora-success)', label: 'Catalog live' } : undefined}
          stats={[
            { label: 'Tools', value: total || results.length, icon: <Wrench className="size-3" /> },
            { label: 'Servers', value: serverCount, icon: <Wrench className="size-3" /> },
            { label: 'Read only', value: readOnlyCount, tone: 'var(--aurora-success)', icon: <ShieldCheck className="size-3" /> },
            { label: 'Destructive', value: destructiveCount, tone: 'var(--aurora-error)', icon: <TriangleAlert className="size-3" /> },
          ]}
          footer={<LibraryTabs active="tools" attached counts={{ tools: total || results.length }} />}
          actions={<Button variant="outline" size="icon" aria-label="Refresh tools" title="Refresh tools" className="size-9 rounded-[10px]" disabled={loading} onClick={() => void runSearch(executedQuery ?? query)}><RefreshCw className={loading ? 'size-4 animate-spin' : 'size-4'} /></Button>}
        />
        <section className={AURORA_STRONG_PANEL}>
          <form className="flex flex-wrap items-center gap-2 border-b border-aurora-border-default p-3" onSubmit={(event: FormEvent) => { event.preventDefault(); void runSearch(query) }}>
            <div className="relative min-w-[220px] max-w-xl flex-1"><Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted" /><Input className="pl-9" aria-label="Search tools" maxLength={1024} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search tools, servers, descriptions, or tags…" /></div>
            <Button type="submit" size="icon" variant="outline" aria-label={query.trim() ? 'Search tools' : 'Browse all tools'} title={query.trim() ? 'Search tools' : 'Browse all tools'} disabled={loading}><Search className="size-4" /></Button>
            <span className="min-w-0 text-[11px] font-semibold text-aurora-text-muted">{summary}</span>
            <div className="ml-auto flex shrink-0 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5">{([[Table2, 'Table', 'table'], [List, 'List', 'list'], [Grid2X2, 'Cards', 'cards']] as const).map(([Icon, label, mode]) => <button key={mode} type="button" aria-label={label + ' view'} title={label + ' view'} aria-pressed={view === mode} onClick={() => setView(mode)} className="rounded p-1.5 text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-pressed:bg-aurora-selected-bg aria-pressed:text-aurora-accent-primary"><Icon className="size-3.5" /></button>)}</div>
          </form>
          {error && <div role="alert" className="m-3 flex items-center gap-2 rounded-aurora-1 border border-aurora-error/40 bg-aurora-error/10 p-3 text-sm"><TriangleAlert className="size-4" /><span>{error.message}{error.requestId ? ' Request ID: ' + error.requestId : ''}</span>{error.status !== 401 && error.status !== 403 && error.retry && <Button variant="ghost" size="sm" onClick={error.retry}>Retry</Button>}</div>}
          {results.length === 0 && !loading ? <div className="grid min-h-64 place-items-center px-6 text-center text-sm text-aurora-text-muted">Search or browse the connected tool catalog.</div> : null}
          {results.length > 0 && view === 'table' ? <div className="overflow-x-auto"><table aria-label="Library tools" className="w-full table-fixed border-collapse text-left"><thead className="bg-aurora-control-surface text-[9.5px] font-bold uppercase tracking-[.12em] text-aurora-text-muted"><tr><th className="w-[20%] px-4 py-2.5">Server</th><th className="w-[28%] px-4 py-2.5">Tool</th><th className="w-[18%] px-4 py-2.5">Tags</th><th className="w-[15%] px-4 py-2.5">Safety</th><th className="px-4 py-2.5">Signature</th></tr></thead><tbody className="divide-y divide-aurora-border-subtle">{results.map((hit) => <tr key={hit.id} className={selectedId === hit.id ? 'bg-aurora-selected-bg/50' : 'hover:bg-aurora-hover-bg/50'}><td className="truncate px-4 py-2.5 text-xs font-semibold text-aurora-text-muted" title={hit.namespace}>{hit.namespace}</td><td className="px-4 py-2.5"><button type="button" aria-pressed={selectedId === hit.id} onClick={() => void selectTool(hit)} className="max-w-full truncate text-left text-[12.5px] font-semibold text-aurora-accent-strong hover:underline">{hit.path}</button><p className="mt-0.5 truncate text-[10.5px] text-aurora-text-muted">{hit.description || 'No description provided.'}</p></td><td className="px-4 py-2.5"><div className="flex flex-wrap gap-1">{hit.tags.slice(0, 3).map((tag) => <span key={tag} className="rounded-[5px] border border-aurora-border-default px-1.5 py-0.5 text-[9.5px] text-aurora-text-muted">#{tag}</span>)}</div></td><td className="px-4 py-2.5"><Safety safety={hit.safety} /></td><td className="truncate px-4 py-2.5 font-mono text-[10.5px] text-aurora-text-muted" title={hit.signature}>{hit.signature}</td></tr>)}</tbody></table></div> : null}
          {results.length > 0 && view !== 'table' ? <div className={view === 'cards' ? 'grid gap-3 p-3 sm:grid-cols-2 xl:grid-cols-3' : 'divide-y divide-aurora-border-subtle'}>{results.map((hit) => <button key={hit.id} type="button" aria-pressed={selectedId === hit.id} onClick={() => void selectTool(hit)} className={view === 'cards' ? 'rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4 text-left transition hover:-translate-y-0.5 hover:border-aurora-border-strong aria-pressed:border-aurora-accent-primary' : 'block w-full px-4 py-3 text-left transition hover:bg-aurora-hover-bg aria-pressed:bg-aurora-selected-bg'}><div className="flex items-center justify-between gap-2"><code className="truncate text-[12.5px] font-semibold text-aurora-accent-strong">{hit.path}</code><Safety safety={hit.safety} /></div><p className="mt-1 line-clamp-2 text-xs leading-5 text-aurora-text-primary">{hit.description || 'No description provided.'}</p><p className="mt-1 truncate text-[10.5px] text-aurora-text-muted">{hit.namespace}{hit.tags.length ? ' · #' + hit.tags.join(' #') : ''}</p></button>)}</div> : null}
          {detail ? <section aria-label="Tool details" className="border-t border-aurora-border-default bg-aurora-panel-medium/35 px-5 py-4"><div className="flex flex-wrap items-start justify-between gap-3"><div className="min-w-0"><code className="break-all text-sm font-bold text-aurora-accent-strong">{detail.path}</code><p className="mt-1.5 max-w-3xl text-[12.5px] leading-[1.55] text-aurora-text-primary">{detail.description}</p></div><Safety safety={detail.safety} /></div><div className="mt-3 grid gap-3 lg:grid-cols-[minmax(220px,.55fr)_minmax(0,1.45fr)]"><dl className="grid content-start gap-2 text-[11px]"><div><dt className="text-[9.5px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">ID</dt><dd className="mt-1 break-all font-mono text-aurora-text-primary">{detail.id}</dd></div><div><dt className="text-[9.5px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">Helper</dt><dd className="mt-1 break-all font-mono text-aurora-text-primary">{detail.helper}</dd></div></dl><div><h2 className="text-[9.5px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">Parameters · TypeScript</h2>{detail.typescript ? <pre className="aurora-scrollbar mt-2 max-h-[300px] overflow-auto rounded-[10px] border border-aurora-border-default bg-aurora-page-bg px-3.5 py-3 text-[11.5px] leading-[1.6] text-aurora-text-primary"><code>{detail.typescript}</code></pre> : <p className="mt-2 text-sm text-aurora-text-muted">Parameters unavailable{detail.typescript_omitted === 'size_limit' ? ' because the declaration exceeds the response limit.' : '.'}</p>}</div></div></section> : null}
        </section>
      </div>
    </main>
  </>
}

function Safety({ safety }: { safety?: { read_only?: boolean; destructive?: boolean } }) {
  if (!safety) return <span className="text-xs text-aurora-text-muted">Safety unknown</span>
  if (safety.destructive) return <span className="inline-flex items-center gap-1 text-xs text-aurora-warn"><TriangleAlert className="size-3" />Destructive</span>
  if (safety.read_only) return <span className="inline-flex items-center gap-1 text-xs text-aurora-success"><ShieldCheck className="size-3" />Read only</span>
  return <span className="text-xs text-aurora-text-muted">Safety unspecified</span>
}
